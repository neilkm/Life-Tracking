# AGENTS.md

## Overview

This repo is a small Rust terminal UI task manager. It runs as a full-screen TUI using `ratatui` + `crossterm`, stores state as TOML files on disk, and persists edits immediately.

The app supports:

- Multiple task lists, each with a name, numeric priority, and display color
- A combined main task view across all lists
- A task-list page with per-list counts and sorting
- Expanded overlays for both task lists and individual tasks
- Single-line inline editing for titles, dates, hours, priorities, colors, and list moves
- External Vim editing for notes and task descriptions
- Search, due-date sorting, time-left sorting, completion toggling, multi-selection, batch task operations, and delete confirmations
- Startup validation, duplicate-id repair, seeded demo data, and automatic pruning of old completed tasks

## Agent Rules

These instructions are mandatory for any agent working in this repo:

- Do not make changes outside this repository, ever.
- Always ask before running any install script, including `scripts/install-app.sh` or other commands that install the app onto the machine.
- Never alter, overwrite, migrate, prune, or delete saved app data without asking the user first and receiving an explicit `yes`.
- When testing anything that could touch app data, prefer repo-local fixtures, temporary directories, or `LIFE_TRACKING_DATA_DIR` pointed at a disposable path.

## Repo Map

- `src/lib.rs`: data model, file loading/saving, validation, sorting/filtering, mutation methods, pruning, color parsing
- `src/main.rs`: TUI app state, rendering, key routing, inline editors, Vim handoff, popups/confirmations, boot path
- `tests/app_logic.rs`: integration tests for data loading, sorting, list suggestions, pruning, and persistence
- `tests/data/*.toml`: fixtures used by the integration tests
- `scripts/run-app.sh`, `scripts/run-tests.sh`, `scripts/install-app.sh`: thin cargo wrappers

There is no deeper module split. Most UI behavior lives in a single `App` impl in `src/main.rs`.

## How The App Works

### Boot and startup

Startup is intentionally simple:

1. `main()` resolves the data directory.
2. `AppData::load_from_dir()` loads every `.toml` file in that directory.
3. Loaded task lists are validated, duplicate ids are repaired, old completed tasks are pruned, and everything is re-saved into canonical `list_<id>.toml` files.
4. `notes.txt` is loaded separately.
5. `App::run()` enables raw mode, enters the alternate screen, and starts the render/event loop.

The data directory comes from `LIFE_TRACKING_DATA_DIR` when set. Otherwise it uses the platform app-data directory and falls back to `.life-tracking`.

### State model

The central runtime object is `App` in `src/main.rs`. It owns:

- All loaded `AppData`
- Current page (`Main` or `TaskLists`)
- Sort/search flags
- Selection indices for each list/task context
- Overlay/editor state
- Popup/confirmation state
- Loaded notes text

The draw path and the key-routing path use the same precedence order:

1. Multi-task edit popup
2. Delete confirmation
3. Popup
4. Single-line editor
5. Expanded task
6. Expanded list
7. Base page (`Main` or `TaskLists`)

If you add a new modal/overlay, keep draw order and input-routing order in sync.

### Persistence model

All task/list mutations should go through `AppData` methods in `src/lib.rs`.

- Every mutation ends in `persist_after_edit(today)`.
- `persist_after_edit()` prunes completed overdue tasks, then calls `save_all()`.
- `save_all()` rewrites generated list files and removes stale generated files that no longer correspond to a live list.

The only exception is notes editing: `notes.txt` is written directly from `open_notes_editor()` after `vim` exits.

### Operational flow

The normal runtime flow is:

1. Launch the binary.
2. The app loads task-list TOML files plus `notes.txt`.
3. The app opens on the main page with the combined task view.
4. The user either:
   - stays on the main page to search, sort, toggle completion, create/delete tasks, open notes, select multiple tasks, or open a task
   - switches to the task-list page to browse lists, sort/filter lists, create/delete lists, or open a list
5. Opening a list shows the expanded list overlay on top of the current page.
6. Opening a task shows the expanded task overlay on top of the current page or list overlay.
7. From task or list overlays, the user can enter single-line edit mode for structured fields.
8. From notes or task description fields, the user can launch external `vim`.
9. Any confirmed task/list edit persists immediately to disk.
10. Delete actions always go through a confirmation overlay before mutating state.
11. Popup overlays report startup issues and runtime save/validation errors without exiting the app.

The app behaves like a layered state machine rather than route-based navigation. Most work is controlled by which overlay is currently active.

## Feature Map

### Main page

Implemented in `draw_main_page()`, `handle_main_key()`, and helpers that build task rows.

Behavior:

- Shows combined tasks from all lists
- Includes a search box
- Supports two task sort modes:
  - `DueDate`: ascending due date, then ascending list priority, then title
  - `TimeLeft`: descending `estimated - actual`, optionally reversed
