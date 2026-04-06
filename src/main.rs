use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear as TermClear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use life_tracking::{AppData, ListSortMode, TaskKey, TaskSortMode, parse_hex_color};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::block::Title;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

const VIM_HELP_START: &str = "# ----- life-tracking help -----";
const VIM_HELP_END: &str = "# ----- end life-tracking help -----";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    TaskLists,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskField {
    Title,
    Estimated,
    Actual,
    DueDate,
    ListName,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListField {
    Name,
    Priority,
    Color,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditTarget {
    Task(TaskKey, TaskField),
    List(u64, ListField),
}

struct EditorState {
    target: EditTarget,
    input: String,
    suggestion_index: usize,
    cursor_index: usize,
}

#[derive(Clone)]
enum ConfirmAction {
    DeleteTask(TaskKey),
    DeleteTasks(Vec<TaskKey>),
    DeleteList(u64),
}

struct ConfirmState {
    message: String,
    action: ConfirmAction,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MultiTaskEditMode {
    Menu,
    DueDate,
    ListName,
}

struct MultiTaskEditState {
    keys: Vec<TaskKey>,
    mode: MultiTaskEditMode,
    input: String,
    cursor_index: usize,
    suggestion_index: usize,
}

struct App {
    data: AppData,

    page: Page,
    running: bool,

    main_sort_mode: TaskSortMode,
    list_sort_mode: ListSortMode,
    list_sort_reversed: bool,
    time_left_reversed: bool,
    show_completed_in_due_mode: bool,

    search_active: bool,
    search: String,
    search_cursor_index: usize,
    list_search_active: bool,
    list_search: String,
    list_search_cursor_index: usize,

    selected_main_idx: usize,
    selected_list_idx: usize,
    selected_list_task_idx: usize,
    task_selection_anchor: Option<usize>,
    multi_selected_tasks: HashSet<TaskKey>,

    expanded_list_id: Option<u64>,
    expanded_task_key: Option<TaskKey>,
    editor: Option<EditorState>,
    multi_task_edit: Option<MultiTaskEditState>,
    confirm: Option<ConfirmState>,

    popup_messages: Vec<String>,
    show_popup: bool,
    cursor_blink_start: Instant,
    pending_full_redraw: bool,
    notes_text: String,
}

impl App {
    fn new(data: AppData, startup_messages: Vec<String>, notes_text: String) -> Self {
        let show_popup = !startup_messages.is_empty();
        Self {
            data,
            page: Page::Main,
            running: true,
            main_sort_mode: TaskSortMode::DueDate,
            list_sort_mode: ListSortMode::RemainingCount,
            list_sort_reversed: false,
            time_left_reversed: false,
            show_completed_in_due_mode: true,
            search_active: false,
            search: String::new(),
            search_cursor_index: 0,
            list_search_active: false,
            list_search: String::new(),
            list_search_cursor_index: 0,
            selected_main_idx: 0,
            selected_list_idx: 0,
            selected_list_task_idx: 0,
            task_selection_anchor: None,
            multi_selected_tasks: HashSet::new(),
            expanded_list_id: None,
            expanded_task_key: None,
            editor: None,
            multi_task_edit: None,
            confirm: None,
            popup_messages: startup_messages,
            show_popup,
            cursor_blink_start: Instant::now(),
            pending_full_redraw: false,
            notes_text,
        }
    }

    fn run(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.event_loop(&mut terminal);
        Self::restore_terminal(terminal)?;
        result
    }

    fn restore_terminal(
        mut terminal: Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    ) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    ) -> io::Result<()> {
        while self.running {
            if self.pending_full_redraw {
                terminal.clear()?;
                self.pending_full_redraw = false;
            }

            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key);
                }
            }
        }

        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(frame.area());

        let top_bar = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(82), Constraint::Percentage(18)])
            .split(layout[0]);

        self.draw_search_bar(frame, top_bar[0]);
        self.draw_now_panel(frame, top_bar[1]);

        match self.page {
            Page::Main => self.draw_main_page(frame, layout[1]),
            Page::TaskLists => self.draw_task_list_page(frame, layout[1]),
        }

        if let Some(list_id) = self.expanded_list_id {
            self.draw_list_expanded(frame, list_id);
        }

        if let Some(task_key) = self.expanded_task_key {
            self.draw_task_expanded(frame, task_key);
        }

        if self.show_popup {
            self.draw_popup(frame);
        }

        if self.confirm.is_some() {
            self.draw_confirm(frame);
        }

        if self.multi_task_edit.is_some() {
            self.draw_multi_task_edit(frame);
        }

        self.draw_controls(frame, layout[2]);
    }

    fn draw_controls(&self, frame: &mut Frame, area: Rect) {
        let (label, controls) = self.controls_text();
        let line = Line::from(vec![
            Span::styled(
                format!("[{}] ", label),
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(controls, Style::default().add_modifier(Modifier::BOLD)),
        ]);

        let paragraph = Paragraph::new(vec![line])
            .block(
                Block::default().borders(Borders::ALL).title(Span::styled(
                    "Keys",
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                )),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }

    fn draw_now_panel(&self, frame: &mut Frame, area: Rect) {
        let now = Local::now();
        let now_style = Style::default()
            .fg(current_accent_color())
            .add_modifier(Modifier::BOLD);
        let paragraph = Paragraph::new(vec![
            Line::from(Span::styled(
                now.format("%a %Y-%m-%d").to_string(),
                now_style,
            )),
            Line::from(Span::styled(
                now.format("%I:%M:%S %p").to_string(),
                now_style,
            )),
        ])
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(paragraph, area);
    }

    fn draw_search_bar(&self, frame: &mut Frame, area: Rect) {
        let (line, active, cursor_index, title) = match self.page {
            Page::Main => {
                let line = if self.search.is_empty() {
                    Line::from("")
                } else {
                    Line::from(Span::raw(self.search.clone()))
                };
                let title = if self.search_active {
                    "Search -- [enter] apply search, [esc] clear"
                } else if self.search.is_empty() {
                    "Search -- [/] start search"
                } else {
                    "Search -- [/] edit search, [esc] clear search"
                };
                (line, self.search_active, self.search_cursor_index, title)
            }
            Page::TaskLists => {
                let line = if self.list_search.is_empty() {
                    Line::from("")
                } else {
                    Line::from(Span::raw(self.list_search.clone()))
                };
                let title = if self.list_search_active {
                    "Search -- [enter] apply search, [esc] clear"
                } else if self.list_search.is_empty() {
                    "Search -- [/] start search"
                } else {
                    "Search -- [/] edit search, [esc] clear search"
                };
                (
                    line,
                    self.list_search_active,
                    self.list_search_cursor_index,
                    title,
                )
            }
        };

        frame.render_widget(
            Paragraph::new(vec![line]).block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );

        if active && self.cursor_visible() {
            let value = match self.page {
                Page::Main => &self.search,
                Page::TaskLists => &self.list_search,
            };
            let cursor = self.cursor_position_for_input(area, value, cursor_index);
            frame.set_cursor_position(cursor);
        }
    }

    fn controls_text(&self) -> (&'static str, String) {
        if let Some(state) = &self.multi_task_edit {
            return match state.mode {
                MultiTaskEditMode::Menu => (
                    "MULTI",
                    "[r] due date, [l] move to list, [d] delete selected, [esc] cancel"
                        .to_string(),
                ),
                MultiTaskEditMode::DueDate => (
                    "MULTI",
                    "type YYYY-MM-DD, [enter] save, [esc] cancel".to_string(),
                ),
                MultiTaskEditMode::ListName => (
                    "MULTI",
                    "type filter, [↑/↓] choose, [enter] save, [esc] cancel".to_string(),
                ),
            };
        }

        if self.confirm.is_some() {
            return ("CONFIRM", "[enter/y] yes, [esc/n] no".to_string());
        }

        if self.show_popup {
            return ("POPUP", "[enter/esc] close".to_string());
        }

        if let Some(editor) = &self.editor {
            if matches!(editor.target, EditTarget::Task(_, TaskField::ListName)) {
                return (
                    "EDIT",
                    "type filter, [↑/↓] choose, [enter] save, [esc] cancel".to_string(),
                );
            }

            return ("EDIT", "type value, [enter] save, [esc] cancel".to_string());
        }

        if self.expanded_task_key.is_some() {
            return ("TASK", "[esc] back, [delete] delete task".to_string());
        }

        if self.expanded_list_id.is_some() {
            let c_text = if self.main_sort_mode == TaskSortMode::TimeLeft {
                "[c] reverse"
            } else {
                "[c] hide/show done"
            };
            return (
                "LIST+",
                format!(
                    "[esc] back, [enter] task, [a] done, [d] delete task, [n] new, [t] name, [p] prio, [q] color, [x] sort, {}",
                    c_text
                ),
            );
        }

        match self.page {
            Page::Main => {
                let c_text = match self.main_sort_mode {
                    TaskSortMode::DueDate => {
                        if self.show_completed_in_due_mode {
                            "hide completed"
                        } else {
                            "show completed"
                        }
                    }
                    TaskSortMode::TimeLeft => {
                        if self.time_left_reversed {
                            "asc."
                        } else {
                            "desc."
                        }
                    }
                };
                let x_text = match self.main_sort_mode {
                    TaskSortMode::DueDate => "sort by time remaining",
                    TaskSortMode::TimeLeft => "sort by due date",
                };
                (
                    "MAIN",
                    format!(
                        "[c] {}, [x] {}, [⬇/⬆(j/k)] navigate, [q] quit app",
                        c_text, x_text
                    ),
                )
            }
            Page::TaskLists => (
                "LISTS",
                if self.list_sort_reversed {
                    format!(
                        "[c] asc., [x] {}, [⬇/⬆(j/k)] navigate, [q] quit app",
                        match self.list_sort_mode {
                            ListSortMode::RemainingCount => "sort by priority",
                            ListSortMode::Priority => "sort by remaining tasks",
                        }
                    )
                } else {
                    format!(
                        "[c] desc., [x] {}, [⬇/⬆(j/k)] navigate, [q] quit app",
                        match self.list_sort_mode {
                            ListSortMode::RemainingCount => "sort by priority",
                            ListSortMode::Priority => "sort by remaining tasks",
                        }
                    )
                }
            ),
        }
    }

    fn draw_main_page(&self, frame: &mut Frame, area: Rect) {
        let content_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(82), Constraint::Percentage(18)])
            .split(area);

        let items = self.data.main_task_items(
            &self.search,
            self.main_sort_mode,
            self.time_left_reversed,
            self.show_completed_in_due_mode,
        );

        let (rows, selected_render_idx) =
            self.build_task_rows(&items, self.main_sort_mode, &self.search, true);

        let title = match self.main_sort_mode {
            TaskSortMode::DueDate =>
                "Tasks (due date) -- [a] mark completed, [del/bckspc] delete task, [n] new task, [enter] open/edit task",
            TaskSortMode::TimeLeft =>
                "Tasks (time remaining) -- [a] mark completed, [del/bckspc] delete task, [n] new task, [enter] open/edit task",
        };

        let mut state = ListState::default();
        state.select(selected_render_idx);

        let list = List::new(rows)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, content_chunks[0], &mut state);
        self.draw_main_priority_panel(frame, content_chunks[1]);
    }

    fn draw_task_list_page(&self, frame: &mut Frame, area: Rect) {
        let items = self.filtered_task_list_items();
        let content_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(82), Constraint::Percentage(18)])
            .split(area);

        let rows: Vec<ListItem> = if items.is_empty() {
            vec![ListItem::new("No task lists available")]
        } else {
            items
                .iter()
                .map(|item| {
                    let completed = item.total_items.saturating_sub(item.remaining_items);
                    let row = format!(
                        "{} | {} to-do | Completed: {}/{}",
                        fit_column(&item.name, 24),
                        item.remaining_items,
                        completed,
                        item.total_items,
                    );
                    ListItem::new(Line::from(Span::styled(
                        row,
                        Style::default().fg(color_from_hex(&item.color_hex)),
                    )))
                })
                .collect()
        };

        let mut state = ListState::default();
        if !items.is_empty() {
            state.select(Some(self.selected_list_idx.min(items.len() - 1)));
        }

        let order = if self.list_sort_reversed {
            "(asc.)"
        } else {
            "(desc.)"
        };
        let title = match self.list_sort_mode {
            ListSortMode::RemainingCount => format!(
                "Task Lists (remaining tasks {}) -- [del/bckspc] delete task list, [n] new task list, [enter] open/edit list",
                order
            ),
            ListSortMode::Priority => format!(
                "Task Lists (priority {}) -- [del/bckspc] delete task list, [n] new task list, [enter] open/edit list",
                order
            ),
        };

        let list = List::new(rows)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, content_chunks[0], &mut state);
        self.draw_task_list_side_panel(frame, content_chunks[1], &items);
    }

    fn draw_multi_task_edit(&self, frame: &mut Frame) {
        let Some(state) = &self.multi_task_edit else {
            return;
        };

        let area = centered_rect(60, 40, frame.area());
        frame.render_widget(Clear, area);

        let mut selected_task_lines =
            vec![Line::from(format!("Selected tasks: {}", state.keys.len()))];
        for key in &state.keys {
            if let Some(task) = self.data.get_task(*key) {
                let list_color = self
                    .data
                    .get_list(key.list_id)
                    .map(|list| color_from_hex(&list.color_hex))
                    .unwrap_or(Color::White);
                selected_task_lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} | ", task.due_date),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::styled(empty_if_blank(&task.title), Style::default().fg(list_color)),
                ]));
            }
        }

        let mut lines = match state.mode {
            MultiTaskEditMode::Menu => vec![],
            MultiTaskEditMode::DueDate => vec![
                Line::from("Set due date for all selected tasks:"),
                Line::from(state.input.clone()),
                Line::from(""),
            ],
            MultiTaskEditMode::ListName => {
                let mut lines = vec![
                    Line::from("Move all selected tasks to list:"),
                    Line::from(state.input.clone()),
                    Line::from(""),
                    Line::from("Suggestions:"),
                ];
                for (idx, suggestion) in self
                    .data
                    .list_name_suggestions(&state.input)
                    .iter()
                    .enumerate()
                    .take(5)
                {
                    let style = if idx == state.suggestion_index {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    lines.push(Line::from(Span::styled(suggestion.clone(), style)));
                }
                lines
            }
        };
        lines.extend(selected_task_lines);

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Multi-Task Edit");
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
            area,
        );

        if self.cursor_visible()
            && matches!(
                state.mode,
                MultiTaskEditMode::DueDate | MultiTaskEditMode::ListName
            )
        {
            let cursor = self.cursor_position_for_input(area, &state.input, state.cursor_index);
            frame.set_cursor_position(cursor);
        }
    }

    fn draw_task_expanded(&self, frame: &mut Frame, task_key: TaskKey) {
        let Some(task) = self.data.get_task(task_key) else {
            return;
        };
        let (list_name, list_color) = self
            .data
            .get_list(task_key.list_id)
            .map(|list| (list.name.clone(), color_from_hex(&list.color_hex)))
            .unwrap_or_else(|| ("Unknown".to_string(), Color::White));

        let area = centered_rect(84, 78, frame.area());
        frame.render_widget(Clear, area);
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(list_color).add_modifier(Modifier::BOLD))
            .title(Span::styled(
                "Task",
                Style::default().fg(list_color).add_modifier(Modifier::BOLD),
            ));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let editor = self
            .editor
            .as_ref()
            .filter(|editor| matches!(editor.target, EditTarget::Task(_, _)));
        let suggestion_lines = editor.and_then(|active_editor| {
            if matches!(
                active_editor.target,
                EditTarget::Task(_, TaskField::ListName)
            ) {
                let suggestions = self.data.list_name_suggestions(&active_editor.input);
                Some(
                    suggestions
                        .iter()
                        .enumerate()
                        .map(|(idx, suggestion)| {
                            let style = if idx == active_editor.suggestion_index {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Gray)
                            };
                            Line::from(Span::styled(suggestion.clone(), style))
                        })
                        .collect::<Vec<Line>>(),
                )
            } else {
                None
            }
        });

        let content_chunks = if suggestion_lines.is_some() {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(8),
                    Constraint::Length(3),
                    Constraint::Length(7),
                ])
                .split(inner)
        } else if editor.is_some() {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(8), Constraint::Length(3)])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(8)])
                .split(inner)
        };

        let completion = if task.completed {
            match task.completed_on {
                Some(date) => format!("Yes ({})", date),
                None => "Yes".to_string(),
            }
        } else {
            "No".to_string()
        };

        let lines = vec![
            line_with_label("Title [t]", &empty_if_blank(&task.title), Color::LightBlue),
            line_with_label("Task List [l]", &list_name, list_color),
            line_with_label(
                "Estimated [e]",
                &format!("{} hours", task.estimated_minutes),
                Color::LightBlue,
            ),
            line_with_label(
                "Actual [w]",
                &format!("{} hours", task.actual_minutes),
                Color::LightBlue,
            ),
            line_with_label(
                "Due Date [r]",
                &format!("{}", task.due_date),
                Color::LightBlue,
            ),
            line_with_label("Completed [a]", &completion, Color::LightBlue),
            Line::from(""),
            line_with_multiline_label(
                "Description [d]",
                &empty_if_blank(&task.description),
                Color::LightBlue,
            ),
        ];
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: true }),
            content_chunks[0],
        );

        if let Some(active_editor) = editor {
            let input_title = if let EditTarget::Task(_, field) = active_editor.target {
                task_field_label(field).to_string()
            } else {
                "Edit".to_string()
            };
            frame.render_widget(
                Paragraph::new(active_editor.input.clone()).block(
                    Block::default().borders(Borders::ALL).title(Span::styled(
                        input_title,
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                ),
                content_chunks[1],
            );

            if self.cursor_visible() {
                let cursor = self.cursor_position_for_input(
                    content_chunks[1],
                    &active_editor.input,
                    active_editor.cursor_index,
                );
                frame.set_cursor_position(cursor);
            }
        }

        if let Some(lines) = suggestion_lines {
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default().borders(Borders::ALL).title(Span::styled(
                        "Lists",
                        Style::default()
                            .fg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                ),
                content_chunks[2],
            );
        }
    }

    fn draw_list_expanded(&self, frame: &mut Frame, list_id: u64) {
        let Some(list) = self.data.get_list(list_id) else {
            return;
        };

        let tasks = self.data.list_task_items(
            list_id,
            "",
            self.main_sort_mode,
            self.time_left_reversed,
            self.show_completed_in_due_mode,
        );

        let area = centered_rect(90, 84, frame.area());
        frame.render_widget(Clear, area);
        let list_color = color_from_hex(&list.color_hex);
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(list_color).add_modifier(Modifier::BOLD))
            .title(Span::styled(
                "List",
                Style::default().fg(list_color).add_modifier(Modifier::BOLD),
            ));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let list_editor = self
            .editor
            .as_ref()
            .filter(|editor| matches!(editor.target, EditTarget::List(_, _)));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if list_editor.is_some() {
                vec![
                    Constraint::Length(6),
                    Constraint::Min(0),
                    Constraint::Length(3),
                ]
            } else {
                vec![Constraint::Length(6), Constraint::Min(0)]
            })
            .split(inner);

        let remaining = list.tasks.iter().filter(|task| !task.completed).count();

        let meta_lines = vec![
            line_with_label("Task List", &list.name, list_color),
            line_with_label("Priority", &format!("{}", list.priority), Color::LightBlue),
            line_with_label("Color", &list.color_hex, Color::LightBlue),
            line_with_label(
                "Items",
                &format!("total {} | remaining {}", list.tasks.len(), remaining),
                Color::LightBlue,
            ),
        ];

        let meta = Paragraph::new(meta_lines)
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .wrap(Wrap { trim: true });
        frame.render_widget(meta, chunks[0]);

        let (rows, selected_render_idx) =
            self.build_task_rows(&tasks, self.main_sort_mode, "", false);

        let title = match self.main_sort_mode {
            TaskSortMode::DueDate => {
                if self.show_completed_in_due_mode {
                    "Due Date - (showing completed)"
                } else {
                    "Due Date - (completed hidden)"
                }
            }
            TaskSortMode::TimeLeft => {
                if self.time_left_reversed {
                    "Time Remaining - (asc.)"
                } else {
                    "Time Remaining - (desc.)"
                }
            }
        };

        let mut state = ListState::default();
        state.select(selected_render_idx);

        let list_widget = List::new(rows)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title(
                        Title::from("'⬇/⬆(j/k)' navigate, + 'shift' multiselect")
                            .alignment(Alignment::Right),
                    ),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list_widget, chunks[1], &mut state);

        if let Some(active_editor) = list_editor {
            let input_title = if let EditTarget::List(_, field) = active_editor.target {
                list_field_label(field).to_string()
            } else {
                "Edit".to_string()
            };
            frame.render_widget(
                Paragraph::new(active_editor.input.clone()).block(
                    Block::default().borders(Borders::ALL).title(Span::styled(
                        input_title,
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                ),
                chunks[2],
            );

            if self.cursor_visible() {
                let cursor = self.cursor_position_for_input(
                    chunks[2],
                    &active_editor.input,
                    active_editor.cursor_index,
                );
                frame.set_cursor_position(cursor);
            }
        }
    }

    fn cursor_visible(&self) -> bool {
        (self.cursor_blink_start.elapsed().as_millis() / 500).is_multiple_of(2)
    }

    fn cursor_position_for_input(
        &self,
        area: Rect,
        input: &str,
        cursor_index: usize,
    ) -> (u16, u16) {
        let max_offset = area.width.saturating_sub(3);
        let max_index = input.chars().count();
        let clamped = cursor_index.min(max_index);
        let offset = clamped.min(usize::from(max_offset)) as u16;
        (
            area.x.saturating_add(1).saturating_add(offset),
            area.y.saturating_add(1),
        )
    }

    fn draw_popup(&self, frame: &mut Frame) {
        let area = centered_rect(80, 55, frame.area());
        frame.render_widget(Clear, area);

        let mut lines = Vec::new();

        for message in &self.popup_messages {
            lines.push(Line::from(format!("- {}", message)));
        }

        lines.push(Line::from(""));
        lines.push(Line::from("Press Enter or Esc to close this popup."));

        let popup = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Info"))
            .wrap(Wrap { trim: true });

        frame.render_widget(popup, area);
    }

    fn draw_confirm(&self, frame: &mut Frame) {
        let Some(confirm) = &self.confirm else {
            return;
        };

        let area = centered_rect(58, 28, frame.area());
        frame.render_widget(Clear, area);

        let lines = vec![
            Line::from(confirm.message.clone()),
            Line::from(""),
            Line::from("Press Enter/Y to delete, Esc/N to cancel."),
        ];

        let popup = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Are you sure you want to delete? (this can't be undone)"),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(popup, area);
    }

    fn draw_main_priority_panel(&self, frame: &mut Frame, area: Rect) {
        let panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(area);

        let ranked = self.data.task_list_items(ListSortMode::Priority);
        let rows: Vec<ListItem> = if ranked.is_empty() {
            vec![ListItem::new("No task lists")]
        } else {
            ranked
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let completed = item.total_items.saturating_sub(item.remaining_items);
                    let row = format!(
                        "{:>2}. {} | {}/{}",
                        idx + 1,
                        fit_column(&item.name, 16),
                        completed,
                        item.total_items
                    );
                    ListItem::new(Line::from(Span::styled(
                        row,
                        Style::default().fg(color_from_hex(&item.color_hex)),
                    )))
                })
                .collect()
        };

        let panel =
            List::new(rows).block(Block::default().borders(Borders::ALL).title("Task Lists - [u]"));
        frame.render_widget(panel, panels[0]);

        let notes_content = if self.notes_text.trim().is_empty() {
            "(empty)".to_string()
        } else {
            self.notes_text.clone()
        };
        let notes = Paragraph::new(notes_content)
            .block(Block::default().borders(Borders::ALL).title("Notes - [o]"))
            .wrap(Wrap { trim: false });
        frame.render_widget(notes, panels[1]);
    }

    fn draw_task_list_side_panel(
        &self,
        frame: &mut Frame,
        area: Rect,
        items: &[life_tracking::TaskListViewItem],
    ) {
        let panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(area);

        let task_rows: Vec<ListItem> = if let Some(item) = items.get(self.selected_list_idx) {
            if let Some(list) = self.data.get_list(item.list_id) {
                let completed = list.tasks.iter().filter(|task| task.completed).count();
                let mut rows = vec![ListItem::new(format!("{}/{} completed", completed, list.tasks.len()))];
                if list.tasks.is_empty() {
                    rows.push(ListItem::new("No tasks in list"));
                } else {
                    rows.extend(list.tasks.iter().map(|task| {
                        ListItem::new(Line::from(Span::styled(
                            empty_if_blank(&task.title),
                            Style::default().fg(color_from_hex(&list.color_hex)),
                        )))
                    }));
                }
                rows
            } else {
                vec![ListItem::new("No list selected")]
            }
        } else {
            vec![ListItem::new("No list selected")]
        };

        let tasks_panel =
            List::new(task_rows).block(Block::default().borders(Borders::ALL).title("Tasks - [u]"));
        frame.render_widget(tasks_panel, panels[0]);

        let notes_content = if self.notes_text.trim().is_empty() {
            "(empty)".to_string()
        } else {
            self.notes_text.clone()
        };
        let notes = Paragraph::new(notes_content)
            .block(Block::default().borders(Borders::ALL).title("Notes - [o]"))
            .wrap(Wrap { trim: false });
        frame.render_widget(notes, panels[1]);
    }

    fn build_task_rows(
        &self,
        items: &[life_tracking::TaskViewItem],
        sort_mode: TaskSortMode,
        search_query: &str,
        show_description_match_preview: bool,
    ) -> (Vec<ListItem<'static>>, Option<usize>) {
        if items.is_empty() {
            if sort_mode == TaskSortMode::DueDate {
                return (vec![due_date_header_item(AppData::today())], None);
            }
            return (vec![ListItem::new("No matching tasks")], None);
        }

        let selected_task_idx =
            if self.expanded_list_id.is_some() && self.expanded_task_key.is_none() {
                self.selected_list_task_idx.min(items.len() - 1)
            } else {
                self.selected_main_idx.min(items.len() - 1)
            };

        let mut rows = Vec::new();
        let mut task_to_render_idx = Vec::with_capacity(items.len());
        let mut previous_due = None;
        let today = AppData::today();
        let mut inserted_today_header = false;

        for item in items {
            if sort_mode == TaskSortMode::DueDate
                && !inserted_today_header
                && previous_due.map(|due| due < today).unwrap_or(true)
                && item.due_date > today
            {
                rows.push(due_date_header_item(today));
                inserted_today_header = true;
            }

            if sort_mode == TaskSortMode::DueDate && previous_due != Some(item.due_date) {
                rows.push(due_date_header_item(item.due_date));
                previous_due = Some(item.due_date);
                if item.due_date == today {
                    inserted_today_header = true;
                }
            }

            let completed_marker = if item.completed { "[x]" } else { "[ ]" };
            let title = empty_if_blank(&item.title);
            let list_color = color_from_hex(&item.list_color_hex);
            let mut base_style = Style::default().fg(list_color);
            if self.multi_selected_tasks.contains(&item.key) {
                base_style = base_style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
            }

            let mut spans = vec![Span::styled(
                format!(
                    "{} {} | {} | ",
                    completed_marker,
                    fit_column(&title, 22),
                    fit_column(&item.list_name, 16)
                ),
                base_style,
            )];

            if show_description_match_preview {
                if let Some(preview) = description_match_preview(&item.description, search_query) {
                    let ellipsis_before = if preview.has_prefix { "... " } else { "" };
                    let ellipsis_after = if preview.has_suffix { " ..." } else { "" };
                    spans.push(Span::styled(ellipsis_before.to_string(), base_style));
                    spans.push(Span::styled(preview.before, base_style));
                    spans.push(Span::styled(
                        preview.matched,
                        base_style.add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(preview.after, base_style));
                    spans.push(Span::styled(ellipsis_after.to_string(), base_style));
                    spans.push(Span::styled(" | ".to_string(), base_style));
                }
            }

            spans.push(Span::styled(
                format!(
                    "est:{:>4}h actual:{:>4}h left:{:>4}h",
                    item.estimated_minutes, item.actual_minutes, item.time_left
                ),
                base_style,
            ));

            rows.push(ListItem::new(Line::from(spans)));
            task_to_render_idx.push(rows.len() - 1);
        }

        if sort_mode == TaskSortMode::DueDate && !inserted_today_header {
            rows.push(due_date_header_item(today));
        }

        let selected_render_idx = task_to_render_idx.get(selected_task_idx).copied();
        (rows, selected_render_idx)
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.multi_task_edit.is_some() {
            self.handle_multi_task_edit_key(key);
            return;
        }

        if self.confirm.is_some() {
            self.handle_confirm_key(key);
            return;
        }

        if self.show_popup {
            if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                self.show_popup = false;
            }
            return;
        }

        if self.editor.is_some() {
            self.handle_editor_key(key);
            return;
        }

        if self.expanded_task_key.is_some() {
            self.handle_task_expanded_key(key);
            return;
        }

        if self.expanded_list_id.is_some() {
            self.handle_list_expanded_key(key);
            return;
        }

        match self.page {
            Page::Main => self.handle_main_key(key),
            Page::TaskLists => self.handle_task_list_page_key(key),
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        if self.search_active {
            self.handle_search_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Char('/') => {
                self.search_active = true;
                self.search_cursor_index = self.search.chars().count();
            }
            KeyCode::Esc => {
                self.search.clear();
                self.search_cursor_index = 0;
                self.selected_main_idx = 0;
                self.clear_multi_selection();
            }
            KeyCode::Char('u') => {
                self.clear_multi_selection();
                self.page = Page::TaskLists;
            }
            KeyCode::Char('n') => self.create_task_from_main(),
            KeyCode::Char('o') => self.open_notes_editor(),
            KeyCode::Char('x') => {
                self.main_sort_mode = toggle_task_sort_mode(self.main_sort_mode);
                self.selected_main_idx = 0;
                self.clear_multi_selection();
            }
            KeyCode::Char('c') => {
                if self.main_sort_mode == TaskSortMode::TimeLeft {
                    self.time_left_reversed = !self.time_left_reversed;
                } else {
                    self.show_completed_in_due_mode = !self.show_completed_in_due_mode;
                }
                self.selected_main_idx = 0;
                self.clear_multi_selection();
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.extend_task_selection(1);
                } else {
                    self.move_main_selection(1);
                }
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.extend_task_selection(-1);
                } else {
                    self.move_main_selection(-1);
                }
            }
            KeyCode::Char('J') => self.extend_task_selection(1),
            KeyCode::Char('K') => self.extend_task_selection(-1),
            KeyCode::Char('j') => self.move_main_selection(1),
            KeyCode::Char('k') => self.move_main_selection(-1),
            KeyCode::Enter => {
                if self.multi_selected_tasks.len() > 1 {
                    self.open_multi_task_edit();
                } else {
                    let items = self.current_task_items();
                    if let Some(item) = items.get(self.selected_main_idx) {
                        self.clear_multi_selection();
                        self.expanded_task_key = Some(item.key);
                    }
                }
            }
            KeyCode::Char('a') => {
                let items = self.current_task_items();
                if let Some(item) = items.get(self.selected_main_idx) {
                    let today = AppData::today();
                    let result = self.data.toggle_task_completed(item.key, today);
                    self.run_io(result);
                    self.clear_multi_selection();
                    self.normalize_main_selection();
                }
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                let items = self.current_task_items();
                if let Some(item) = items.get(self.selected_main_idx) {
                    self.clear_multi_selection();
                    self.confirm = Some(ConfirmState {
                        message: format!("Delete task '{}'?", empty_if_blank(&item.title)),
                        action: ConfirmAction::DeleteTask(item.key),
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_task_list_page_key(&mut self, key: KeyEvent) {
        if self.list_search_active {
            self.handle_task_list_search_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Char('u') => {
                self.clear_multi_selection();
                self.page = Page::Main;
            }
            KeyCode::Char('/') => {
                self.list_search_active = true;
                self.list_search_cursor_index = self.list_search.chars().count();
            }
            KeyCode::Char('o') => self.open_notes_editor(),
            KeyCode::Char('x') => {
                self.list_sort_mode = toggle_list_sort_mode(self.list_sort_mode);
                self.selected_list_idx = 0;
            }
            KeyCode::Char('c') => {
                self.list_sort_reversed = !self.list_sort_reversed;
                self.selected_list_idx = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_list_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_list_selection(-1),
            KeyCode::Enter => {
                let items = self.filtered_task_list_items();
                if let Some(item) = items.get(self.selected_list_idx) {
                    self.clear_multi_selection();
                    self.expanded_list_id = Some(item.list_id);
                    self.selected_list_task_idx = 0;
                }
            }
            KeyCode::Char('n') => {
                let today = AppData::today();
                match self.data.create_task_list(today) {
                    Ok(list_id) => {
                        self.expanded_list_id = Some(list_id);
                        self.selected_list_task_idx = 0;
                        let items = self.filtered_task_list_items();
                        if let Some(idx) = items.iter().position(|item| item.list_id == list_id) {
                            self.selected_list_idx = idx;
                        }
                    }
                    Err(err) => self.push_popup(format!("failed to create task list: {}", err)),
                }
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                let items = self.filtered_task_list_items();
                if let Some(item) = items.get(self.selected_list_idx) {
                    self.confirm = Some(ConfirmState {
                        message: format!("Delete task list '{}' and all its tasks?", item.name),
                        action: ConfirmAction::DeleteList(item.list_id),
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_task_list_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.list_search_active = false;
                self.list_search.clear();
                self.list_search_cursor_index = 0;
                self.selected_list_idx = 0;
            }
            KeyCode::Enter => self.list_search_active = false,
            KeyCode::Backspace => {
                if self.list_search_cursor_index > 0 {
                    remove_char_before_cursor(
                        &mut self.list_search,
                        &mut self.list_search_cursor_index,
                    );
                }
                self.selected_list_idx = 0;
            }
            KeyCode::Delete => {
                remove_char_at_cursor(&mut self.list_search, self.list_search_cursor_index);
                self.selected_list_idx = 0;
            }
            KeyCode::Left => {
                self.list_search_cursor_index = self.list_search_cursor_index.saturating_sub(1);
            }
            KeyCode::Right => {
                self.list_search_cursor_index =
                    (self.list_search_cursor_index + 1).min(self.list_search.chars().count());
            }
            KeyCode::Home => self.list_search_cursor_index = 0,
            KeyCode::End => self.list_search_cursor_index = self.list_search.chars().count(),
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    insert_char_at_cursor(
                        &mut self.list_search,
                        &mut self.list_search_cursor_index,
                        c,
                    );
                    self.selected_list_idx = 0;
                }
            }
            _ => {}
        }
    }

    fn handle_list_expanded_key(&mut self, key: KeyEvent) {
        let Some(list_id) = self.expanded_list_id else {
            return;
        };

        match key.code {
            KeyCode::Esc => {
                self.expanded_list_id = None;
                self.selected_list_task_idx = 0;
            }
            KeyCode::Char('x') => {
                self.main_sort_mode = toggle_task_sort_mode(self.main_sort_mode);
                self.selected_list_task_idx = 0;
                self.clear_multi_selection();
            }
            KeyCode::Char('c') => {
                if self.main_sort_mode == TaskSortMode::TimeLeft {
                    self.time_left_reversed = !self.time_left_reversed;
                } else {
                    self.show_completed_in_due_mode = !self.show_completed_in_due_mode;
                }
                self.selected_list_task_idx = 0;
                self.clear_multi_selection();
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.extend_task_selection(1);
                } else {
                    self.move_list_task_selection(1);
                }
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.extend_task_selection(-1);
                } else {
                    self.move_list_task_selection(-1);
                }
            }
            KeyCode::Char('J') => self.extend_task_selection(1),
            KeyCode::Char('K') => self.extend_task_selection(-1),
            KeyCode::Char('j') => self.move_list_task_selection(1),
            KeyCode::Char('k') => self.move_list_task_selection(-1),
            KeyCode::Enter => {
                if self.multi_selected_tasks.len() > 1 {
                    self.open_multi_task_edit();
                } else {
                    let items = self.current_task_items();
                    if let Some(item) = items.get(self.selected_list_task_idx) {
                        self.clear_multi_selection();
                        self.expanded_task_key = Some(item.key);
                    }
                }
            }
            KeyCode::Char('a') => {
                let items = self.current_task_items();
                if let Some(item) = items.get(self.selected_list_task_idx) {
                    let today = AppData::today();
                    let result = self.data.toggle_task_completed(item.key, today);
                    self.run_io(result);
                    self.clear_multi_selection();
                    self.normalize_list_task_selection();
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                let items = self.current_task_items();
                if let Some(item) = items.get(self.selected_list_task_idx) {
                    self.clear_multi_selection();
                    self.confirm = Some(ConfirmState {
                        message: format!("Delete task '{}'?", empty_if_blank(&item.title)),
                        action: ConfirmAction::DeleteTask(item.key),
                    });
                }
            }
            KeyCode::Char('n') => {
                let today = AppData::today();
                let created = self.data.create_task_in_list(list_id, today);
                match created {
                    Ok(Some(key)) => {
                        self.clear_multi_selection();
                        self.expanded_task_key = Some(key);
                        self.selected_list_task_idx = 0;
                    }
                    Ok(None) => {}
                    Err(err) => self.push_popup(format!("failed to create task: {}", err)),
                }
            }
            KeyCode::Char('t') => self.start_list_edit(ListField::Name),
            KeyCode::Char('p') => self.start_list_edit(ListField::Priority),
            KeyCode::Char('q') => self.start_list_edit(ListField::Color),
            _ => {}
        }
    }

    fn handle_task_expanded_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.expanded_task_key = None,
            KeyCode::Char('a') => {
                if let Some(task_key) = self.expanded_task_key {
                    let today = AppData::today();
                    let result = self.data.toggle_task_completed(task_key, today);
                    self.run_io(result);
                }
            }
            KeyCode::Char('t') => self.start_task_edit(TaskField::Title),
            KeyCode::Char('d') => {
                if let Some(task_key) = self.expanded_task_key {
                    self.open_task_description_editor(task_key);
                }
            }
            KeyCode::Char('e') => self.start_task_edit(TaskField::Estimated),
            KeyCode::Char('w') => self.start_task_edit(TaskField::Actual),
            KeyCode::Char('r') => self.start_task_edit(TaskField::DueDate),
            KeyCode::Char('l') => self.start_task_edit(TaskField::ListName),
            KeyCode::Delete => {
                if let Some(task_key) = self.expanded_task_key {
                    let title = self
                        .data
                        .get_task(task_key)
                        .map(|task| empty_if_blank(&task.title))
                        .unwrap_or_else(|| "this task".to_string());
                    self.confirm = Some(ConfirmState {
                        message: format!("Delete task '{}'?", title),
                        action: ConfirmAction::DeleteTask(task_key),
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.apply_confirmed_action();
                self.confirm = None;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.confirm = None;
            }
            _ => {}
        }
    }

    fn handle_multi_task_edit_key(&mut self, key: KeyEvent) {
        let Some(state) = &mut self.multi_task_edit else {
            return;
        };

        match state.mode {
            MultiTaskEditMode::Menu => match key.code {
                KeyCode::Esc => self.multi_task_edit = None,
                KeyCode::Char('r') => {
                    state.mode = MultiTaskEditMode::DueDate;
                    state.input.clear();
                    state.cursor_index = 0;
                }
                KeyCode::Char('l') => {
                    state.mode = MultiTaskEditMode::ListName;
                    state.input.clear();
                    state.cursor_index = 0;
                    state.suggestion_index = 0;
                }
                KeyCode::Char('d') => self.request_multi_task_delete(),
                _ => {}
            },
            MultiTaskEditMode::DueDate => match key.code {
                KeyCode::Esc => state.mode = MultiTaskEditMode::Menu,
                KeyCode::Enter => self.commit_multi_task_due_date(),
                KeyCode::Backspace => {
                    if state.cursor_index > 0 {
                        remove_char_before_cursor(&mut state.input, &mut state.cursor_index);
                    }
                }
                KeyCode::Delete => remove_char_at_cursor(&mut state.input, state.cursor_index),
                KeyCode::Left => state.cursor_index = state.cursor_index.saturating_sub(1),
                KeyCode::Right => {
                    state.cursor_index = (state.cursor_index + 1).min(state.input.chars().count())
                }
                KeyCode::Home => state.cursor_index = 0,
                KeyCode::End => state.cursor_index = state.input.chars().count(),
                KeyCode::Char(c) if c.is_ascii_digit() || c == '-' => {
                    insert_char_at_cursor(&mut state.input, &mut state.cursor_index, c);
                }
                _ => {}
            },
            MultiTaskEditMode::ListName => match key.code {
                KeyCode::Esc => state.mode = MultiTaskEditMode::Menu,
                KeyCode::Enter => self.commit_multi_task_list_move(),
                KeyCode::Up => {
                    state.suggestion_index = state.suggestion_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let suggestions = self.data.list_name_suggestions(&state.input);
                    if !suggestions.is_empty() {
                        state.suggestion_index =
                            (state.suggestion_index + 1).min(suggestions.len() - 1);
                    }
                }
                KeyCode::Backspace => {
                    if state.cursor_index > 0 {
                        remove_char_before_cursor(&mut state.input, &mut state.cursor_index);
                    }
                    state.suggestion_index = 0;
                }
                KeyCode::Delete => {
                    remove_char_at_cursor(&mut state.input, state.cursor_index);
                    state.suggestion_index = 0;
                }
                KeyCode::Left => state.cursor_index = state.cursor_index.saturating_sub(1),
                KeyCode::Right => {
                    state.cursor_index = (state.cursor_index + 1).min(state.input.chars().count())
                }
                KeyCode::Home => state.cursor_index = 0,
                KeyCode::End => state.cursor_index = state.input.chars().count(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    insert_char_at_cursor(&mut state.input, &mut state.cursor_index, c);
                    state.suggestion_index = 0;
                }
                _ => {}
            },
        }
    }

    fn open_notes_editor(&mut self) {
        let current_notes = self.notes_text.clone();
        match self.edit_text_in_vim(
            "notes",
            &current_notes,
            &[
                "# Notes editor",
                "#",
                "# Edit freely, then save and quit with :wq",
                "# Quit without saving with :q!",
                "# Lines in this help block are ignored when saved",
            ],
        ) {
            Ok(content) => {
                let path = self.data.data_dir.join("notes.txt");
                if let Err(err) = fs::write(&path, &content) {
                    self.push_popup(format!("failed to save notes: {}", err));
                    return;
                }
                self.notes_text = content;
            }
            Err(err) => self.push_popup(format!("failed to open vim: {}", err)),
        }
    }

    fn create_task_from_main(&mut self) {
        let ranked_lists = self.data.task_list_items(ListSortMode::Priority);
        let Some(default_list) = ranked_lists.first() else {
            self.push_popup("no task list available for new task".to_string());
            return;
        };

        let today = AppData::today();
        match self.data.create_task_in_list(default_list.list_id, today) {
            Ok(Some(key)) => {
                self.clear_multi_selection();
                self.normalize_main_selection();
                self.expanded_task_key = Some(key);
            }
            Ok(None) => {}
            Err(err) => self.push_popup(format!("failed to create task: {}", err)),
        }
    }

    fn open_task_description_editor(&mut self, task_key: TaskKey) {
        let Some(task) = self.data.get_task(task_key) else {
            return;
        };
        let current_description = task.description.clone();
        match self.edit_text_in_vim(
            "description",
            &current_description,
            &[
                "# Task description editor",
                "#",
                "# Write any task details you want to keep",
                "# Save and quit with :wq",
                "# Quit without saving with :q!",
                "# Lines in this help block are ignored when saved",
            ],
        ) {
            Ok(content) => {
                let result = self
                    .data
                    .update_task_description(task_key, content, AppData::today());
                self.run_io(result);
            }
            Err(err) => self.push_popup(format!("failed to open vim: {}", err)),
        }
    }

    fn edit_text_in_vim(
        &mut self,
        file_stem: &str,
        initial_content: &str,
        help_lines: &[&str],
    ) -> io::Result<String> {
        let temp_path = self.data.data_dir.join(format!(
            ".life-tracking-{}-{}.tmp",
            file_stem,
            std::process::id()
        ));
        let mut temp_content = String::new();
        temp_content.push_str(VIM_HELP_START);
        temp_content.push('\n');
        for line in help_lines {
            temp_content.push_str(line);
            temp_content.push('\n');
        }
        temp_content.push_str(VIM_HELP_END);
        temp_content.push_str("\n\n");
        temp_content.push_str(initial_content);
        fs::write(&temp_path, temp_content)?;

        let mut stdout = io::stdout();
        disable_raw_mode()?;
        execute!(stdout, Show, LeaveAlternateScreen)?;
        stdout.flush()?;

        let edit_result = (|| -> io::Result<String> {
            let status = Command::new("vim")
                .arg("-c")
                .arg("setlocal filetype=gitcommit")
                .arg(&temp_path)
                .status()?;

            if !status.success() {
                return Err(io::Error::other(format!(
                    "vim exited with status {}",
                    status
                )));
            }

            fs::read_to_string(&temp_path).map(|content| strip_vim_help_block(&content))
        })();

        let restore_result = (|| -> io::Result<()> {
            enable_raw_mode()?;
            self.cursor_blink_start = Instant::now();
            self.pending_full_redraw = true;
            execute!(
                io::stdout(),
                EnterAlternateScreen,
                Hide,
                TermClear(ClearType::All)
            )?;
            Ok(())
        })();

        let _ = fs::remove_file(&temp_path);

        restore_result?;
        edit_result
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.search_active = false;
                self.search.clear();
                self.search_cursor_index = 0;
                self.selected_main_idx = 0;
            }
            KeyCode::Enter => self.search_active = false,
            KeyCode::Backspace => {
                if self.search_cursor_index > 0 {
                    remove_char_before_cursor(&mut self.search, &mut self.search_cursor_index);
                }
                self.selected_main_idx = 0;
            }
            KeyCode::Delete => {
                remove_char_at_cursor(&mut self.search, self.search_cursor_index);
                self.selected_main_idx = 0;
            }
            KeyCode::Left => {
                self.search_cursor_index = self.search_cursor_index.saturating_sub(1);
            }
            KeyCode::Right => {
                self.search_cursor_index =
                    (self.search_cursor_index + 1).min(self.search.chars().count());
            }
            KeyCode::Home => {
                self.search_cursor_index = 0;
            }
            KeyCode::End => {
                self.search_cursor_index = self.search.chars().count();
                self.selected_main_idx = 0;
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    insert_char_at_cursor(&mut self.search, &mut self.search_cursor_index, c);
                    self.selected_main_idx = 0;
                }
            }
            _ => {}
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        let Some((target, input, suggestion_index)) = self
            .editor
            .as_ref()
            .map(|editor| (editor.target, editor.input.clone(), editor.suggestion_index))
        else {
            return;
        };

        if let EditTarget::Task(_, TaskField::ListName) = target {
            match key.code {
                KeyCode::Down => {
                    let suggestions = self.data.list_name_suggestions(&input);
                    if !suggestions.is_empty() {
                        if let Some(editor) = &mut self.editor {
                            editor.suggestion_index =
                                (suggestion_index + 1).min(suggestions.len() - 1);
                        }
                    }
                    return;
                }
                KeyCode::Up => {
                    if let Some(editor) = &mut self.editor {
                        editor.suggestion_index = suggestion_index.saturating_sub(1);
                    }
                    return;
                }
                _ => {}
            }
        }

        let Some(editor) = &mut self.editor else {
            return;
        };

        match key.code {
            KeyCode::Esc => self.editor = None,
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Backspace => {
                if editor.cursor_index > 0 {
                    remove_char_before_cursor(&mut editor.input, &mut editor.cursor_index);
                }
                editor.suggestion_index = 0;
            }
            KeyCode::Delete => {
                remove_char_at_cursor(&mut editor.input, editor.cursor_index);
                editor.suggestion_index = 0;
            }
            KeyCode::Left => {
                editor.cursor_index = editor.cursor_index.saturating_sub(1);
            }
            KeyCode::Right => {
                editor.cursor_index = (editor.cursor_index + 1).min(editor.input.chars().count());
            }
            KeyCode::Home => editor.cursor_index = 0,
            KeyCode::End => editor.cursor_index = editor.input.chars().count(),
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return;
                }

                match editor.target {
                    EditTarget::Task(_, TaskField::Title) => {
                        if editor.input.chars().count() < 30 {
                            insert_char_at_cursor(&mut editor.input, &mut editor.cursor_index, c);
                        }
                    }
                    EditTarget::Task(_, TaskField::Estimated)
                    | EditTarget::Task(_, TaskField::Actual)
                    | EditTarget::List(_, ListField::Priority) => {
                        if c.is_ascii_digit() {
                            insert_char_at_cursor(&mut editor.input, &mut editor.cursor_index, c);
                        }
                    }
                    EditTarget::Task(_, TaskField::DueDate) => {
                        if c.is_ascii_digit() || c == '-' {
                            insert_char_at_cursor(&mut editor.input, &mut editor.cursor_index, c);
                        }
                    }
                    EditTarget::Task(_, TaskField::ListName)
                    | EditTarget::List(_, ListField::Name) => {
                        insert_char_at_cursor(&mut editor.input, &mut editor.cursor_index, c);
                        editor.suggestion_index = 0;
                    }
                    EditTarget::List(_, ListField::Color) => {
                        let valid_char = c.is_ascii_hexdigit() || c == '#';
                        if valid_char && editor.input.chars().count() < 7 {
                            insert_char_at_cursor(&mut editor.input, &mut editor.cursor_index, c);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn commit_edit(&mut self) {
        let Some(editor) = self.editor.take() else {
            return;
        };

        let today = AppData::today();

        match editor.target {
            EditTarget::Task(task_key, TaskField::Title) => {
                let result = self.data.update_task_title(task_key, editor.input, today);
                self.run_io(result);
            }
            EditTarget::Task(task_key, TaskField::Estimated) => {
                if let Ok(value) = editor.input.parse::<u32>() {
                    let result = self.data.update_task_estimated(task_key, value, today);
                    self.run_io(result);
                }
            }
            EditTarget::Task(task_key, TaskField::Actual) => {
                if let Ok(value) = editor.input.parse::<u32>() {
                    let result = self.data.update_task_actual(task_key, value, today);
                    self.run_io(result);
                }
            }
            EditTarget::Task(task_key, TaskField::DueDate) => {
                let input = editor.input.trim();
                if let Ok(value) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
                    let result = self.data.update_task_due_date(task_key, value, today);
                    self.run_io(result);
                } else {
                    self.push_popup("invalid due date format; use YYYY-MM-DD".to_string());
                }
            }
            EditTarget::Task(task_key, TaskField::ListName) => {
                let suggestions = self.data.list_name_suggestions(&editor.input);
                let selected_name = suggestions
                    .get(editor.suggestion_index)
                    .cloned()
                    .unwrap_or_else(|| editor.input.trim().to_string());

                if let Some(list_id) = self.data.find_list_id_by_name(&selected_name) {
                    let result = self.data.move_task_to_list(task_key, list_id, today);
                    self.run_io(result);
                    if self.expanded_list_id.is_some() {
                        self.normalize_list_task_selection();
                    } else {
                        self.normalize_main_selection();
                    }
                }
            }
            EditTarget::List(list_id, ListField::Name) => {
                let result = self.data.update_list_name(list_id, editor.input, today);
                self.run_io(result);
            }
            EditTarget::List(list_id, ListField::Priority) => {
                if let Ok(value) = editor.input.parse::<u8>() {
                    let result = self.data.update_list_priority(list_id, value, today);
                    self.run_io(result);
                }
            }
            EditTarget::List(list_id, ListField::Color) => {
                let result = self.data.update_list_color(list_id, editor.input, today);
                self.run_io(result);
            }
        }
    }

    fn start_task_edit(&mut self, field: TaskField) {
        let Some(task_key) = self.expanded_task_key else {
            return;
        };
        let Some(task) = self.data.get_task(task_key) else {
            return;
        };

        let input = match field {
            TaskField::Title => task.title.clone(),
            TaskField::Estimated => task.estimated_minutes.to_string(),
            TaskField::Actual => task.actual_minutes.to_string(),
            TaskField::DueDate => task.due_date.to_string(),
            TaskField::ListName => self
                .data
                .get_list(task_key.list_id)
                .map(|list| list.name.clone())
                .unwrap_or_default(),
        };

        self.editor = Some(EditorState {
            target: EditTarget::Task(task_key, field),
            input,
            suggestion_index: 0,
            cursor_index: 0,
        });
        if let Some(editor) = &mut self.editor {
            editor.cursor_index = editor.input.chars().count();
        }
    }

    fn start_list_edit(&mut self, field: ListField) {
        let Some(list_id) = self.expanded_list_id else {
            return;
        };
        let Some(list) = self.data.get_list(list_id) else {
            return;
        };

        let input = match field {
            ListField::Name => list.name.clone(),
            ListField::Priority => list.priority.to_string(),
            ListField::Color => list.color_hex.clone(),
        };

        self.editor = Some(EditorState {
            target: EditTarget::List(list_id, field),
            input,
            suggestion_index: 0,
            cursor_index: 0,
        });
        if let Some(editor) = &mut self.editor {
            editor.cursor_index = editor.input.chars().count();
        }
    }

    fn move_main_selection(&mut self, delta: isize) {
        self.clear_multi_selection();
        let items = self.data.main_task_items(
            &self.search,
            self.main_sort_mode,
            self.time_left_reversed,
            self.show_completed_in_due_mode,
        );
        self.selected_main_idx = shift_index(self.selected_main_idx, items.len(), delta);
    }

    fn move_list_selection(&mut self, delta: isize) {
        self.clear_multi_selection();
        let items = self.filtered_task_list_items();
        self.selected_list_idx = shift_index(self.selected_list_idx, items.len(), delta);
    }

    fn move_list_task_selection(&mut self, delta: isize) {
        self.clear_multi_selection();
        let Some(list_id) = self.expanded_list_id else {
            return;
        };

        let items = self.data.list_task_items(
            list_id,
            "",
            self.main_sort_mode,
            self.time_left_reversed,
            self.show_completed_in_due_mode,
        );

        self.selected_list_task_idx = shift_index(self.selected_list_task_idx, items.len(), delta);
    }

    fn normalize_main_selection(&mut self) {
        let len = self
            .data
            .main_task_items(
                &self.search,
                self.main_sort_mode,
                self.time_left_reversed,
                self.show_completed_in_due_mode,
            )
            .len();

        if len == 0 {
            self.selected_main_idx = 0;
        } else {
            self.selected_main_idx = self.selected_main_idx.min(len - 1);
        }
    }

    fn normalize_list_task_selection(&mut self) {
        let Some(list_id) = self.expanded_list_id else {
            self.selected_list_task_idx = 0;
            return;
        };

        let len = self
            .data
            .list_task_items(
                list_id,
                "",
                self.main_sort_mode,
                self.time_left_reversed,
                self.show_completed_in_due_mode,
            )
            .len();

        if len == 0 {
            self.selected_list_task_idx = 0;
        } else {
            self.selected_list_task_idx = self.selected_list_task_idx.min(len - 1);
        }
    }

    fn apply_confirmed_action(&mut self) {
        let Some(action) = self.confirm.as_ref().map(|confirm| confirm.action.clone()) else {
            return;
        };

        let today = AppData::today();
        match action {
            ConfirmAction::DeleteTask(task_key) => {
                let result = self.data.delete_task(task_key, today);
                self.run_io(result);

                if self.expanded_task_key == Some(task_key) {
                    self.expanded_task_key = None;
                }

                self.normalize_main_selection();
                self.normalize_list_task_selection();
            }
            ConfirmAction::DeleteTasks(keys) => {
                let result = self.data.delete_tasks(&keys, today);
                self.run_io(result);

                self.clear_multi_selection();
                self.normalize_main_selection();
                self.normalize_list_task_selection();
            }
            ConfirmAction::DeleteList(list_id) => {
                let result = self.data.delete_task_list(list_id, today);
                self.run_io(result);

                if self.expanded_list_id == Some(list_id) {
                    self.expanded_list_id = None;
                    self.selected_list_task_idx = 0;
                }
                if self
                    .expanded_task_key
                    .map(|task_key| task_key.list_id == list_id)
                    .unwrap_or(false)
                {
                    self.expanded_task_key = None;
                }

                self.normalize_list_selection();
                self.normalize_main_selection();
            }
        }
    }

    fn current_task_items(&self) -> Vec<life_tracking::TaskViewItem> {
        if let Some(list_id) = self.expanded_list_id {
            self.data.list_task_items(
                list_id,
                "",
                self.main_sort_mode,
                self.time_left_reversed,
                self.show_completed_in_due_mode,
            )
        } else {
            self.data.main_task_items(
                &self.search,
                self.main_sort_mode,
                self.time_left_reversed,
                self.show_completed_in_due_mode,
            )
        }
    }

    fn current_task_selection_index(&self) -> usize {
        if self.expanded_list_id.is_some() {
            self.selected_list_task_idx
        } else {
            self.selected_main_idx
        }
    }

    fn set_current_task_selection_index(&mut self, value: usize) {
        if self.expanded_list_id.is_some() {
            self.selected_list_task_idx = value;
        } else {
            self.selected_main_idx = value;
        }
    }

    fn clear_multi_selection(&mut self) {
        self.task_selection_anchor = None;
        self.multi_selected_tasks.clear();
        self.multi_task_edit = None;
    }

    fn extend_task_selection(&mut self, delta: isize) {
        let items = self.current_task_items();
        if items.is_empty() {
            self.clear_multi_selection();
            return;
        }

        let current = self.current_task_selection_index().min(items.len() - 1);
        let next = shift_index(current, items.len(), delta);
        let anchor = self.task_selection_anchor.unwrap_or(current);
        let start = anchor.min(next);
        let end = anchor.max(next);

        self.task_selection_anchor = Some(anchor);
        self.set_current_task_selection_index(next);
        self.multi_selected_tasks = items[start..=end].iter().map(|item| item.key).collect();
    }

    fn open_multi_task_edit(&mut self) {
        let items = self.current_task_items();
        let keys: Vec<TaskKey> = items
            .iter()
            .filter(|item| self.multi_selected_tasks.contains(&item.key))
            .map(|item| item.key)
            .collect();

        if keys.len() < 2 {
            return;
        }

        self.multi_task_edit = Some(MultiTaskEditState {
            keys,
            mode: MultiTaskEditMode::Menu,
            input: String::new(),
            cursor_index: 0,
            suggestion_index: 0,
        });
    }

    fn commit_multi_task_due_date(&mut self) {
        let Some(state) = self.multi_task_edit.as_ref() else {
            return;
        };
        let input = state.input.trim().to_string();
        let keys = state.keys.clone();

        match NaiveDate::parse_from_str(&input, "%Y-%m-%d") {
            Ok(date) => {
                let result = self
                    .data
                    .update_tasks_due_date(&keys, date, AppData::today());
                self.run_io(result);
                self.clear_multi_selection();
                self.normalize_main_selection();
                self.normalize_list_task_selection();
            }
            Err(_) => self.push_popup("invalid due date format; use YYYY-MM-DD".to_string()),
        }
    }

    fn commit_multi_task_list_move(&mut self) {
        let Some(state) = self.multi_task_edit.as_ref() else {
            return;
        };

        let suggestions = self.data.list_name_suggestions(&state.input);
        let selected_name = suggestions
            .get(state.suggestion_index)
            .cloned()
            .unwrap_or_else(|| state.input.trim().to_string());
        let keys = state.keys.clone();

        if let Some(list_id) = self.data.find_list_id_by_name(&selected_name) {
            let result = self
                .data
                .move_tasks_to_list(&keys, list_id, AppData::today());
            self.run_io(result);
            self.clear_multi_selection();
            self.normalize_main_selection();
            self.normalize_list_task_selection();
        }
    }

    fn request_multi_task_delete(&mut self) {
        let Some(state) = self.multi_task_edit.as_ref() else {
            return;
        };

        let keys = state.keys.clone();
        self.multi_task_edit = None;
        self.confirm = Some(ConfirmState {
            message: format!("Delete {} selected tasks?", keys.len()),
            action: ConfirmAction::DeleteTasks(keys),
        });
    }

    fn normalize_list_selection(&mut self) {
        let len = self.filtered_task_list_items().len();
        if len == 0 {
            self.selected_list_idx = 0;
        } else {
            self.selected_list_idx = self.selected_list_idx.min(len - 1);
        }
    }

    fn filtered_task_list_items(&self) -> Vec<life_tracking::TaskListViewItem> {
        let mut items = self.data.task_list_items(self.list_sort_mode);
        if self.list_sort_reversed {
            items.reverse();
        }
        let query = self.list_search.trim().to_lowercase();
        if query.is_empty() {
            return items;
        }

        items
            .into_iter()
            .filter(|item| item.name.to_lowercase().contains(&query))
            .collect()
    }

    fn run_io(&mut self, result: io::Result<()>) {
        if let Err(err) = result {
            self.push_popup(format!("save error: {}", err));
        }
    }

    fn push_popup(&mut self, message: String) {
        self.popup_messages.push(message);
        self.show_popup = true;
    }
}

fn shift_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }

    let next = current as isize + delta;
    if next < 0 {
        0
    } else if next >= len as isize {
        len - 1
    } else {
        next as usize
    }
}

fn char_to_byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len())
}

fn insert_char_at_cursor(value: &mut String, cursor_index: &mut usize, ch: char) {
    let byte_index = char_to_byte_index(value, *cursor_index);
    value.insert(byte_index, ch);
    *cursor_index += 1;
}

fn remove_char_before_cursor(value: &mut String, cursor_index: &mut usize) {
    if *cursor_index == 0 {
        return;
    }
    let end = char_to_byte_index(value, *cursor_index);
    let start = char_to_byte_index(value, *cursor_index - 1);
    value.replace_range(start..end, "");
    *cursor_index -= 1;
}

fn remove_char_at_cursor(value: &mut String, cursor_index: usize) {
    let start = char_to_byte_index(value, cursor_index);
    let end = char_to_byte_index(value, cursor_index + 1);
    if start < end {
        value.replace_range(start..end, "");
    }
}

struct DescriptionMatchPreview {
    before: String,
    matched: String,
    after: String,
    has_prefix: bool,
    has_suffix: bool,
}

fn description_match_preview(description: &str, query: &str) -> Option<DescriptionMatchPreview> {
    let needle = query.trim();
    if needle.is_empty() {
        return None;
    }

    let desc_folded = description.to_ascii_lowercase();
    let needle_folded = needle.to_ascii_lowercase();
    let match_start = desc_folded.find(&needle_folded)?;
    let match_end = match_start + needle_folded.len();

    let words = word_byte_ranges(description);
    if words.is_empty() {
        return None;
    }

    let start_word_idx = words
        .iter()
        .position(|(start, end)| match_start < *end && match_end > *start)?;
    let end_word_idx = words
        .iter()
        .rposition(|(start, end)| match_start < *end && match_end > *start)?;

    let context_start_idx = start_word_idx.saturating_sub(3);
    let context_end_idx = (end_word_idx + 3).min(words.len() - 1);
    let snippet_start = words[context_start_idx].0;
    let snippet_end = words[context_end_idx].1;

    let highlight_start = match_start.max(snippet_start);
    let highlight_end = match_end.min(snippet_end);
    if highlight_start >= highlight_end {
        return None;
    }

    Some(DescriptionMatchPreview {
        before: description[snippet_start..highlight_start].to_string(),
        matched: description[highlight_start..highlight_end].to_string(),
        after: description[highlight_end..snippet_end].to_string(),
        has_prefix: context_start_idx > 0,
        has_suffix: context_end_idx + 1 < words.len(),
    })
}

fn word_byte_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut in_word = false;
    let mut start = 0usize;

    for (idx, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if in_word {
                ranges.push((start, idx));
                in_word = false;
            }
        } else if !in_word {
            in_word = true;
            start = idx;
        }
    }

    if in_word {
        ranges.push((start, input.len()));
    }

    ranges
}

fn fit_column(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let trimmed = value.trim();
    let len = trimmed.chars().count();
    if len <= width {
        return format!("{:<width$}", trimmed, width = width);
    }

    if width == 1 {
        return "…".to_string();
    }

    let keep = width - 1;
    let mut short = trimmed.chars().take(keep).collect::<String>();
    short.push('…');
    short
}

fn strip_vim_help_block(content: &str) -> String {
    let mut result = Vec::new();
    let mut skipping = false;

    for line in content.lines() {
        if line == VIM_HELP_START {
            skipping = true;
            continue;
        }
        if line == VIM_HELP_END {
            skipping = false;
            continue;
        }
        if !skipping {
            result.push(line);
        }
    }

    result.join("\n").trim_start_matches('\n').to_string()
}

fn toggle_task_sort_mode(mode: TaskSortMode) -> TaskSortMode {
    match mode {
        TaskSortMode::DueDate => TaskSortMode::TimeLeft,
        TaskSortMode::TimeLeft => TaskSortMode::DueDate,
    }
}

fn toggle_list_sort_mode(mode: ListSortMode) -> ListSortMode {
    match mode {
        ListSortMode::RemainingCount => ListSortMode::Priority,
        ListSortMode::Priority => ListSortMode::RemainingCount,
    }
}

fn task_field_label(field: TaskField) -> &'static str {
    match field {
        TaskField::Title => "Title",
        TaskField::Estimated => "Estimated",
        TaskField::Actual => "Actual",
        TaskField::DueDate => "Due Date",
        TaskField::ListName => "Task List",
    }
}

