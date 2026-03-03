use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use life_tracking::{AppData, TaskKey, TaskSortMode};
use tempfile::tempdir;

fn write_fixture(dir: &Path, file_name: &str, contents: &str) {
    fs::write(dir.join(file_name), contents).expect("failed to write fixture");
}

fn task_key_by_names(data: &AppData, list_name: &str, task_title: &str) -> TaskKey {
    let list = data
        .lists
        .iter()
        .find(|list| list.name == list_name)
        .expect("list should exist");
    let task = list
        .tasks
        .iter()
        .find(|task| task.title == task_title)
        .expect("task should exist");
    TaskKey {
        list_id: list.id,
        task_id: task.id,
    }
}

#[test]
fn load_reports_invalid_config_and_keeps_valid_config() {
    let dir = tempdir().expect("failed to create temp dir");
    write_fixture(
        dir.path(),
        "valid.toml",
        include_str!("data/valid_list.toml"),
    );
    write_fixture(
        dir.path(),
        "invalid.toml",
        include_str!("data/invalid_list.toml"),
    );

    let (data, startup_messages) = AppData::load_from_dir(dir.path()).expect("load should succeed");

    assert!(data.lists.iter().any(|list| list.name == "Work"));
    assert!(
        startup_messages
            .iter()
            .any(|message| message.contains("invalid.toml") && message.contains("invalid"))
    );

    let generated_files: Vec<_> = fs::read_dir(dir.path())
        .expect("should read generated dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(|name| name.starts_with("list_") && name.ends_with(".toml"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(generated_files.len(), 1);
}

#[test]
fn sorts_tasks_by_due_date_priority_and_time_left() {
    let dir = tempdir().expect("failed to create temp dir");
    write_fixture(
        dir.path(),
        "alpha.toml",
        include_str!("data/multi_list.toml"),
    );
    write_fixture(
        dir.path(),
        "beta.toml",
        include_str!("data/multi_list_priority.toml"),
    );

    let (data, _) = AppData::load_from_dir(dir.path()).expect("load should succeed");

    let due_items = data.main_task_items("", TaskSortMode::DueDate, false, true);
    assert_eq!(due_items[0].title, "Earlier by priority");
    assert_eq!(due_items[1].title, "Later by priority");

    let time_left_items = data.main_task_items("", TaskSortMode::TimeLeft, false, true);
    assert_eq!(time_left_items[0].title, "High time left");

    let time_left_reversed = data.main_task_items("", TaskSortMode::TimeLeft, true, true);
    assert_eq!(time_left_reversed[0].title, "Low time left");
}

#[test]
fn supports_case_insensitive_list_suggestions() {
    let dir = tempdir().expect("failed to create temp dir");
    write_fixture(
        dir.path(),
        "valid.toml",
        include_str!("data/valid_list.toml"),
    );
    write_fixture(
        dir.path(),
        "alpha.toml",
        include_str!("data/multi_list.toml"),
    );

    let (data, _) = AppData::load_from_dir(dir.path()).expect("load should succeed");

    let suggestions = data.list_name_suggestions("wo");
    assert!(suggestions.iter().any(|value| value == "Work"));

    assert!(data.find_list_id_by_name("work").is_some());
}

#[test]
fn prunes_completed_tasks_older_than_one_week_past_due_on_load() {
    let dir = tempdir().expect("failed to create temp dir");
    write_fixture(
        dir.path(),
        "prune.toml",
        include_str!("data/prune_list.toml"),
    );

    let (data, startup_messages) = AppData::load_from_dir(dir.path()).expect("load should succeed");

    let archive = data
        .lists
        .iter()
        .find(|list| list.name == "Archive")
        .expect("archive list should exist");

    assert_eq!(archive.tasks.len(), 1);
    assert_eq!(archive.tasks[0].title, "Keep this");
    assert!(
        startup_messages
            .iter()
            .any(|message| message.contains("Removed 1"))
    );
}

#[test]
fn toggling_completion_sets_completed_date_and_persists() {
    let dir = tempdir().expect("failed to create temp dir");
    write_fixture(
        dir.path(),
        "valid.toml",
        include_str!("data/valid_list.toml"),
    );

    let (mut data, _) = AppData::load_from_dir(dir.path()).expect("load should succeed");

    let key = task_key_by_names(&data, "Work", "Draft roadmap");

    let today = NaiveDate::from_ymd_opt(2026, 3, 2).expect("valid date");
    data.toggle_task_completed(key, today)
        .expect("toggle completion should succeed");

    let task = data.get_task(key).expect("task should exist");
    assert!(task.completed);
    assert_eq!(task.completed_on, Some(today));

    let file_contents = fs::read_to_string(dir.path().join(format!("list_{}.toml", key.list_id)))
        .expect("generated file should be readable");
    assert!(file_contents.contains("completed = true"));
    assert!(file_contents.contains("completed_on = \"2026-03-02\""));
}

#[test]
fn updating_due_date_persists_changes() {
    let dir = tempdir().expect("failed to create temp dir");
    write_fixture(
        dir.path(),
        "valid.toml",
        include_str!("data/valid_list.toml"),
    );

    let (mut data, _) = AppData::load_from_dir(dir.path()).expect("load should succeed");

    let key = task_key_by_names(&data, "Work", "Draft roadmap");

    let today = NaiveDate::from_ymd_opt(2026, 3, 2).expect("valid date");
    let new_due_date = NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date");

    data.update_task_due_date(key, new_due_date, today)
        .expect("update due date should succeed");

    let task = data.get_task(key).expect("task should exist");
    assert_eq!(task.due_date, new_due_date);

    let file_contents = fs::read_to_string(dir.path().join(format!("list_{}.toml", key.list_id)))
        .expect("generated file should be readable");
    assert!(file_contents.contains("due_date = \"2026-03-10\""));
}