- Can hide completed tasks only while in `DueDate` mode
- `n` creates a blank task in the default task list and opens its task view
- `o` opens notes in Vim
- Supports range-style multi-selection with Shift navigation
- Shows a right-side panel with task lists ranked by priority and the contents of `notes.txt`
- Search matches task title, description, list name, and due-date string
- When the search term matches a description, the row renders a short bolded preview snippet

Task rows are built by `build_task_rows()`. Due-date grouping headers are inserted only in due-date mode, and today's header is rendered specially.

### Task list page

Implemented in `draw_task_list_page()`, `filtered_task_list_items()`, and `handle_task_list_page_key()`.

Behavior:

- Shows all task lists
- Supports search by list name only
- Supports sorting by remaining task count or list priority
- Supports reverse ordering
- Can create and delete lists
- `o` opens notes in Vim
- Shows a right-side panel with tasks from the currently highlighted list plus notes
- Opening a list launches the expanded list overlay

### Expanded list view

Implemented in `draw_list_expanded()` and `handle_list_expanded_key()`.

Behavior:

- Shows list metadata at the top and that list's tasks below
- Reuses the same task sort flags as the main page
- Supports creating tasks, toggling completion, deleting tasks, opening task details, and editing list metadata
- Border/title color comes from the list's `color_hex`

There is no independent search field inside this overlay.

### Expanded task view

Implemented in `draw_task_expanded()` and `handle_task_expanded_key()`.

Behavior:

- Shows title, description, list, estimated, actual, due date, and completion info
- Allows single-line edits for title, estimated, actual, due date, and target list
- Launches external `vim` for description editing
- Supports completion toggle and delete confirmation

Moving a task to another list uses case-insensitive list-name lookup plus suggestion selection.

### Single-line inline editing

Implemented through:

- `EditorState`
- `start_task_edit()`
- `start_list_edit()`
- `handle_editor_key()`
- `commit_edit()`

Validation and constraints are split between UI and data layer:

- Task title input is capped at 30 characters in the UI
- Estimated/actual/priorities accept digits only
- Due dates accept digits plus `-` and must parse as `%Y-%m-%d`
- List color accepts hex digits plus optional `#`, max 7 chars, then normalizes to uppercase `#RRGGBB`
- Empty task titles and empty list names are ignored on commit
- Invalid colors are ignored by the data layer
- List moves only succeed if the chosen name resolves to an existing list

### Vim-backed text editing

Notes and task descriptions now use real external `vim`, not an in-app modal editor.

Behavior:

- `o` opens shared notes in `vim`
- Task description editing launches `vim` from task view
- The app writes a temp file, inserts a generated help block, launches `vim`, then strips that help block before saving
- Returning from `vim` requires a full terminal/TUI restore path and a forced redraw

### Multi-task edit

Implemented through:

- `multi_selected_tasks`
- `task_selection_anchor`
- `MultiTaskEditState`
- `draw_multi_task_edit()`
- `handle_multi_task_edit_key()`

Behavior:

- Shift navigation builds a selected task range
- Pressing `Enter` with multiple selected tasks opens `Multi-Task Edit`
- Batch actions support due-date change, move to another list, and delete
- Batch delete still routes through the shared confirmation popup

### Popups and confirmations

Implemented in `draw_popup()`, `draw_confirm()`, `handle_confirm_key()`, and `push_popup()`.

Startup validation errors, save failures, invalid due-date messages, and unknown editor commands all surface through popup messages.

Delete actions are always gated by a confirmation overlay.

## Flow Of Operation

### Startup and data load flow

1. Resolve the active data directory.
2. Read all `.toml` files in that directory.
3. Deserialize each file into `TaskList`.
4. Validate list-level constraints such as non-empty names and valid colors.
5. Accumulate startup messages for invalid files instead of aborting the whole app.
6. If nothing valid loads, seed default lists.
7. Repair duplicate or zero ids.
8. Prune completed tasks older than one week past due.
9. Save the canonicalized state back to generated list files.
10. Load `notes.txt`.
11. Enter raw-mode TUI loop.

### Per-frame UI flow

1. Draw the controls banner.
2. Draw the active base page.
3. Draw expanded list overlay if open.
4. Draw expanded task overlay if open.
5. Draw multi-task edit popup if open.
6. Draw popup if open.
7. Draw delete confirmation if open.
8. Poll for keyboard input and dispatch it according to the active overlay priority.

### Edit and persistence flow

1. User enters an edit mode.
2. The UI collects and constrains input.
3. On save/confirm, the app validates or parses the value as needed.
4. Task/list changes go through `AppData`.
5. `AppData` mutates in-memory state.
6. `persist_after_edit()` prunes expired completed tasks and writes the full list set back to disk.
7. The UI normalizes selections so indices remain valid after creates, moves, deletes, or sort/filter changes.

### Notes and description flow