fn list_field_label(field: ListField) -> &'static str {
    match field {
        ListField::Name => "Task List",
        ListField::Priority => "Priority",
        ListField::Color => "Color",
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn color_from_hex(hex: &str) -> Color {
    parse_hex_color(hex)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .unwrap_or(Color::White)
}

fn current_accent_color() -> Color {
    Color::LightCyan
}

fn due_date_header_item(date: NaiveDate) -> ListItem<'static> {
    let is_today = date == AppData::today();
    let style = if is_today {
        Style::default()
            .fg(current_accent_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    };

    let label = if is_today {
        format!("{} -- TODAY", date)
    } else {
        format!("{}", date)
    };

    ListItem::new(Line::from(Span::styled(label, style)))
}

fn line_with_label(label: &str, value: &str, label_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{}: ", label),
            Style::default()
                .fg(label_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

fn line_with_multiline_label(label: &str, value: &str, label_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{}:\n", label),
            Style::default()
                .fg(label_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

fn empty_if_blank(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "(empty)".to_string()
    } else {
        trimmed.to_string()
    }
}

fn resolve_data_dir() -> PathBuf {
    if let Ok(value) = env::var("LIFE_TRACKING_DATA_DIR") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if let Some(base_dir) = dirs::data_local_dir().or_else(dirs::data_dir) {
        return base_dir.join("life-tracking");
    }

    PathBuf::from(".life-tracking")
}

fn load_notes_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn main() -> io::Result<()> {
    let data_dir = resolve_data_dir();
    let (data, startup_messages) = AppData::load_from_dir(data_dir)?;
    let notes_text = load_notes_text(&data.data_dir.join("notes.txt"));

    let mut app = App::new(data, startup_messages, notes_text);
    app.run()
}
