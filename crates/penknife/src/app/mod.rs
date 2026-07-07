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

#[cfg(test)]
pub(crate) mod test_support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Span;
use tokio::task::JoinHandle;
use tui_tree_widget::TreeState;

use std::time::{Duration, Instant};

use crate::config::Config;
use crate::event::AsyncSender;
use crate::hydrate::AmbiguousMatch;
use crate::scanner::{self, ScannedFile};
use crate::store::Store;
use crate::sync;
use crate::ui::input::LineEditor;
use anyhow::Result;

/// Shown when a token-requiring action runs with no token. The token is read
/// once at launch, so fixing it means restarting.
pub const NO_TOKEN_HINT: &str =
    "No GitHub token. Run `gh auth login` (or set GITHUB_TOKEN), then restart penknife.";

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
    /// Find in files: prompt for the search string.
    SearchQuery,
    /// Find in files: scrollable jump list of content matches.
    SearchResults {
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
    /// Manually link the selected file to an existing gist. The input editor
    /// holds a gist URL or bare ID; Enter fetches it and records the mapping.
    /// Used for gists created outside the app (e.g. the GitHub web UI) whose
    /// filename doesn't match the local file, so hydration can't auto-pair
    /// them.
    LinkGist {
        rel_path: String,
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
    /// Delete menu for the selected file: remote gist, local file, or both.
    /// `selected` indexes into `delete_options()`.
    DeleteMenu {
        selected: usize,
    },
    /// Git menu for the active root's repo: status / log / pull / push.
    /// `selected` indexes into `GIT_MENU_LABELS`.
    GitMenu {
        selected: usize,
    },
}

/// The delete menu's choices for the selected file. Built contextually:
/// remote options only appear when the file has a gist mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteChoice {
    Remote,
    Local,
    Both,
}

impl DeleteChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Remote => "Remote gist (keep local file)",
            Self::Local => "Local file (to trash; keep gist)",
            Self::Both => "Both (delete gist, trash file)",
        }
    }
}

/// Labels for the git menu, in display order.
pub const GIT_MENU_LABELS: &[&str] = &["git status", "git log", "git pull --rebase", "git push"];

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
        remote_id: String,
    },
    TrashLocal {
        rel_path: String,
        root: PathBuf,
    },
    /// Delete the remote gist and trash the local file in one confirmed
    /// step. The remote delete is async; the trash is immediate.
    DeleteBoth {
        rel_path: String,
        root: PathBuf,
        remote_id: String,
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
    /// `status_message` don't need to clear spans - the mismatch invalidates
    /// the rich version automatically.
    pub status_spans: Vec<Span<'static>>,
    pub picker_editor: LineEditor,
    pub picker: crate::picker::Picker,
    pub picker_matches: Vec<crate::picker::PickerMatch>,
    pub input_editor: LineEditor,
    pub gdoc_content: Option<String>,
    /// Set when the pending import came from a gist: the mapping to record
    /// against the file once it's saved. Cleared on save or cancel.
    pub pending_import_entry: Option<crate::store::FileEntry>,
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
    /// Find-in-files state: the query and its content matches, feeding the
    /// SearchResults jump list.
    pub search_query: String,
    pub search_matches: Vec<crate::replace::ReplaceMatch>,
    /// Multi-file find-and-replace state - populated as the user moves
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
    /// When the last background remote check *completed* (measured from
    /// completion, not start, so a slow check never overlaps the next).
    /// None = never; the first tick fires immediately.
    pub last_remote_poll: Option<Instant>,
    /// True while a remote check is in flight; prevents stacking.
    pub remote_check_inflight: bool,
    /// Consecutive remote-check failures. Each failure doubles the effective
    /// poll interval (capped), so an offline session doesn't spam errors.
    pub remote_poll_failures: u32,
    /// When the last local filesystem sweep ran.
    pub last_local_sweep: Option<Instant>,
    /// One-shot: incremental hydration fires on the first tick.
    pub startup_hydrate_done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Tree,
    Right,
}

/// Whether two scan results disagree on membership or mtimes. Drives the
/// local sweep's "did anything change outside the TUI" decision without
/// hashing content. Order-independent: `self.files` is kept sorted by the
/// active sort mode, while a fresh scan comes back in walk order.
fn files_differ(current: &[ScannedFile], scanned: &[ScannedFile]) -> bool {
    if current.len() != scanned.len() {
        return true;
    }
    let key = |f: &ScannedFile| (f.rel_path.clone(), f.modified);
    let mut a: Vec<_> = current.iter().map(key).collect();
    let mut b: Vec<_> = scanned.iter().map(key).collect();
    a.sort();
    b.sort();
    a != b
}

