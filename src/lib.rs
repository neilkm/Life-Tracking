use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

const GENERATED_FILE_PREFIX: &str = "list_";
const GENERATED_FILE_SUFFIX: &str = ".toml";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    #[serde(default, skip_serializing)]
    pub id: u64,
    pub title: String,
    pub description: String,
    pub due_date: NaiveDate,
    #[serde(rename = "estimated_hours", alias = "estimated_minutes")]
    pub estimated_minutes: u32,
    #[serde(rename = "actual_hours", alias = "actual_minutes")]
    pub actual_minutes: u32,
    pub completed: bool,
    #[serde(default)]
    pub completed_on: Option<NaiveDate>,
}

impl Task {
    pub fn time_left(&self) -> i32 {
        self.estimated_minutes as i32 - self.actual_minutes as i32
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskList {
    #[serde(default, skip_serializing)]
    pub id: u64,
    pub name: String,
    pub priority: u8,
    pub color_hex: String,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskKey {
    pub list_id: u64,
    pub task_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskSortMode {
    DueDate,
    TimeLeft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListSortMode {
    RemainingCount,
    Priority,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskViewItem {
    pub key: TaskKey,
    pub list_name: String,
    pub list_priority: u8,
    pub list_color_hex: String,
    pub title: String,
    pub description: String,
    pub due_date: NaiveDate,
    pub estimated_minutes: u32,
    pub actual_minutes: u32,
    pub completed: bool,
    pub time_left: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskListViewItem {
    pub list_id: u64,
    pub name: String,
    pub priority: u8,
    pub color_hex: String,
    pub total_items: usize,
    pub remaining_items: usize,
}

#[derive(Clone, Debug)]
pub struct AppData {
    pub lists: Vec<TaskList>,
    pub data_dir: PathBuf,
    next_list_id: u64,
    next_task_id: u64,
}

impl AppData {
    pub fn load_from_dir(path: impl AsRef<Path>) -> io::Result<(Self, Vec<String>)> {
        let data_dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;

        let mut startup_messages = Vec::new();
        let mut lists = Vec::new();

        let mut files: Vec<PathBuf> = fs::read_dir(&data_dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        files.sort();

        for path in files {
            let filename = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("<unknown>")
                .to_string();

            match fs::read_to_string(&path) {
                Ok(content) => match toml::from_str::<TaskList>(&content) {
                    Ok(list) => match validate_task_list(&list) {
                        Ok(()) => lists.push(list),
                        Err(message) => startup_messages
                            .push(format!("Config '{}' invalid: {}", filename, message)),
                    },
                    Err(err) => startup_messages
                        .push(format!("Config '{}' has invalid TOML: {}", filename, err)),
                },
                Err(err) => startup_messages
                    .push(format!("Config '{}' could not be read: {}", filename, err)),
            }
        }

        if lists.is_empty() {
            lists = seeded_lists(Local::now().date_naive());
        }

        let mut app_data = Self {
            lists,
            data_dir,
            next_list_id: 1,
            next_task_id: 1,
        };

        app_data.resolve_duplicate_ids(&mut startup_messages);
        app_data.refresh_next_ids();

        let pruned = app_data.prune_completed_overdue(Local::now().date_naive());
        if pruned > 0 {
            startup_messages.push(format!(
                "Removed {} completed task(s) that were over one week past due date.",
                pruned
            ));
        }

        app_data.save_all()?;

        Ok((app_data, startup_messages))
    }

    pub fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    pub fn save_all(&self) -> io::Result<()> {
        fs::create_dir_all(&self.data_dir)?;

        let mut keep_files: HashSet<String> = HashSet::new();

        for list in &self.lists {
            let name = generated_list_file_name(list.id);
            let path = self.data_dir.join(&name);
            let serialized = toml::to_string_pretty(list).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to serialize task list '{}': {}", list.name, err),
                )
            })?;
            fs::write(path, serialized)?;
            keep_files.insert(name);
        }

        for entry in fs::read_dir(&self.data_dir)? {
            let entry = match entry {
                Ok(value) => value,
                Err(_) => continue,
            };
            let path = entry.path();
            let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };

            let generated = filename.starts_with(GENERATED_FILE_PREFIX)
                && filename.ends_with(GENERATED_FILE_SUFFIX);

            if generated && !keep_files.contains(filename) {
                let _ = fs::remove_file(path);
            }
        }

        Ok(())
    }

    pub fn list_name_suggestions(&self, input: &str) -> Vec<String> {
        let needle = input.trim().to_lowercase();
        let mut names: Vec<String> = self
            .lists
            .iter()
            .filter(|list| needle.is_empty() || list.name.to_lowercase().contains(&needle))
            .map(|list| list.name.clone())
            .collect();
        names.sort();
        names
    }

    pub fn find_list_id_by_name(&self, name: &str) -> Option<u64> {
        self.lists
            .iter()
            .find(|list| list.name.eq_ignore_ascii_case(name))
            .map(|list| list.id)
    }

    pub fn task_list_items(&self, sort_mode: ListSortMode) -> Vec<TaskListViewItem> {
        let mut items: Vec<TaskListViewItem> = self
            .lists
            .iter()
            .map(|list| {
                let total_items = list.tasks.len();
                let remaining_items = list.tasks.iter().filter(|task| !task.completed).count();
                TaskListViewItem {
                    list_id: list.id,
                    name: list.name.clone(),
                    priority: list.priority,
                    color_hex: list.color_hex.clone(),
                    total_items,
                    remaining_items,
                }
            })
            .collect();

        match sort_mode {
            ListSortMode::RemainingCount => {
                items.sort_by(|a, b| {
                    b.remaining_items
                        .cmp(&a.remaining_items)
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
            ListSortMode::Priority => {
                items.sort_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
            }
        }

        items
    }

    pub fn main_task_items(
        &self,
        query: &str,
        sort_mode: TaskSortMode,
        time_left_reversed: bool,
        show_completed_in_due_mode: bool,
    ) -> Vec<TaskViewItem> {
        self.task_items_for_lists(
            self.lists.iter().map(|list| list.id).collect(),
            query,
            sort_mode,
            time_left_reversed,
            show_completed_in_due_mode,
        )
    }

    pub fn list_task_items(
        &self,
        list_id: u64,
        query: &str,
        sort_mode: TaskSortMode,
        time_left_reversed: bool,
        show_completed_in_due_mode: bool,
    ) -> Vec<TaskViewItem> {
        self.task_items_for_lists(
            vec![list_id],
            query,
            sort_mode,
            time_left_reversed,
            show_completed_in_due_mode,
        )
    }

    pub fn get_task(&self, key: TaskKey) -> Option<&Task> {
        self.lists
            .iter()
            .find(|list| list.id == key.list_id)
            .and_then(|list| list.tasks.iter().find(|task| task.id == key.task_id))
    }

    pub fn get_list(&self, list_id: u64) -> Option<&TaskList> {
        self.lists.iter().find(|list| list.id == list_id)
    }

    pub fn toggle_task_completed(&mut self, key: TaskKey, today: NaiveDate) -> io::Result<()> {
        let Some(task) = self.get_task_mut(key) else {
            return Ok(());
        };

        task.completed = !task.completed;
        task.completed_on = if task.completed { Some(today) } else { None };

        self.persist_after_edit(today)
    }

    pub fn update_task_title(
        &mut self,
        key: TaskKey,
        title: String,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(task) = self.get_task_mut(key) else {
            return Ok(());
        };

        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        task.title = trimmed.to_string();
        self.persist_after_edit(today)
    }

    pub fn update_task_description(
        &mut self,
        key: TaskKey,
        description: String,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(task) = self.get_task_mut(key) else {
            return Ok(());
        };

        task.description = description;
        self.persist_after_edit(today)
    }

    pub fn update_task_estimated(
        &mut self,
        key: TaskKey,
        estimated_minutes: u32,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(task) = self.get_task_mut(key) else {
            return Ok(());
        };

        task.estimated_minutes = estimated_minutes;
        self.persist_after_edit(today)
    }

    pub fn update_task_actual(
        &mut self,
        key: TaskKey,
        actual_minutes: u32,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(task) = self.get_task_mut(key) else {
            return Ok(());
        };

        task.actual_minutes = actual_minutes;
        self.persist_after_edit(today)
    }

    pub fn update_task_due_date(
        &mut self,
        key: TaskKey,
        due_date: NaiveDate,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(task) = self.get_task_mut(key) else {
            return Ok(());
        };

        task.due_date = due_date;
        self.persist_after_edit(today)
    }

    pub fn move_task_to_list(
        &mut self,
        key: TaskKey,
        target_list_id: u64,
        today: NaiveDate,
    ) -> io::Result<()> {
        if key.list_id == target_list_id {
            return Ok(());
        }

        let Some(source_idx) = self.lists.iter().position(|list| list.id == key.list_id) else {
            return Ok(());
        };
        let Some(mut target_idx) = self.lists.iter().position(|list| list.id == target_list_id)
        else {
            return Ok(());
        };

        let Some(task_idx) = self.lists[source_idx]
            .tasks
            .iter()
            .position(|task| task.id == key.task_id)
        else {
            return Ok(());
        };

        let task = self.lists[source_idx].tasks.remove(task_idx);

        if source_idx < target_idx {
            target_idx -= 1;
        }

        self.lists[target_idx].tasks.push(task);
        self.persist_after_edit(today)
    }

    pub fn create_task_in_list(
        &mut self,
        list_id: u64,
        today: NaiveDate,
    ) -> io::Result<Option<TaskKey>> {
        let Some(list) = self.lists.iter_mut().find(|value| value.id == list_id) else {
            return Ok(None);
        };

        let task_id = self.next_task_id;
        self.next_task_id += 1;

        list.tasks.push(Task {
            id: task_id,
            title: String::new(),
            description: String::new(),
            due_date: today,
            estimated_minutes: 0,
            actual_minutes: 0,
            completed: false,
            completed_on: None,
        });

        self.persist_after_edit(today)?;

        Ok(Some(TaskKey { list_id, task_id }))
    }

    pub fn create_task_list(&mut self, today: NaiveDate) -> io::Result<u64> {
        let list_id = self.next_list_id;
        self.next_list_id += 1;

        self.lists.push(TaskList {
            id: list_id,
            name: "New List".to_string(),
            priority: 1,
            color_hex: "#4DA3FF".to_string(),
            tasks: Vec::new(),
        });

        self.persist_after_edit(today)?;
        Ok(list_id)
    }

    pub fn delete_task(&mut self, key: TaskKey, today: NaiveDate) -> io::Result<()> {
        let Some(list) = self.lists.iter_mut().find(|list| list.id == key.list_id) else {
            return Ok(());
        };

        let before = list.tasks.len();
        list.tasks.retain(|task| task.id != key.task_id);
        if list.tasks.len() == before {
            return Ok(());
        }

        self.persist_after_edit(today)
    }

    pub fn delete_task_list(&mut self, list_id: u64, today: NaiveDate) -> io::Result<()> {
        let before = self.lists.len();
        self.lists.retain(|list| list.id != list_id);
        if self.lists.len() == before {
            return Ok(());
        }

        self.persist_after_edit(today)
    }

    pub fn update_list_name(
        &mut self,
        list_id: u64,
        name: String,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(list) = self.lists.iter_mut().find(|value| value.id == list_id) else {
            return Ok(());
        };

        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        list.name = trimmed.to_string();
        self.persist_after_edit(today)
    }

    pub fn update_list_priority(
        &mut self,
        list_id: u64,
        priority: u8,
        today: NaiveDate,
    ) -> io::Result<()> {
        let Some(list) = self.lists.iter_mut().find(|value| value.id == list_id) else {
            return Ok(());
        };

        list.priority = priority;
        self.persist_after_edit(today)
    }

    pub fn update_list_color(
        &mut self,
        list_id: u64,
        color_hex: String,
        today: NaiveDate,
    ) -> io::Result<()> {
        if parse_hex_color(&color_hex).is_none() {
            return Ok(());
        }

        let Some(list) = self.lists.iter_mut().find(|value| value.id == list_id) else {
            return Ok(());
        };

        list.color_hex = normalize_hex_color(&color_hex);
        self.persist_after_edit(today)
    }

    pub fn prune_completed_overdue(&mut self, today: NaiveDate) -> usize {
        let mut removed = 0usize;

        for list in &mut self.lists {
            let before = list.tasks.len();
            list.tasks.retain(|task| {
                if !task.completed {
                    return true;
                }

                let Some(remove_after) = task.due_date.checked_add_signed(Duration::days(7)) else {
                    return true;
                };

                today < remove_after
            });
            removed += before.saturating_sub(list.tasks.len());
        }

        removed
    }

    fn task_items_for_lists(
        &self,
        list_ids: Vec<u64>,
        query: &str,
        sort_mode: TaskSortMode,
        time_left_reversed: bool,
        show_completed_in_due_mode: bool,
    ) -> Vec<TaskViewItem> {
        let query_lc = query.trim().to_lowercase();
        let include_completed = sort_mode != TaskSortMode::DueDate || show_completed_in_due_mode;

        let mut items = Vec::new();

        for list in self.lists.iter().filter(|list| list_ids.contains(&list.id)) {
            for task in &list.tasks {
                if !include_completed && task.completed {
                    continue;
                }

                let due_text = task.due_date.to_string();
                let matches_query = query_lc.is_empty()
                    || task.title.to_lowercase().contains(&query_lc)
                    || task.description.to_lowercase().contains(&query_lc)
                    || list.name.to_lowercase().contains(&query_lc)
                    || due_text.contains(&query_lc);

                if !matches_query {
                    continue;
                }

                items.push(TaskViewItem {
                    key: TaskKey {
                        list_id: list.id,
                        task_id: task.id,
                    },
                    list_name: list.name.clone(),
                    list_priority: list.priority,
                    list_color_hex: list.color_hex.clone(),
                    title: task.title.clone(),
                    description: task.description.clone(),
                    due_date: task.due_date,
                    estimated_minutes: task.estimated_minutes,
                    actual_minutes: task.actual_minutes,
                    completed: task.completed,
                    time_left: task.time_left(),
                });
            }
        }

        match sort_mode {
            TaskSortMode::DueDate => {
                items.sort_by(|a, b| {
                    a.due_date
                        .cmp(&b.due_date)
                        .then(a.list_priority.cmp(&b.list_priority))
                        .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
                });
            }
            TaskSortMode::TimeLeft => {
                items.sort_by(|a, b| {
                    let base = b
                        .time_left
                        .cmp(&a.time_left)
                        .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()));
                    if time_left_reversed {
                        reverse_ordering(base)
                    } else {
                        base
                    }
                });
            }
        }

        items
    }

    fn persist_after_edit(&mut self, today: NaiveDate) -> io::Result<()> {
        self.prune_completed_overdue(today);
        self.save_all()
    }

    fn resolve_duplicate_ids(&mut self, startup_messages: &mut Vec<String>) {
        let mut used_list_ids: HashSet<u64> = HashSet::new();
        let mut next_list_id = self.lists.iter().map(|list| list.id).max().unwrap_or(0) + 1;

        for list in &mut self.lists {
            if list.id == 0 || !used_list_ids.insert(list.id) {
                let old = list.id;
                while next_list_id == 0 || used_list_ids.contains(&next_list_id) {
                    next_list_id += 1;
                }
                list.id = next_list_id;
                next_list_id += 1;
                if old != 0 {
                    startup_messages.push(format!(
                        "Duplicate task list id {} detected; reassigned '{}' to id {}.",
                        old, list.name, list.id
                    ));
                }
                used_list_ids.insert(list.id);
            }

            let mut used_task_ids: HashSet<u64> = HashSet::new();
            let mut next_task_id = list.tasks.iter().map(|task| task.id).max().unwrap_or(0) + 1;

            for task in &mut list.tasks {
                if task.id == 0 || !used_task_ids.insert(task.id) {
                    let old = task.id;
                    while next_task_id == 0 || used_task_ids.contains(&next_task_id) {
                        next_task_id += 1;
                    }
                    task.id = next_task_id;
                    next_task_id += 1;
                    if old != 0 {
                        startup_messages.push(format!(
                            "Duplicate task id {} detected in list '{}'; reassigned '{}' to id {}.",
                            old, list.name, task.title, task.id
                        ));
                    }
                    used_task_ids.insert(task.id);
                }
            }
        }
    }

    fn refresh_next_ids(&mut self) {
        self.next_list_id = self.lists.iter().map(|list| list.id).max().unwrap_or(0) + 1;
        self.next_task_id = self
            .lists
            .iter()
            .flat_map(|list| list.tasks.iter().map(|task| task.id))
            .max()
            .unwrap_or(0)
            + 1;
    }

    fn get_task_mut(&mut self, key: TaskKey) -> Option<&mut Task> {
        self.lists
            .iter_mut()
            .find(|list| list.id == key.list_id)
            .and_then(|list| list.tasks.iter_mut().find(|task| task.id == key.task_id))
    }
}

pub fn parse_hex_color(input: &str) -> Option<(u8, u8, u8)> {
    let normalized = normalize_hex_color(input);
    let stripped = normalized.strip_prefix('#')?;

    if stripped.len() != 6 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let r = u8::from_str_radix(&stripped[0..2], 16).ok()?;
    let g = u8::from_str_radix(&stripped[2..4], 16).ok()?;
    let b = u8::from_str_radix(&stripped[4..6], 16).ok()?;

    Some((r, g, b))
}

pub fn normalize_hex_color(input: &str) -> String {
    let mut cleaned = input.trim().to_string();
    if !cleaned.starts_with('#') {
        cleaned.insert(0, '#');
    }
    cleaned.make_ascii_uppercase();
    cleaned
}

fn generated_list_file_name(list_id: u64) -> String {
    format!(
        "{}{}{}",
        GENERATED_FILE_PREFIX, list_id, GENERATED_FILE_SUFFIX
    )
}

fn validate_task_list(list: &TaskList) -> Result<(), String> {
    if list.name.trim().is_empty() {
        return Err("task list name must not be empty".to_string());
    }

    if parse_hex_color(&list.color_hex).is_none() {
        return Err("task list color_hex must be a valid #RRGGBB value".to_string());
    }

    Ok(())
}

fn reverse_ordering(ordering: Ordering) -> Ordering {
    match ordering {
        Ordering::Less => Ordering::Greater,
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => Ordering::Less,
    }
}

fn seeded_lists(today: NaiveDate) -> Vec<TaskList> {
    vec![
        TaskList {
            id: 1,
            name: "Work".to_string(),
            priority: 1,
            color_hex: "#4DA3FF".to_string(),
            tasks: vec![Task {
                id: 1,
                title: "Plan sprint goals".to_string(),
                description: "Draft sprint objective notes for the team sync.".to_string(),
                due_date: today,
                estimated_minutes: 90,
                actual_minutes: 20,
                completed: false,
                completed_on: None,
            }],
        },
        TaskList {
            id: 2,
            name: "Health".to_string(),
            priority: 2,
            color_hex: "#44C777".to_string(),
            tasks: vec![Task {
                id: 2,
                title: "30 minute run".to_string(),
                description: "Keep a moderate pace and log heart rate.".to_string(),
                due_date: today.checked_add_signed(Duration::days(1)).unwrap_or(today),
                estimated_minutes: 30,
                actual_minutes: 0,
                completed: false,
                completed_on: None,
            }],
        },
        TaskList {
            id: 3,
            name: "Home".to_string(),
            priority: 3,
            color_hex: "#D67AFF".to_string(),
            tasks: vec![Task {
                id: 3,
                title: "Fix cabinet hinge".to_string(),
                description: "Tighten screws and realign cabinet door.".to_string(),
                due_date: today.checked_add_signed(Duration::days(2)).unwrap_or(today),
                estimated_minutes: 20,
                actual_minutes: 0,
                completed: false,
                completed_on: None,
            }],
        },
    ]
}
