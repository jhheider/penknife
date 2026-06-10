//! Application state and behavior, split by concern:
//!
//! - [`view`]: tree/preview/status refresh, sorting, navigation, mouse
//! - [`keys`]: key dispatch and per-mode key handlers
//! - [`gist`]: gist sync operations (push/pull/diff/check/hydrate/delete)
//!   and async-event application
//! - [`files`]: local file operations (rename/trash/format/replace,
//!   clipboard, Google Doc import, git shell-outs, bulk ops)
//!
//! This module owns the `App` struct itself, its mode/action types, the
//! constructor, and the small accessors everything else builds on.

mod files;
mod gist;
mod keys;
mod view;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Span;
use tokio::task::JoinHandle;
use tui_tree_widget::TreeState;

use crate::config::Config;
use crate::error::Result;
use crate::event::AsyncSender;
use crate::hydrate::{AmbiguousMatch, HydrationProgress};
use crate::scanner::{self, ScannedFile};
use crate::store::Store;
use crate::sync;
use crate::ui::input::LineEditor;

#[derive(Debug)]
pub enum Mode {
    Normal,
    Help,
    FilePicker {
        selected: usize,
    },
    Diff {
        local: String,
        remote: String,
    },
    Confirm {
        message: String,
        action: ConfirmAction,
    },
    GdocUrl,
    GdocFilename,
    Hydrating {
        progress: Option<HydrationProgress>,
        done: bool,
    },
    Message(String),
    RootSwitcher {
        selected: usize,
    },
    AddRoot,
    SetupRoot,
    ResolveAmbiguous {
        item: usize,
        selected: usize,
    },
    /// Step 1 of replace: prompt for the search string.
    ReplaceQuery,
    /// Step 2 of replace: prompt for the replacement string.
    ReplaceTarget,
    /// Step 3: scrollable checklist of all matches. User toggles to omit
    /// false positives, then Enter applies.
    ReplaceReview {
        selected: usize,
    },
    /// Rename / move the selected file. The input editor is pre-filled with
    /// the current rel_path; Enter commits, Esc cancels.
    Rename {
        old_rel: String,
    },
    /// Modal picker for the tree's sort order. `selected` indexes into
    /// `SortMode::all()`.
    SortMenu {
        selected: usize,
    },
    /// Bulk operations menu (push-all-dirty / pull-all-remote-newer /
    /// format-all-json / prune-orphans). `selected` indexes into the
    /// option list.
    BulkMenu {
        selected: usize,
    },
}

/// The four operations offered by the bulk menu. Each carries the rel_paths
/// (or store keys) it will touch, computed at menu-construction time so the
/// confirm dialog can quote an accurate count.
#[derive(Debug, Clone)]
pub enum BulkAction {
    PushAllDirty { rels: Vec<String> },
    PullAllRemoteNewer { rels: Vec<String> },
    FormatAllJson { rels: Vec<String> },
    PruneOrphans { rels: Vec<String> },
}

