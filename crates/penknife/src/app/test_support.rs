//! Shared test scaffolding for the `app` submodules.
//!
//! `App::new` reads global config/store from the user's data dir and resolves
//! a GitHub token, so it can't run in tests. [`new_for_test`] builds an
//! equivalent `App` from `Default` config/store with a single root pointed at
//! a caller-owned temp directory, then runs the same post-construction refresh
//! pipeline the real constructor does.
//!
//! Any handler that calls `store.save()` / `config.save()` writes under the
//! data dir. [`guard`] redirects that dir to a process-wide temp location
//! (via `HOME` / `XDG_*`) and serializes those tests so their shared
//! `store.json` never races.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Color;
use tui_tree_widget::TreeState;

use super::{App, Mode, PaneFocus};
use crate::config::{Config, Root};
use crate::event::AsyncSender;
use crate::picker::Picker;
use crate::scanner;
use crate::store::Store;
use crate::ui::input::LineEditor;

/// A bare `KeyEvent` for `code` with no modifiers.
pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A single character key with no modifiers.
pub(crate) fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

/// A `Ctrl`-modified key event.
pub(crate) fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

static ENV_LOCK: Mutex<()> = Mutex::new(());
static HOME_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

/// Acquire the serialization lock for tests that persist state, redirecting
/// the data dir to a throwaway location on first use. Hold the returned guard
/// for the duration of the test.
pub(crate) fn guard() -> MutexGuard<'static, ()> {
    let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    HOME_DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir for test HOME");
        // SAFETY: env mutation is confined to tests holding `ENV_LOCK`, which
        // serializes every writer, and this initializer runs exactly once.
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        }
        dir
    });
    g
}

/// Write `content` to `root/rel`, creating parent directories as needed.
/// Returns the absolute path written.
pub(crate) fn write_file(root: &Path, rel: &str, content: &str) -> std::path::PathBuf {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(&abs, content).expect("write test file");
    abs
}

/// Select `rel` in the tree (expanding ancestors), so `selected_file()`
/// returns it. Convenience wrapper over the crate-private `jump_to`.
pub(crate) fn select(app: &mut App, rel: &str) {
    app.jump_to(rel);
}

/// Build an `App` over a single root at `root`, with `Default` config/store
/// and no token, then run the real refresh pipeline. No global state is read.
pub(crate) fn new_for_test(root: &Path, tx: AsyncSender) -> App {
    let mut config = Config::default();
    config.roots.push(Root::new(root.to_path_buf()));

    let ignore = scanner::build_globset(&[]);
    let files = scanner::scan_directory(root, &ignore).unwrap_or_default();

    let mut app = App {
        config,
        store: std::sync::Arc::new(Store::default()),
        files,
        tree_items: Vec::new(),
        tree_identifiers: Vec::new(),
        tree_file_ids: HashSet::new(),
        tree_state: TreeState::default(),
        mode: Mode::Normal,
        preview_content: String::new(),
        status_message: String::new(),
        status_color: Color::White,
        status_spans: Vec::new(),
        picker_editor: LineEditor::new(),
        picker: Picker::new(),
        picker_matches: Vec::new(),
        input_editor: LineEditor::new(),
        gdoc_content: None,
        pending_import_entry: None,
        should_quit: false,
        async_tx: tx,
        token: None,
        gist_client: None,
        active_root: 0,
        pending_ambiguous: Vec::new(),
        pending_copy: None,
        pending_pushes: HashSet::new(),
        preview_scroll: 0,
        diff_scroll: 0,
        tree_pane_rect: Rect::default(),
        right_pane_rect: Rect::default(),
        focused_pane: PaneFocus::Tree,
        mouse_capture: false,
        tasks: Vec::new(),
        pending_editor: None,
        pending_alias: None,
        search_query: String::new(),
        search_matches: Vec::new(),
        replace_query: String::new(),
        replace_target: String::new(),
        replace_matches: Vec::new(),
        replace_checked: Vec::new(),
        git_repo_root: None,
        git_statuses: HashMap::new(),
        status_cache: HashMap::new(),
        last_remote_poll: None,
        remote_check_inflight: false,
        remote_poll_failures: 0,
        last_local_sweep: Some(Instant::now()),
        startup_hydrate_done: false,
        refresh_generation: 0,
    };
    app.refresh_status_cache();
    app.refresh_git_status();
    app.rebuild_tree();
    app.update_status();
    app
}
