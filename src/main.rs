use std::env;
use std::fs;
use std::io::{self, Stdout};
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use life_tracking::{AppData, ListSortMode, TaskKey, TaskSortMode, parse_hex_color};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

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

#[derive(Clone, Copy)]
enum ConfirmAction {
    DeleteTask(TaskKey),
    DeleteList(u64),
}

struct ConfirmState {
    message: String,
    action: ConfirmAction,
}

#[derive(Clone, Copy)]
enum TextEditorTarget {
    Notes,
    TaskDescription(TaskKey),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextEditorMode {
    Normal,
    Insert,
}

struct TextEditorState {
    target: TextEditorTarget,
    content: String,
    cursor_index: usize,
    mode: TextEditorMode,
    command_active: bool,
    command: String,
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

    expanded_list_id: Option<u64>,
    expanded_task_key: Option<TaskKey>,
    editor: Option<EditorState>,
    text_editor: Option<TextEditorState>,
    confirm: Option<ConfirmState>,

    popup_messages: Vec<String>,
    show_popup: bool,
    cursor_blink_start: Instant,
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
            expanded_list_id: None,
            expanded_task_key: None,
            editor: None,
            text_editor: None,
            confirm: None,
            popup_messages: startup_messages,
            show_popup,
            cursor_blink_start: Instant::now(),
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
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(frame.area());

        self.draw_controls(frame, layout[0]);

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

        if self.text_editor.is_some() {
            self.draw_text_editor(frame);
        }

        if self.show_popup {
            self.draw_popup(frame);
        }

        if self.confirm.is_some() {
            self.draw_confirm(frame);
        }
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

    fn controls_text(&self) -> (&'static str, String) {
        if let Some(editor) = &self.text_editor {
            if editor.command_active {
                return (
                    "EDIT",
                    ":w save, :wq save+quit, :q quit, esc cancel".to_string(),
                );
            }
            if editor.mode == TextEditorMode::Insert {
                return (
                    "EDIT",
                    "insert mode: type, enter newline, esc normal, ctrl+s save".to_string(),
                );
            }
            return (
                "EDIT",
                "normal mode: i insert, h/j/k/l move, x delete, : command".to_string(),
            );
        }

        if self.confirm.is_some() {
            return ("CONFIRM", "enter/y yes, esc/n no".to_string());
        }

        if self.show_popup {
            return ("POPUP", "enter/esc close".to_string());
        }

        if let Some(editor) = &self.editor {
            if matches!(editor.target, EditTarget::Task(_, TaskField::ListName)) {
                return (
                    "EDIT",
                    "type filter, up/down choose, enter save, esc cancel".to_string(),
                );
            }

            return ("EDIT", "type value, enter save, esc cancel".to_string());
        }

        if self.expanded_task_key.is_some() {
            return (
                "TASK",
                "esc back, t title, d desc, e est, w actual, r due, a done, l move list, del delete"
                    .to_string(),
            );
        }

        if self.expanded_list_id.is_some() {
            let c_text = if self.main_sort_mode == TaskSortMode::TimeLeft {
                "c reverse"
            } else {
                "c hide/show done"
            };
            return (
                "LIST+",
                format!(
                    "esc back, j/k move, enter task, a done, d delete task, n new, t name, p prio, q color, x sort, {}",
                    c_text
                ),
            );
        }

        match self.page {
            Page::Main => {
                let c_text = if self.main_sort_mode == TaskSortMode::TimeLeft {
                    "c reverse"
                } else {
                    "c hide/show done"
                };

                if self.search_active {
                    (
                        "MAIN",
                        format!(
                            "type search, enter finish, esc clear, j/k move, enter open, a done, d delete, n notes, x sort, {}, u lists, q quit",
                            c_text
                        ),
                    )
                } else {
                    (
                        "MAIN",
                        format!(
                            "/ search, esc clear, j/k move, enter open, a done, d delete, n notes, x sort, {}, u lists, q quit",
                            c_text
                        ),
                    )
                }
            }
            Page::TaskLists => (
                "LISTS",
                if self.list_search_active {
                    "type search, enter finish, esc clear, j/k move, enter expand, n new list, d delete list, x sort, c reverse, u main, q quit".to_string()
                } else {
                    "/ search, j/k move, enter expand, n new list, d delete list, x sort, c reverse, u main, q quit".to_string()
                },
            ),
        }
    }