impl BulkAction {
    pub fn count(&self) -> usize {
        match self {
            Self::PushAllDirty { rels }
            | Self::PullAllRemoteNewer { rels }
            | Self::FormatAllJson { rels }
            | Self::PruneOrphans { rels } => rels.len(),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::PushAllDirty { .. } => "Push all dirty",
            Self::PullAllRemoteNewer { .. } => "Pull all remote-newer",
            Self::FormatAllJson { .. } => "Format all JSON",
            Self::PruneOrphans { .. } => "Prune store orphans",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    SyncDown,
    /// Re-push overwriting a remote that changed since the last sync. Only
    /// reachable from the PushBlocked prompt, so the user has already seen
    /// what diverged.
    ForcePush {
        rel_path: String,
    },
    DeleteRemote {
        rel_path: String,
        root: PathBuf,
        gist_id: String,
    },
    TrashLocal {
        rel_path: String,
        root: PathBuf,
    },
    /// A bulk operation, with the file list captured at menu-display time.
    Bulk(BulkAction),
    /// Run a shell command via the suspend/resume pattern. Used by the git
    /// write commands (`git push`, `git pull --rebase`).
    RunShell {
        cmd: String,
    },
}

pub struct App {
    pub config: Config,
    pub store: Store,
    pub files: Vec<ScannedFile>,
    pub tree_items: Vec<tui_tree_widget::TreeItem<'static, String>>,
    pub tree_identifiers: Vec<String>,
    pub tree_file_ids: HashSet<String>,
    pub tree_state: TreeState<String>,
    pub mode: Mode,
    pub preview_content: String,
    pub status_message: String,
    pub status_color: Color,
    /// Styled spans that mirror `status_message`. When non-empty and their
    /// concatenated text equals `status_message`, the renderer uses these for
    /// a multi-color dashboard; otherwise it falls back to the flat string +
    /// `status_color`. This means transient setters that touch only
    /// `status_message` don't need to clear spans — the mismatch invalidates
    /// the rich version automatically.
    pub status_spans: Vec<Span<'static>>,
    pub picker_editor: LineEditor,
    pub picker: crate::picker::Picker,
    pub picker_matches: Vec<crate::picker::PickerMatch>,
    pub input_editor: LineEditor,
    pub gdoc_content: Option<String>,
    pub should_quit: bool,
    pub async_tx: AsyncSender,
    pub token: Option<String>,
    pub active_root: usize,
    pub pending_ambiguous: Vec<AmbiguousMatch>,
    /// If set, copy the URL of this rel_path after the next successful PushDone.
    pub pending_copy: Option<String>,
    /// Vertical scroll offset (rows) for the markdown preview pane.
    pub preview_scroll: u16,
    /// Vertical scroll offset (rows) for the diff pane.
    pub diff_scroll: u16,
    /// Last-rendered area for the tree pane; used to route mouse events.
    pub tree_pane_rect: Rect,
    /// Last-rendered area for the right pane (preview/diff).
    pub right_pane_rect: Rect,
    /// Which pane currently captures j/k/arrow input in Normal mode.
    pub focused_pane: PaneFocus,
    /// True when crossterm mouse capture is currently enabled (env var only).
    pub mouse_capture: bool,
    /// Outstanding spawned tokio tasks; aborted on quit.
    pub tasks: Vec<JoinHandle<()>>,
    /// If set, main.rs should suspend the TUI, spawn `$EDITOR` on this file,
    /// then resume. Cleared by the main loop before each editor invocation.
    pub pending_editor: Option<PathBuf>,
    /// If set, main.rs should suspend the TUI and run this shell command
    /// (via `sh -c`) with PWD set to the active root, then resume.
    pub pending_alias: Option<String>,
    /// Multi-file find-and-replace state — populated as the user moves
    /// through ReplaceQuery → ReplaceTarget → ReplaceReview.
    pub replace_query: String,
    pub replace_target: String,
    pub replace_matches: Vec<crate::replace::ReplaceMatch>,
    pub replace_checked: Vec<bool>,
    /// If the active root is inside a git repo, this is the repo's worktree
    /// root and the per-file status map (keyed by rel_path relative to the
    /// *active root*, matching ScannedFile.rel_path). Refreshed by
    /// `refresh_git_status` on root switch and on `r`.
    pub git_repo_root: Option<PathBuf>,
    pub git_statuses: std::collections::HashMap<String, crate::git::GitStatus>,
    /// Per-file sync status, populated once per `refresh_files`. Computing
    /// status requires a disk read + SHA-256, so this cache lets the
    /// tree-render, sort-by-status, status-counts, and bulk-menu paths all
    /// reuse one pass instead of each doing their own ~N reads per call.
    /// Keyed by rel_path. Files not in this map are treated as `NotGisted`.
    pub status_cache: std::collections::HashMap<String, sync::SyncStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Tree,
    Right,
}

/// Single-character keys reserved by the TUI's built-in bindings. User
/// aliases cannot shadow these — conflicting entries are dropped at load
/// time and reported in the status bar.
const RESERVED_KEYS: &[char] = &[
    'q', '?', '/', 'j', 'k', 'l', 'h', 'u', 'd', 'c', 'C', 'V', 'e', 'o', 'X', 'n', 'N', 'D', '_',
    'H', 'r', 'R', 'I', 's', 'm', '=', 'O', 'B', 'g', 'G', '(', ')', 'f',
];

impl App {
    pub fn new(async_tx: AsyncSender) -> Result<Self> {
        let mut config = Config::load()?;
        let store = Store::load()?;

        // Filter aliases against built-in keys and shape constraints (single
        // character keys only). Collect names of any drops to surface in the
        // status bar so the user sees the warning without scraping stderr.
        let mut dropped: Vec<String> = Vec::new();
        config.aliases.retain(|k, _cmd| {
            let chars: Vec<char> = k.chars().collect();
            if chars.len() != 1 {
                dropped.push(format!("'{k}' (not a single char)"));
                return false;
            }
            if RESERVED_KEYS.contains(&chars[0]) {
                dropped.push(format!("'{k}' (built-in)"));
                return false;
            }
            true
        });
        let alias_warning = if dropped.is_empty() {
            None
        } else {
            Some(format!(
                "Dropped alias{}: {}",
                if dropped.len() == 1 { "" } else { "es" },
                dropped.join(", ")
            ))
        };

        let token = gist_rs::auth::resolve_token().ok();

        let start_mode = if config.roots.is_empty() {
            Mode::SetupRoot
        } else {
            Mode::Normal
        };

        let files = if config.roots.is_empty() {
            Vec::new()
        } else {
            let ignore = scanner::build_globset(&config.roots[0].ignore);
            scanner::scan_directory(&config.roots[0].path, &ignore).unwrap_or_default()
        };

        let mut app = App {
            config,
            store,
            files,
            tree_items: Vec::new(),
            tree_identifiers: Vec::new(),
            tree_file_ids: HashSet::new(),
            tree_state: TreeState::default(),
            mode: start_mode,
            preview_content: String::new(),
            status_message: String::new(),
            status_color: Color::White,
            status_spans: Vec::new(),
            picker_editor: LineEditor::new(),
            picker: crate::picker::Picker::new(),
            picker_matches: Vec::new(),
            input_editor: LineEditor::new(),
            gdoc_content: None,
            should_quit: false,
            async_tx,
            token,
            active_root: 0,
            pending_ambiguous: Vec::new(),
            pending_copy: None,
            preview_scroll: 0,
            diff_scroll: 0,
            tree_pane_rect: Rect::default(),
            right_pane_rect: Rect::default(),
            focused_pane: PaneFocus::Tree,
            mouse_capture: false,
            tasks: Vec::new(),
            pending_editor: None,
            pending_alias: None,
            replace_query: String::new(),
            replace_target: String::new(),
            replace_matches: Vec::new(),
            replace_checked: Vec::new(),
            git_repo_root: None,
            git_statuses: std::collections::HashMap::new(),
            status_cache: std::collections::HashMap::new(),
        };
        // Populate git status before the first tree build so leaf glyphs
        // show the right state without needing an explicit refresh first.
        app.refresh_git_status();
        app.rebuild_tree();
        app.update_status();
        if let Some(w) = alias_warning {
            app.status_message = w;
        }
        Ok(app)
    }

    /// Get the path of the current root directory, if any.
    fn current_root(&self) -> Option<&PathBuf> {
        self.config.roots.get(self.active_root).map(|r| &r.path)
    }

    /// Get the full `Root` entry (path + ignore patterns) for the active root.
    fn current_root_entry(&self) -> Option<&crate::config::Root> {
        self.config.roots.get(self.active_root)
    }

    /// Public accessor for the active root path. Used by main.rs to set the
    /// working directory when running user aliases or other shell-outs.
    pub fn active_root_path(&self) -> Option<PathBuf> {
        self.current_root().cloned()
    }

    /// Spawn a tokio task and track its JoinHandle so it can be aborted on quit.
    /// Also opportunistically drops any handles that have already finished.
    fn spawn_tracked<F>(&mut self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tasks.retain(|h| !h.is_finished());
        self.tasks.push(tokio::spawn(fut));
    }

    /// Abort any in-flight tasks. Called from main on shutdown so we don't
    /// keep work going after the UI is gone.
    pub fn abort_tasks(&mut self) {
        for h in self.tasks.drain(..) {
            h.abort();
        }
    }

    /// Get the currently selected file's rel_path (if it's a leaf file, not a directory).
    pub fn selected_file(&self) -> Option<String> {
        let selected = self.tree_state.selected();
        let id = selected.last()?.clone();
        if self.tree_file_ids.contains(&id) {
            Some(id)
        } else {
            None
        }
    }

    /// Get the absolute path for a rel_path.
    pub fn abs_path(&self, rel_path: &str) -> PathBuf {
        if let Some(root) = self.current_root() {
            root.join(rel_path)
        } else {
            PathBuf::from(rel_path)
        }
    }
}

/// Single-quote a path for safe inclusion in a `sh -c` command. Single
/// quotes in the input are escaped via the standard `'\''` trick.
fn shell_quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Expand a leading `~` to the user's home directory. Returns the input
/// unchanged if it doesn't start with `~` or the home directory can't be
/// resolved.
fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix('~')
        && let Some(home) = dirs::home_dir()
    {
        home.join(rest.strip_prefix('/').unwrap_or(rest))
    } else {
        PathBuf::from(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_passes_absolute_through() {
        assert_eq!(expand_tilde("/foo/bar"), PathBuf::from("/foo/bar"));
    }

    #[test]
    fn expand_tilde_passes_relative_through() {
        assert_eq!(expand_tilde("foo/bar"), PathBuf::from("foo/bar"));
    }

    #[test]
    fn expand_tilde_rewrites_home_prefix() {
        let Some(home) = dirs::home_dir() else {
            return; // can't validate without a home dir
        };
        assert_eq!(expand_tilde("~/Documents"), home.join("Documents"));
        // Bare ~ also expands to home.
        assert_eq!(expand_tilde("~"), home);
    }
}
