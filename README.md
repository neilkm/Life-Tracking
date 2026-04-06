# Life-Tracking

Terminal task manager with multiple task lists, priorities, due dates, time tracking, inline editing, batch task operations, and Vim-backed notes/description editing.

## Requirements

- Rust toolchain (stable)
- A terminal window (the app runs as full-screen TUI in the alternate screen)

## Quick Start

1. Install the app:
```bash
cargo install --path .
```
2. Run it from anywhere:
```bash
life-tracking
```

Installed executable location:

- Cargo installs the binary into Cargo's bin directory.
- Typical path on macOS/Linux: `~/.cargo/bin/life-tracking`
- Typical path on Windows: `%USERPROFILE%\.cargo\bin\life-tracking.exe`

For local development:

```bash
cargo build
./scripts/run-app.sh
```

## Running Tests

```bash
./scripts/run-tests.sh
```

This runs the integration tests in `tests/` and validates sorting, persistence, suggestions, config validation, and pruning rules.

## App Layout

The app has two main pages:

1. Main page (combined tasks from all lists)
2. Task list page (all task lists)

It also has:

1. Task expanded view
2. Task list expanded view
3. Multi-task edit popup
4. Confirm and info popups

The top bar contains the search box plus the current date/time.
The footer shows the accepted controls for the current state.

## Main Page Usage

- `/`: start editing the search box
- `esc`: clear search
- `j`/`k` or arrow keys: move highlight
- `shift+up/down` or `shift+J/K`: extend multi-selection
- `enter`: open highlighted task, or open `Multi-Task Edit` when multiple tasks are selected
- `a`: toggle completed on highlighted task
- `d`/`delete`/`backspace`: delete highlighted task (with confirmation)
- `n`: create a new blank task in the default task list and open its task view
- `o`: open notes editor in Vim
- `x`: toggle between due-date sort and time-remaining sort
- `c`:
  - In due-date mode: hide/show completed tasks
  - In time-left mode: reverse sort order
- `u`: switch to task list page
- `q`: quit

In due-date mode, tasks are grouped by due-date headers (earliest to latest), and today's header is highlighted with `-- TODAY`.
The right-side column shows:

- `Task Lists - [u]`: lists ranked by priority with completed counts shown as `XX/XX`
- `Notes - [o]`: text loaded from `notes.txt` in the active data directory

## Task Expanded View

- `esc`: close task expanded view
- `t`: edit title (30 char limit)
- `d`: open description editor in Vim
- `e`: edit estimated hours (digits only)
- `w`: edit actual hours (digits only)
- `r`: edit due date (`YYYY-MM-DD`)
- `a`: toggle completed
- `l`: move task to another list
- `delete`: delete this task (with confirmation)
  - Type to filter list-name suggestions (case-insensitive)
  - Use up/down to choose suggestion
  - `enter` saves selected list, `esc` cancels

For single-line field edits:

- `enter` saves and closes input box
- `esc` cancels and closes input box

Task details are shown with inline labels such as `Title [t]` and `Description [d]`, and the description is rendered at the bottom of the popup.

## Multi-Task Edit

When multiple tasks are selected from a task list, pressing `enter` opens `Multi-Task Edit`.

- `r`: set the due date for all selected tasks
- `l`: move all selected tasks to another list
- `d`: delete all selected tasks (with confirmation)

Batch list moves use the same case-insensitive list-name suggestion flow as single-task list moves.

## Task List Page

- `/`: start typing in task-list search box
- `j`/`k` or arrow keys: move highlight
- `enter`: open highlighted task list expanded view
- `n`: create new task list and open its expanded view
- `d`/`delete`/`backspace`: delete highlighted task list (with confirmation)
- `o`: open notes editor in Vim
- `x`: toggle list sorting
- `c`: reverse current task-list sort order
- `u`: return to main page
- `q`: quit

Each row shows:

- List name
- `X to-do | Completed: X/X`

List title text is rendered in the list color.
The right-side column shows:

- `Tasks - [u]`: tasks from the currently highlighted list, starting with `XX/XX completed`
- `Notes - [o]`: the shared notes text

## Task List Expanded View

The expanded container border color matches the selected task list color.

- `esc`: close this expanded view
- `j`/`k` or arrow keys: move highlighted task in that list
- `enter`: open highlighted task in task expanded view
- `a`: toggle completed on highlighted task
- `d`/`delete`: delete highlighted task (with confirmation)
- `x`: toggle task sorting (same behavior as main page)
- `c`: mode-dependent toggle (same behavior as main page)
- `n`: create new task in this list and open task expanded view
- `t`: rename task list
- `p`: set task list priority (digits only)
- `q`: set task list color (`#RRGGBB`)

For list edits:

- `enter` saves immediately
- `esc` cancels edit

Delete confirmations:

- `enter`/`y`: confirm delete
- `esc`/`n`: cancel

## Data Storage

By default, task data is stored in your OS app-data directory.

- macOS: `~/Library/Application Support/life-tracking`
- Linux: `${XDG_DATA_HOME:-~/.local/share}/life-tracking`
- Windows: `%LOCALAPPDATA%\life-tracking`

Set `LIFE_TRACKING_DATA_DIR` to override that location.

Installed data location:

- macOS: `~/Library/Application Support/life-tracking`
- Linux: `${XDG_DATA_HOME:-~/.local/share}/life-tracking`
- Windows: `%LOCALAPPDATA%\life-tracking`

- One TOML file per list: `list_<id>.toml`
- Notes file: `notes.txt`
- Task-list-level fields at top: `name`, `priority`, `color_hex`
- Tasks stored under `[[tasks]]`

Example shape:

```toml
name = "Work"
priority = 1
color_hex = "#4DA3FF"

[[tasks]]
title = "Plan sprint goals"
description = "Draft sprint objective notes"
due_date = "2026-03-03"
estimated_hours = 90
actual_hours = 20
completed = false
completed_on = "2026-03-03"
```

## Persistence Behavior

- Any edit in the app is saved immediately to the active data directory.
- On startup, config files are loaded and validated.
- If any config file has invalid format/content, a startup popup shows the issue.
- When a task is marked complete, `completed_on` is recorded.
- Completed tasks are auto-pruned once they are more than 1 week past due date.
- Notes and task descriptions are edited by launching real `vim`, then saved back into the app after `vim` exits.

## Scripts

- `scripts/install-app.sh`: builds and installs the app with `cargo install --path . --force`
- `scripts/run-app.sh`: runs `cargo run`
- `scripts/run-tests.sh`: runs `cargo test`