    fn draw_main_page(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);
        let content_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(82), Constraint::Percentage(18)])
            .split(chunks[1]);

        let search_line = if self.search.is_empty() {
            Line::from(vec![
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(" search", Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(Span::raw(self.search.clone()))
        };

        let search_title = "Search";

        frame.render_widget(
            Paragraph::new(vec![search_line])
                .block(Block::default().borders(Borders::ALL).title(search_title)),
            chunks[0],
        );

        if self.search_active && self.cursor_visible() {
            let cursor =
                self.cursor_position_for_input(chunks[0], &self.search, self.search_cursor_index);
            frame.set_cursor_position(cursor);
        }

        let items = self.data.main_task_items(
            &self.search,
            self.main_sort_mode,
            self.time_left_reversed,
            self.show_completed_in_due_mode,
        );

        let (rows, selected_render_idx) =
            self.build_task_rows(&items, self.main_sort_mode, &self.search, true);

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

        let list = List::new(rows)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, content_chunks[0], &mut state);
        self.draw_main_priority_panel(frame, content_chunks[1]);
    }

    fn draw_task_list_page(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);
        let items = self.filtered_task_list_items();

        let search_line = if self.list_search.is_empty() {
            Line::from(vec![
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(" search lists", Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(Span::raw(self.list_search.clone()))
        };
        let search_title = "Search";
        frame.render_widget(
            Paragraph::new(vec![search_line])
                .block(Block::default().borders(Borders::ALL).title(search_title)),
            chunks[0],
        );

        if self.list_search_active && self.cursor_visible() {
            let cursor = self.cursor_position_for_input(
                chunks[0],
                &self.list_search,
                self.list_search_cursor_index,
            );
            frame.set_cursor_position(cursor);
        }

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
            ListSortMode::RemainingCount => format!("Lists * Pending Tasks - {}", order),
            ListSortMode::Priority => format!("Lists * Priority - {}", order),
        };

        let list = List::new(rows)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, chunks[1], &mut state);
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
            line_with_label("Title", &empty_if_blank(&task.title), Color::LightBlue),
            line_with_label(
                "Description",
                &empty_if_blank(&task.description),
                Color::LightBlue,
            ),
            line_with_label("Task List", &list_name, list_color),
            line_with_label(
                "Estimated",
                &format!("{} hours", task.estimated_minutes),
                Color::LightBlue,
            ),
            line_with_label(
                "Actual",
                &format!("{} hours", task.actual_minutes),
                Color::LightBlue,
            ),
            line_with_label("Due Date", &format!("{}", task.due_date), Color::LightBlue),
            line_with_label("Completed", &completion, Color::LightBlue),
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
            .block(Block::default().borders(Borders::ALL).title(title))
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
            Line::from(Span::styled(
                "Confirm Delete",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(confirm.message.clone()),
            Line::from(""),
            Line::from("Press Enter/Y to delete, Esc/N to cancel."),
        ];

        let popup = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Confirm"))
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
                    let row = format!(
                        "{:>2}. {} | P{:>2}",
                        idx + 1,
                        fit_column(&item.name, 16),
                        item.priority
                    );
                    ListItem::new(Line::from(Span::styled(
                        row,
                        Style::default().fg(color_from_hex(&item.color_hex)),
                    )))
                })
                .collect()
        };

        let panel =
            List::new(rows).block(Block::default().borders(Borders::ALL).title("Task Lists"));
        frame.render_widget(panel, panels[0]);

        let notes_content = if self.notes_text.trim().is_empty() {
            "(empty)".to_string()
        } else {
            self.notes_text.clone()
        };
        let notes = Paragraph::new(notes_content)
            .block(Block::default().borders(Borders::ALL).title("Notes"))
            .wrap(Wrap { trim: false });
        frame.render_widget(notes, panels[1]);
    }

    fn draw_text_editor(&self, frame: &mut Frame) {
        let Some(editor) = &self.text_editor else {
            return;
        };

        let area = centered_rect(92, 88, frame.area());
        frame.render_widget(Clear, area);

        let title = match editor.target {
            TextEditorTarget::Notes => "Notes Editor",
            TextEditorTarget::TaskDescription(_) => "Description Editor",
        };
        let mode = if editor.command_active {
            "COMMAND"
        } else if editor.mode == TextEditorMode::Insert {
            "INSERT"
        } else {
            "NORMAL"
        };

        let outer = Block::default()
            .borders(Borders::ALL)
            .title(format!("{} [{}]", title, mode));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(inner);

        let text = Paragraph::new(editor.content.clone()).wrap(Wrap { trim: false });
        frame.render_widget(text, chunks[0]);

        let command_text = if editor.command_active {
            format!(":{}", editor.command)
        } else if editor.mode == TextEditorMode::Insert {
            "-- INSERT --".to_string()
        } else {
            "-- NORMAL --".to_string()
        };
        frame.render_widget(
            Paragraph::new(command_text).block(Block::default().borders(Borders::ALL).title("Cmd")),
            chunks[1],
        );

        if self.cursor_visible() {
            let (line, col) = cursor_line_col(&editor.content, editor.cursor_index);
            let x = chunks[0]
                .x
                .saturating_add((col as u16).min(chunks[0].width.saturating_sub(1)));
            let y = chunks[0]
                .y
                .saturating_add((line as u16).min(chunks[0].height.saturating_sub(1)));
            frame.set_cursor_position((x, y));
        }
    }

    fn build_task_rows(
        &self,
        items: &[life_tracking::TaskViewItem],
        sort_mode: TaskSortMode,
        search_query: &str,
        show_description_match_preview: bool,
    ) -> (Vec<ListItem<'static>>, Option<usize>) {
        if items.is_empty() {
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

        for item in items {
            if sort_mode == TaskSortMode::DueDate && previous_due != Some(item.due_date) {
                rows.push(ListItem::new(Line::from(Span::styled(
                    format!("{}", item.due_date),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))));
                previous_due = Some(item.due_date);
            }

            let completed_marker = if item.completed { "[x]" } else { "[ ]" };
            let title = empty_if_blank(&item.title);
            let list_color = color_from_hex(&item.list_color_hex);
            let base_style = Style::default().fg(list_color);

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

        let selected_render_idx = task_to_render_idx.get(selected_task_idx).copied();
        (rows, selected_render_idx)
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.text_editor.is_some() {
            self.handle_text_editor_key(key);
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
            }
            KeyCode::Char('u') => self.page = Page::TaskLists,
            KeyCode::Char('n') => self.open_notes_editor(),
            KeyCode::Char('x') => {
                self.main_sort_mode = toggle_task_sort_mode(self.main_sort_mode);
                self.selected_main_idx = 0;
            }
            KeyCode::Char('c') => {
                if self.main_sort_mode == TaskSortMode::TimeLeft {
                    self.time_left_reversed = !self.time_left_reversed;
                } else {
                    self.show_completed_in_due_mode = !self.show_completed_in_due_mode;
                }
                self.selected_main_idx = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_main_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_main_selection(-1),
            KeyCode::Enter => {
                let items = self.data.main_task_items(
                    &self.search,
                    self.main_sort_mode,
                    self.time_left_reversed,
                    self.show_completed_in_due_mode,
                );
                if let Some(item) = items.get(self.selected_main_idx) {
                    self.expanded_task_key = Some(item.key);
                }
            }
            KeyCode::Char('a') => {
                let items = self.data.main_task_items(
                    &self.search,
                    self.main_sort_mode,
                    self.time_left_reversed,
                    self.show_completed_in_due_mode,
                );
                if let Some(item) = items.get(self.selected_main_idx) {
                    let today = AppData::today();
                    let result = self.data.toggle_task_completed(item.key, today);
                    self.run_io(result);
                    self.normalize_main_selection();
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                let items = self.data.main_task_items(
                    &self.search,
                    self.main_sort_mode,
                    self.time_left_reversed,
                    self.show_completed_in_due_mode,
                );
                if let Some(item) = items.get(self.selected_main_idx) {
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
            KeyCode::Char('u') => self.page = Page::Main,
            KeyCode::Char('/') => {
                self.list_search_active = true;
                self.list_search_cursor_index = self.list_search.chars().count();
            }
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
            KeyCode::Char('d') | KeyCode::Delete => {
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
            }
            KeyCode::Char('c') => {
                if self.main_sort_mode == TaskSortMode::TimeLeft {
                    self.time_left_reversed = !self.time_left_reversed;
                } else {
                    self.show_completed_in_due_mode = !self.show_completed_in_due_mode;
                }
                self.selected_list_task_idx = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_list_task_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_list_task_selection(-1),
            KeyCode::Enter => {
                let items = self.data.list_task_items(
                    list_id,
                    "",
                    self.main_sort_mode,
                    self.time_left_reversed,
                    self.show_completed_in_due_mode,
                );
                if let Some(item) = items.get(self.selected_list_task_idx) {
                    self.expanded_task_key = Some(item.key);
                }
            }
            KeyCode::Char('a') => {
                let items = self.data.list_task_items(
                    list_id,
                    "",
                    self.main_sort_mode,
                    self.time_left_reversed,
                    self.show_completed_in_due_mode,
                );
                if let Some(item) = items.get(self.selected_list_task_idx) {
                    let today = AppData::today();
                    let result = self.data.toggle_task_completed(item.key, today);
                    self.run_io(result);
                    self.normalize_list_task_selection();
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                let items = self.data.list_task_items(
                    list_id,
                    "",
                    self.main_sort_mode,
                    self.time_left_reversed,
                    self.show_completed_in_due_mode,
                );
                if let Some(item) = items.get(self.selected_list_task_idx) {
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

    fn handle_text_editor_key(&mut self, key: KeyEvent) {
        let Some(editor) = &mut self.text_editor else {
            return;
        };

        if editor.command_active {
            match key.code {
                KeyCode::Esc => {
                    editor.command_active = false;
                    editor.command.clear();
                }
                KeyCode::Enter => {
                    let command = editor.command.trim().to_string();
                    editor.command_active = false;
                    editor.command.clear();
                    self.execute_text_editor_command(&command);
                }
                KeyCode::Backspace => {
                    editor.command.pop();
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        editor.command.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        if editor.mode == TextEditorMode::Insert {
            match key.code {
                KeyCode::Esc => editor.mode = TextEditorMode::Normal,
                KeyCode::Enter => {
                    insert_char_at_cursor(&mut editor.content, &mut editor.cursor_index, '\n')
                }
                KeyCode::Backspace => {
                    if editor.cursor_index > 0 {
                        remove_char_before_cursor(&mut editor.content, &mut editor.cursor_index);
                    }
                }
                KeyCode::Delete => remove_char_at_cursor(&mut editor.content, editor.cursor_index),
                KeyCode::Left => editor.cursor_index = editor.cursor_index.saturating_sub(1),
                KeyCode::Right => {
                    editor.cursor_index =
                        (editor.cursor_index + 1).min(editor.content.chars().count())
                }
                KeyCode::Up => move_cursor_vertical(&editor.content, &mut editor.cursor_index, -1),
                KeyCode::Down => move_cursor_vertical(&editor.content, &mut editor.cursor_index, 1),
                KeyCode::Home => {
                    let (line, _) = cursor_line_col(&editor.content, editor.cursor_index);
                    editor.cursor_index = index_for_line_col(&editor.content, line, 0);
                }
                KeyCode::End => {
                    let (line, _) = cursor_line_col(&editor.content, editor.cursor_index);
                    let line_len = line_length(&editor.content, line);
                    editor.cursor_index = index_for_line_col(&editor.content, line, line_len);
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.save_text_editor(false);
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        insert_char_at_cursor(&mut editor.content, &mut editor.cursor_index, c);
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('i') => editor.mode = TextEditorMode::Insert,
            KeyCode::Char('a') => {
                editor.cursor_index = (editor.cursor_index + 1).min(editor.content.chars().count());
                editor.mode = TextEditorMode::Insert;
            }
            KeyCode::Char('h') | KeyCode::Left => {
                editor.cursor_index = editor.cursor_index.saturating_sub(1)
            }
            KeyCode::Char('l') | KeyCode::Right => {
                editor.cursor_index = (editor.cursor_index + 1).min(editor.content.chars().count())
            }
            KeyCode::Char('k') | KeyCode::Up => {
                move_cursor_vertical(&editor.content, &mut editor.cursor_index, -1)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                move_cursor_vertical(&editor.content, &mut editor.cursor_index, 1)
            }
            KeyCode::Char('0') | KeyCode::Home => {
                let (line, _) = cursor_line_col(&editor.content, editor.cursor_index);
                editor.cursor_index = index_for_line_col(&editor.content, line, 0);
            }
            KeyCode::Char('$') | KeyCode::End => {
                let (line, _) = cursor_line_col(&editor.content, editor.cursor_index);
                let line_len = line_length(&editor.content, line);
                editor.cursor_index = index_for_line_col(&editor.content, line, line_len);
            }
            KeyCode::Char('x') => remove_char_at_cursor(&mut editor.content, editor.cursor_index),
            KeyCode::Char('o') => {
                let (line, _) = cursor_line_col(&editor.content, editor.cursor_index);
                let line_len = line_length(&editor.content, line);
                editor.cursor_index = index_for_line_col(&editor.content, line, line_len);
                insert_char_at_cursor(&mut editor.content, &mut editor.cursor_index, '\n');
                editor.mode = TextEditorMode::Insert;
            }
            KeyCode::Char('O') => {
                let (line, _) = cursor_line_col(&editor.content, editor.cursor_index);
                editor.cursor_index = index_for_line_col(&editor.content, line, 0);
                insert_char_at_cursor(&mut editor.content, &mut editor.cursor_index, '\n');
                editor.cursor_index = editor.cursor_index.saturating_sub(1);
                editor.mode = TextEditorMode::Insert;
            }
            KeyCode::Char(':') => {
                editor.command_active = true;
                editor.command.clear();
            }
            _ => {}
        }
    }

    fn execute_text_editor_command(&mut self, command: &str) {
        match command {
            "w" => self.save_text_editor(false),
            "wq" => self.save_text_editor(true),
            "q" => self.text_editor = None,
            _ => self.push_popup(format!("unknown command: :{}", command)),
        }
    }

    fn save_text_editor(&mut self, close_after: bool) {
        let Some((target, content)) = self
            .text_editor
            .as_ref()
            .map(|editor| (editor.target, editor.content.clone()))
        else {
            return;
        };

        match target {
            TextEditorTarget::Notes => {
                let path = self.data.data_dir.join("notes.txt");
                if let Err(err) = fs::write(&path, &content) {
                    self.push_popup(format!("failed to save notes: {}", err));
                    return;
                }
                self.notes_text = content;
            }
            TextEditorTarget::TaskDescription(task_key) => {
                let result = self
                    .data
                    .update_task_description(task_key, content, AppData::today());
                self.run_io(result);
            }
        }

        if close_after {
            self.text_editor = None;
        }
    }

    fn open_notes_editor(&mut self) {
        self.text_editor = Some(TextEditorState {
            target: TextEditorTarget::Notes,
            content: self.notes_text.clone(),
            cursor_index: self.notes_text.chars().count(),
            mode: TextEditorMode::Normal,
            command_active: false,
            command: String::new(),
        });
    }

    fn open_task_description_editor(&mut self, task_key: TaskKey) {
        let Some(task) = self.data.get_task(task_key) else {
            return;
        };
        self.text_editor = Some(TextEditorState {
            target: TextEditorTarget::TaskDescription(task_key),
            content: task.description.clone(),
            cursor_index: task.description.chars().count(),
            mode: TextEditorMode::Normal,
            command_active: false,
            command: String::new(),
        });
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
        let items = self.data.main_task_items(
            &self.search,
            self.main_sort_mode,
            self.time_left_reversed,
            self.show_completed_in_due_mode,
        );
        self.selected_main_idx = shift_index(self.selected_main_idx, items.len(), delta);
    }

    fn move_list_selection(&mut self, delta: isize) {
        let items = self.filtered_task_list_items();
        self.selected_list_idx = shift_index(self.selected_list_idx, items.len(), delta);
    }

    fn move_list_task_selection(&mut self, delta: isize) {
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
        let Some(action) = self.confirm.as_ref().map(|confirm| confirm.action) else {
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

fn cursor_line_col(text: &str, cursor_index: usize) -> (usize, usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut remaining = cursor_index.min(text.chars().count());

    for (line_idx, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        if remaining <= len {
            return (line_idx, remaining);
        }
        remaining = remaining.saturating_sub(len + 1);
    }

    let last_idx = lines.len().saturating_sub(1);
    let last_len = lines
        .get(last_idx)
        .map(|line| line.chars().count())
        .unwrap_or(0);
    (last_idx, last_len)
}

fn index_for_line_col(text: &str, line: usize, col: usize) -> usize {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return 0;
    }

    let target_line = line.min(lines.len() - 1);
    let mut index = 0usize;
    for entry in lines.iter().take(target_line) {
        index += entry.chars().count() + 1;
    }
    index + col.min(lines[target_line].chars().count())
}

fn line_length(text: &str, line: usize) -> usize {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return 0;
    }
    let target_line = line.min(lines.len() - 1);
    lines[target_line].chars().count()
}

fn move_cursor_vertical(text: &str, cursor_index: &mut usize, delta: isize) {
    let (line, col) = cursor_line_col(text, *cursor_index);
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        *cursor_index = 0;
        return;
    }

    let next_line = if delta < 0 {
        line.saturating_sub(delta.unsigned_abs())
    } else {
        (line + delta as usize).min(lines.len() - 1)
    };
    *cursor_index = index_for_line_col(text, next_line, col);
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