/// Single-character keys reserved by the TUI's built-in bindings. User
/// aliases cannot shadow these - conflicting entries are dropped at load
/// time and reported in the status bar.
const RESERVED_KEYS: &[char] = &[
    'q', '?', '/', 'j', 'k', 'l', 'h', 'u', 'd', 'c', 'C', 'V', 'e', 'o', 'X', 'n', 'N', 'D', 'M',
    'R', 'I', 's', 'm', 'O', 'B', 'g', 'L', 'f', 'p',
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

        let token = penknife_gist::auth::resolve_token().ok();

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
            pending_import_entry: None,
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
            search_query: String::new(),
            search_matches: Vec::new(),
            replace_query: String::new(),
            replace_target: String::new(),
            replace_matches: Vec::new(),
            replace_checked: Vec::new(),
            git_repo_root: None,
            git_statuses: std::collections::HashMap::new(),
            status_cache: std::collections::HashMap::new(),
            last_remote_poll: None,
            remote_check_inflight: false,
            remote_poll_failures: 0,
            // The constructor just scanned; no point sweeping again for one
            // interval.
            last_local_sweep: Some(Instant::now()),
            startup_hydrate_done: false,
        };
        // Populate the per-file sync-status cache and git status before the
        // first tree build so leaf glyphs reflect the loaded store right away.
        // Without the status pass, every file paints as NotGisted until the
        // first `r` ran `refresh_files` - making a freshly-loaded store look
        // entirely unsynced.
        app.refresh_status_cache();
        app.refresh_git_status();
        app.rebuild_tree();
        app.update_status();
        // Without a token, GitHub sync is simply off; the browse/search/copy
        // side works fine. Say so once, instead of letting the user discover
        // it by pressing `u` and hitting a bare error. An alias warning, if
        // any, is the more actionable message and wins.
        if app.token.is_none() {
            app.status_message = "No GitHub token: browsing and p (copy as rich text) work now. \
                 Run `gh auth login` or set GITHUB_TOKEN to enable sync."
                .into();
        }
        if let Some(w) = alias_warning {
            app.status_message = w;
        }
        Ok(app)
    }

    /// Periodic work, called once per main-loop iteration (roughly every
    /// 50ms). Owns all background cadence: the local filesystem sweep, the
    /// remote poll, and the one-shot startup hydration. Only runs in Normal
    /// mode so modal state (diff, resolver, dialogs) is never yanked out
    /// from under the user.
    pub fn tick(&mut self) {
        if !matches!(self.mode, Mode::Normal) {
            return;
        }

        // Local sweep: rescan and refresh only when something changed on
        // disk, so the common no-op case costs one directory walk and zero
        // hashing.
        let local_secs = self.config.poll.local_secs;
        if local_secs > 0
            && self
                .last_local_sweep
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(local_secs))
        {
            self.last_local_sweep = Some(Instant::now());
            if let Some(entry) = self.current_root_entry()
                && entry.path.exists()
            {
                let ignore = scanner::build_globset(&entry.ignore);
                if let Ok(scanned) = scanner::scan_directory(&entry.path, &ignore)
                    && files_differ(&self.files, &scanned)
                    && let Err(e) = self.refresh_files()
                {
                    self.status_message = format!("Refresh failed: {e}");
                }
            }
        }

        // One-shot incremental hydration: pick up gists created or changed
        // since the last walk (or do the first full walk on a new root).
        if !self.startup_hydrate_done {
            self.startup_hydrate_done = true;
            if self.token.is_some() && !self.files.is_empty() {
                self.start_hydration();
            }
        }

        // Remote poll: exponential backoff on consecutive failures, capped
        // at 64x the configured interval.
        let remote_secs = self.config.poll.remote_secs;
        if remote_secs > 0 && self.token.is_some() && !self.remote_check_inflight {
            let effective = remote_secs * (1u64 << self.remote_poll_failures.min(6));
            if self
                .last_remote_poll
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(effective))
            {
                // Stamp even when there's nothing to check, so an unmapped
                // tree doesn't re-evaluate every tick.
                self.last_remote_poll = Some(Instant::now());
                self.start_remote_check();
            }
        }
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

    fn scanned(rel: &str, secs: u64) -> ScannedFile {
        ScannedFile {
            rel_path: rel.to_string(),
            abs_path: PathBuf::from(format!("/x/{rel}")),
            modified: std::time::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn files_differ_ignores_order() {
        let a = vec![scanned("a.md", 1), scanned("b.md", 2)];
        let b = vec![scanned("b.md", 2), scanned("a.md", 1)];
        assert!(!files_differ(&a, &b));
    }

    #[test]
    fn files_differ_detects_mtime_change() {
        let a = vec![scanned("a.md", 1)];
        let b = vec![scanned("a.md", 9)];
        assert!(files_differ(&a, &b));
    }

    #[test]
    fn files_differ_detects_membership_change() {
        let a = vec![scanned("a.md", 1)];
        let b = vec![scanned("a.md", 1), scanned("new.md", 2)];
        assert!(files_differ(&a, &b));
        assert!(files_differ(&b, &a));
        // Same length, different file.
        let c = vec![scanned("other.md", 1)];
        assert!(files_differ(&a, &c));
    }
}