1. User opens notes with `o`, or opens task description editing from task view.
2. The app writes the current text plus a generated help block to a temp file.
3. The TUI suspends and launches external `vim`.
4. On exit, the app restores the alternate-screen TUI and forces a full redraw.
5. The generated help block is stripped before saving.
6. Notes write directly to `notes.txt`.
7. Descriptions save through `AppData::update_task_description()`.

### Delete flow

1. User triggers delete from a page or overlay.
2. The app opens a confirmation overlay.
3. Only `Enter` or `y` executes the delete.
4. The app clears now-invalid overlays/selections after deletion.

## Data Model and Storage

### Task and list schema

`Task` and `TaskList` live in `src/lib.rs`.

Task fields:

- `id: u64`
- `title: String`
- `description: String`
- `due_date: NaiveDate`
- `estimated_minutes: u32`
- `actual_minutes: u32`
- `completed: bool`
- `completed_on: Option<NaiveDate>`

List fields:

- `id: u64`
- `name: String`
- `priority: u8`
- `color_hex: String`
- `tasks: Vec<Task>`

Important compatibility quirk:

- The Rust field names are `estimated_minutes` and `actual_minutes`.
- The serialized TOML keys are still `estimated_hours` and `actual_hours`.
- The deserializer also accepts `estimated_minutes` and `actual_minutes` as aliases.
- The UI labels and README also say "hours".
- There is no unit conversion. The app currently treats these values as plain integers.

Do not casually rename these fields or "fix" units without updating tests, fixtures, docs, and persistence expectations together.

### On-disk layout

The data directory contains:

- `list_<id>.toml` for each task list
- `notes.txt` for freeform notes

Only generated `list_<id>.toml` files are cleaned up automatically. Arbitrary extra files are left alone.

### Validation and repair

`AppData::load_from_dir()` does more than plain deserialization:

- Rejects invalid TOML with startup messages instead of aborting the whole load
- Rejects empty list names
- Rejects invalid `color_hex`
- Seeds default lists when nothing valid exists
- Repairs duplicate or zero list/task ids
- Prunes completed tasks older than one week past due date
- Saves the repaired/canonicalized state back to disk

## Sorting, Search, and Selection Rules

- Main task search is case-insensitive for text fields.
- Due-date matching uses the string form of `NaiveDate`.
- List search only matches list names.
- Due-date sort groups by date headers in the UI.
- Time-left sort uses `estimated - actual`.
- Reverse ordering only applies in time-left task mode and list-page list sorting.
- Completion hiding only applies in due-date mode.

Selection state is stored as raw indices, so after any mutation that changes list lengths the app normalizes indices. If you add new delete/move/create flows, keep those normalization calls.

## Coding Style In This Repo

The codebase is intentionally direct and stateful.

Prefer:

- Extending `AppData` for domain/persistence changes
- Extending `App` methods for UI changes
- Small enums and helper functions over introducing extra abstraction layers
- Synchronous filesystem I/O
- Straight-line control flow with explicit `match` statements
- Keeping feature state local to `App` instead of introducing global indirection

Avoid:

- Premature module splitting for small changes
- Adding async/runtime complexity
- Introducing persistence paths that bypass `AppData`, except where notes already do
- Changing keybindings without also updating `controls_text()`
- Changing overlay precedence in only draw or input code but not both

The repo already follows `cargo fmt` style. Keep formatting conventional.

## Constraints To Preserve

- The app must always restore the terminal cleanly after leaving raw mode / alternate screen.
- User edits should remain immediately persisted; there is no save buffer for tasks/lists.
- Popup errors should not crash the app.
- Task/list ids must remain stable and unique.
- `color_hex` must stay compatible with `#RRGGBB`.
- Generated filenames must remain `list_<id>.toml` unless migration work is done intentionally.
- Old completed tasks should continue to be pruned when more than 7 days past due.
- Search/sort behavior is covered by tests; update tests if semantics change intentionally.

## Test Expectations

Run:

```bash
cargo test
```

Current integration coverage checks:

- Invalid config handling while preserving valid lists
- Due-date and time-left sorting behavior
- Case-insensitive list suggestions
- Pruning of old completed tasks on load
- Persistence of completion toggles and due-date edits

If you change storage, sort order, search behavior, pruning rules, or validation, update `tests/app_logic.rs` and the TOML fixtures.

## Practical Change Guidance

- New task/list fields usually require changes in both `Task`/`TaskList` and the expanded view render/edit paths.
- New modal UI should be added to both `draw()` and `handle_key()` precedence chains.
- New mutation flows should usually end with selection normalization in the relevant view.
- If you add a new keybinding, update the controls banner so the UI stays self-describing.
- If you add new persisted files beyond `notes.txt` and generated list files, think through cleanup rules explicitly.
- If you change the `vim` handoff path, preserve terminal suspend/resume behavior and the forced redraw after return.
