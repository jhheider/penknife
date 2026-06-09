use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use tokio::task::JoinHandle;
use tui_tree_widget::TreeState;

use crate::config::Config;
use crate::error::Result;
use crate::event::{AsyncEvent, AsyncSender};
use crate::hydrate::{AmbiguousMatch, HydrationProgress};
use crate::scanner::{self, ScannedFile};
use crate::store::{FileEntry, Store};
use crate::sync;
use crate::ui::input::LineEditor;
use crate::ui::tree;

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

#[derive(Debug, Default, Clone, Copy)]
struct StatusCounts {
    synced: usize,
    local_newer: usize,
    remote_newer: usize,
    conflict: usize,
    not_gisted: usize,
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
    #[allow(dead_code)] // used by the upcoming ignore-pattern wiring (task #54)
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

    pub fn rebuild_tree(&mut self) {
        self.apply_sort();
        let built = tree::build_tree(&self.files, &self.status_cache, &self.git_statuses);
        self.tree_items = built.items;
        self.tree_identifiers = built.identifiers;
        self.tree_file_ids = built.file_ids;
    }

    /// Reorder `self.files` per the active sort mode. Status sort needs
    /// store access, which is why this lives on App rather than in scanner.
    fn apply_sort(&mut self) {
        use crate::config::SortMode;
        match self.config.sort.mode {
            SortMode::MtimeDesc => {
                self.files.sort_by_key(|f| std::cmp::Reverse(f.modified));
            }
            SortMode::MtimeAsc => {
                self.files.sort_by_key(|f| f.modified);
            }
            SortMode::AlphaAsc => {
                self.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
            }
            SortMode::AlphaDesc => {
                self.files.sort_by(|a, b| b.rel_path.cmp(&a.rel_path));
            }
            SortMode::Status => {
                // Status rank uses the cached per-file sync state — no
                // disk reads inside the sort comparator.
                let cache = &self.status_cache;
                self.files.sort_by(|a, b| {
                    let sa = status_rank_cached(cache.get(&a.rel_path).copied());
                    let sb = status_rank_cached(cache.get(&b.rel_path).copied());
                    sa.cmp(&sb).then_with(|| b.modified.cmp(&a.modified))
                });
            }
        }
    }

    pub fn refresh_files(&mut self) -> Result<()> {
        if let Some(entry) = self.current_root_entry().cloned() {
            // Surface a missing root explicitly. Without this check the
            // scanner silently returns an empty list (its error tolerance is
            // tuned for "this subdir vanished mid-walk," not "the root
            // itself is gone") and the user sees an empty tree with no
            // explanation.
            if !entry.path.exists() {
                self.status_message =
                    format!("Root missing: {} (check config.toml)", entry.path.display());
                self.files.clear();
                self.status_cache.clear();
                self.git_statuses.clear();
                self.git_repo_root = None;
                self.rebuild_tree();
                return Ok(());
            }
            let ignore = scanner::build_globset(&entry.ignore);
            self.files = scanner::scan_directory(&entry.path, &ignore)?;
        } else {
            self.files.clear();
        }
        self.refresh_status_cache();
        self.refresh_git_status();
        self.rebuild_tree();
        self.update_preview();
        Ok(())
    }

    /// Read each scanned file once, compute its sync status, and cache the
    /// result. Subsequent calls in this refresh cycle (tree render, sort,
    /// dashboard counts, bulk menu) read from the cache instead of repeating
    /// the IO. Must be called *after* `self.files` is populated.
    fn refresh_status_cache(&mut self) {
        self.status_cache.clear();
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        self.status_cache.reserve(self.files.len());
        for f in &self.files {
            let entry = self.store.get(&root, &f.rel_path);
            let status = if entry.is_some() {
                let content = std::fs::read_to_string(&f.abs_path).unwrap_or_default();
                sync::local_status(&content, entry)
            } else {
                sync::SyncStatus::NotGisted
            };
            self.status_cache.insert(f.rel_path.clone(), status);
        }
    }

    /// Recompute one file's cached sync status (one disk read) after a store
    /// entry changed — push/pull/check results — without rescanning the tree.
    fn refresh_status_for(&mut self, rel_path: &str) {
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        let entry = self.store.get(&root, rel_path);
        let status = if entry.is_some() {
            let content = std::fs::read_to_string(self.abs_path(rel_path)).unwrap_or_default();
            sync::local_status(&content, entry)
        } else {
            sync::SyncStatus::NotGisted
        };
        self.status_cache.insert(rel_path.to_string(), status);
    }

    /// Cache lookup; defaults to NotGisted for paths not in the cache.
    pub fn cached_status(&self, rel_path: &str) -> sync::SyncStatus {
        self.status_cache
            .get(rel_path)
            .copied()
            .unwrap_or(sync::SyncStatus::NotGisted)
    }

    /// Re-query git for the active root's state. Quietly clears the status
    /// map if the root isn't in a repo or if git isn't on PATH.
    fn refresh_git_status(&mut self) {
        self.git_statuses.clear();
        self.git_repo_root = None;
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        let Some(repo) = crate::git::find_repo_root(&root) else {
            return;
        };
        let raw = crate::git::status(&repo);
        // raw is keyed by repo-relative path. Translate to active-root-relative
        // for tree lookups: if root == repo, this is a no-op; if root is a
        // subdirectory of repo, strip the prefix and keep matching entries.
        let prefix = root.strip_prefix(&repo).ok().map(|p| {
            let mut s = p.to_string_lossy().to_string();
            if !s.is_empty() && !s.ends_with('/') {
                s.push('/');
            }
            s
        });
        for (path, st) in raw {
            let rel = match &prefix {
                Some(p) if !p.is_empty() => match path.strip_prefix(p.as_str()) {
                    Some(r) => r.to_string(),
                    None => continue, // entry outside our scanned root
                },
                _ => path,
            };
            self.git_statuses.insert(rel, st);
        }
        self.git_repo_root = Some(repo);
    }

    /// Switch to a different root by index.
    fn switch_root(&mut self, index: usize) {
        if index < self.config.roots.len() {
            self.active_root = index;
            if let Err(e) = self.refresh_files() {
                self.status_message = format!("Refresh error: {e}");
            }
            self.update_status();
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

    pub fn update_preview(&mut self) {
        if let Some(ref rel) = self.selected_file() {
            let path = self.abs_path(rel);
            self.preview_content = std::fs::read_to_string(&path).unwrap_or_default();
        } else {
            self.preview_content.clear();
        }
        // Reset scroll whenever the visible content changes.
        self.preview_scroll = 0;
    }

    /// Handle a terminal mouse event. Left-click selects (in the tree) or
    /// switches focus (right pane); wheel scroll is routed to whichever pane
    /// the cursor is over.
    pub fn handle_mouse(&mut self, event: MouseEvent) {
        let over_tree = rect_contains(&self.tree_pane_rect, event.column, event.row);
        let over_right = rect_contains(&self.right_pane_rect, event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollDown => {
                if over_tree {
                    self.tree_state.scroll_down(3);
                } else {
                    self.scroll_right_pane(3, true);
                }
            }
            MouseEventKind::ScrollUp => {
                if over_tree {
                    self.tree_state.scroll_up(3);
                } else {
                    self.scroll_right_pane(3, false);
                }
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                if over_tree {
                    let pos = ratatui::layout::Position {
                        x: event.column,
                        y: event.row,
                    };
                    if self.tree_state.click_at(pos) {
                        self.focused_pane = PaneFocus::Tree;
                        self.update_preview();
                        self.update_status();
                    }
                } else if over_right {
                    self.focused_pane = PaneFocus::Right;
                }
            }
            _ => {}
        }
    }

    /// Scroll the right pane (preview or diff) by `lines` rows. `down=true`
    /// increases the offset.
    fn scroll_right_pane(&mut self, lines: u16, down: bool) {
        let target = if matches!(self.mode, Mode::Diff { .. }) {
            &mut self.diff_scroll
        } else {
            &mut self.preview_scroll
        };
        if down {
            *target = target.saturating_add(lines);
        } else {
            *target = target.saturating_sub(lines);
        }
    }

    pub fn update_status(&mut self) {
        let current_root = self.current_root().cloned();
        let g = crate::glyphs::glyphs();
        if let Some(ref rel) = self.selected_file() {
            let entry = current_root
                .as_ref()
                .and_then(|r| self.store.get(r, rel))
                .cloned();
            // Cached status — populated in refresh_files. One disk read per
            // file per refresh instead of one per status-bar update.
            let status = self.cached_status(rel);
            self.status_color = status.color();
            let url = entry.as_ref().map(|e| e.url.as_str()).unwrap_or("no gist");
            self.status_message = format!("{} {} | {url}", status.icon(), rel);

            let status_color = status.color();
            let mut spans = vec![
                Span::styled(
                    format!("{} ", status.icon()),
                    Style::default().fg(status_color),
                ),
                Span::styled(
                    rel.to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
            ];
            if entry.is_some() {
                spans.push(Span::styled(
                    url.to_string(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::UNDERLINED),
                ));
            } else {
                spans.push(Span::styled(
                    "no gist".to_string(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            self.status_spans = spans;
        } else {
            let counts = self.status_counts();
            self.status_color = if counts.conflict > 0 {
                Color::Red
            } else if counts.local_newer > 0 {
                Color::Yellow
            } else if counts.remote_newer > 0 {
                Color::Blue
            } else if counts.not_gisted > 0 {
                Color::DarkGray
            } else {
                Color::Green
            };
            let root_label = current_root
                .map(|r| r.display().to_string())
                .unwrap_or_else(|| "(no root)".into());
            self.status_message = format!(
                "{} {root_label}  |  {} {}  {} {}  {} {}  {} {}  {} {}",
                g.root,
                g.status_synced,
                counts.synced,
                g.status_local_newer,
                counts.local_newer,
                g.status_remote_newer,
                counts.remote_newer,
                g.status_conflict,
                counts.conflict,
                g.status_not_gisted,
                counts.not_gisted,
            );

            // Rich dashboard: root in magenta, each count colored by category.
            // Zero counts dim to keep the eye on what actually needs attention.
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(24);
            spans.push(Span::styled(
                format!("{} ", g.root),
                Style::default().fg(Color::Magenta),
            ));
            spans.push(Span::styled(
                root_label,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled("  │  ", Style::default().fg(Color::DarkGray)));
            let cells = [
                (
                    g.status_synced,
                    sync::SyncStatus::Synced.color(),
                    counts.synced,
                ),
                (
                    g.status_local_newer,
                    sync::SyncStatus::LocalNewer.color(),
                    counts.local_newer,
                ),
                (
                    g.status_remote_newer,
                    sync::SyncStatus::RemoteNewer.color(),
                    counts.remote_newer,
                ),
                (
                    g.status_conflict,
                    sync::SyncStatus::Conflict.color(),
                    counts.conflict,
                ),
                (
                    g.status_not_gisted,
                    sync::SyncStatus::NotGisted.color(),
                    counts.not_gisted,
                ),
            ];
            for (i, (icon, color, count)) in cells.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("  "));
                }
                if *count == 0 {
                    spans.push(Span::styled(
                        format!("{icon} {count}"),
                        Style::default().fg(Color::DarkGray),
                    ));
                } else {
                    spans.push(Span::styled(
                        format!("{icon} "),
                        Style::default().fg(*color),
                    ));
                    spans.push(Span::styled(
                        count.to_string(),
                        Style::default().fg(*color).add_modifier(Modifier::BOLD),
                    ));
                }
            }
            self.status_spans = spans;
        }
    }

    /// Tally the current set of files by sync status. Used by the status bar
    /// dashboard. Reads from `status_cache` so no disk IO happens here.
    fn status_counts(&self) -> StatusCounts {
        let mut c = StatusCounts::default();
        if self.current_root().is_none() {
            c.not_gisted = self.files.len();
            return c;
        }
        for file in &self.files {
            match self.cached_status(&file.rel_path) {
                sync::SyncStatus::Synced => c.synced += 1,
                sync::SyncStatus::LocalNewer => c.local_newer += 1,
                sync::SyncStatus::RemoteNewer => c.remote_newer += 1,
                sync::SyncStatus::Conflict => c.conflict += 1,
                sync::SyncStatus::NotGisted => c.not_gisted += 1,
            }
        }
        c
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match &self.mode {
            Mode::Help | Mode::Message(_) => {
                self.mode = Mode::Normal;
            }
            Mode::FilePicker { .. } => self.handle_picker_key(key),
            Mode::GdocUrl => self.handle_gdoc_url_key(key),
            Mode::GdocFilename => self.handle_gdoc_filename_key(key),
            Mode::Confirm { .. } => self.handle_confirm_key(key),
            Mode::Hydrating { .. } => self.handle_hydrating_key(),
            Mode::Diff { .. } => self.handle_diff_key(key),
            Mode::ResolveAmbiguous { .. } => self.handle_resolve_ambiguous_key(key),
            Mode::RootSwitcher { .. } => self.handle_root_switcher_key(key),
            Mode::SetupRoot | Mode::AddRoot => self.handle_setup_or_add_root_key(key),
            Mode::ReplaceQuery => self.handle_replace_query_key(key),
            Mode::ReplaceTarget => self.handle_replace_target_key(key),
            Mode::ReplaceReview { .. } => self.handle_replace_review_key(key),
            Mode::Rename { .. } => self.handle_rename_key(key),
            Mode::SortMenu { .. } => self.handle_sort_menu_key(key),
            Mode::BulkMenu { .. } => self.handle_bulk_menu_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_diff_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::PageDown => self.scroll_right_pane(10, true),
            KeyCode::PageUp => self.scroll_right_pane(10, false),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_right_pane(1, true),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_right_pane(1, false),
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            // Anything else exits diff.
            _ => self.mode = Mode::Normal,
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let Mode::FilePicker { selected } = self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                if let Some(m) = self.picker_matches.get(selected) {
                    let rel_path = m.rel_path.clone();
                    self.mode = Mode::Normal;
                    self.jump_to(&rel_path);
                } else {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Down => {
                let max = self.picker_matches.len().saturating_sub(1);
                self.mode = Mode::FilePicker {
                    selected: (selected + 1).min(max),
                };
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let max = self.picker_matches.len().saturating_sub(1);
                self.mode = Mode::FilePicker {
                    selected: (selected + 1).min(max),
                };
            }
            KeyCode::Up => {
                self.mode = Mode::FilePicker {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = Mode::FilePicker {
                    selected: selected.saturating_sub(1),
                };
            }
            _ => {
                if self.picker_editor.handle_key(key) {
                    self.refresh_picker();
                }
            }
        }
    }

    /// Re-rank `self.files` against the picker editor's current content and
    /// clamp the selection cursor.
    fn refresh_picker(&mut self) {
        let query = self.picker_editor.content.clone();
        self.picker_matches = self.picker.rank(&self.files, &query);
        let max = self.picker_matches.len().saturating_sub(1);
        if let Mode::FilePicker { selected } = &mut self.mode {
            *selected = (*selected).min(max);
        }
    }

    /// Select the given rel_path in the tree, expanding *every* ancestor
    /// directory so the leaf is visible. Also focuses the tree pane.
    ///
    /// Note: tui-tree-widget's `open()` and `select()` both take the *full
    /// path from root* to the target node, not just the target's own id.
    /// Passing a single-element vec only worked for top-level entries.
    fn jump_to(&mut self, rel_path: &str) {
        // Cumulative identifiers, one per depth level:
        //   "a/b/c/d.md" → ["a", "a/b", "a/b/c", "a/b/c/d.md"]
        let mut full_path: Vec<String> = Vec::new();
        let mut acc = String::new();
        for part in rel_path.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            full_path.push(acc.clone());
        }
        // Open each ancestor with the full path leading to it.
        for depth in 1..full_path.len() {
            self.tree_state.open(full_path[..depth].to_vec());
        }
        self.tree_state.select(full_path);
        self.tree_state.scroll_selected_into_view();
        self.focused_pane = PaneFocus::Tree;
        self.update_preview();
        self.update_status();
    }

    fn handle_gdoc_url_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let url = self.input_editor.content.clone();
                self.start_gdoc_fetch(&url);
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn handle_gdoc_filename_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.gdoc_content = None;
            }
            KeyCode::Enter => self.save_gdoc_import(),
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        let Mode::Confirm { action, .. } = &self.mode else {
            return;
        };
        let action_copy = action.clone();
        let confirmed = matches!(
            key.code,
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter
        );
        let cancelled = matches!(
            key.code,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q')
        );
        // Stray keys (spacebar, arrow keys, random letters) leave the dialog
        // alone — only an explicit confirm/cancel dismisses.
        if !confirmed && !cancelled {
            return;
        }
        if confirmed {
            match action_copy {
                ConfirmAction::SyncDown => self.do_sync_down(),
                ConfirmAction::ForcePush { rel_path } => {
                    self.do_sync_up_for(rel_path, true);
                }
                ConfirmAction::DeleteRemote {
                    rel_path,
                    root,
                    gist_id,
                } => self.do_delete_remote(rel_path, root, gist_id),
                ConfirmAction::TrashLocal { rel_path, root } => {
                    self.do_trash_local(rel_path, root);
                }
                ConfirmAction::Bulk(action) => {
                    self.run_bulk(action);
                }
                ConfirmAction::RunShell { cmd } => {
                    self.pending_alias = Some(cmd);
                }
            }
        }
        self.mode = Mode::Normal;
    }

    fn handle_hydrating_key(&mut self) {
        let Mode::Hydrating { done, .. } = &self.mode else {
            return;
        };
        if !*done {
            return;
        }
        if let Err(e) = self.refresh_files() {
            self.status_message = format!("Refresh error: {e}");
        }
        self.update_status();
        if !self.pending_ambiguous.is_empty() {
            self.mode = Mode::ResolveAmbiguous {
                item: 0,
                selected: 0,
            };
        } else {
            self.mode = Mode::Normal;
        }
    }

    fn handle_resolve_ambiguous_key(&mut self, key: KeyEvent) {
        let Mode::ResolveAmbiguous { item, selected } = &self.mode else {
            return;
        };
        let item = *item;
        let selected = *selected;
        let total_items = self.pending_ambiguous.len();
        let candidates = self
            .pending_ambiguous
            .get(item)
            .map(|m| m.candidates.len())
            .unwrap_or(0);
        let advance = |this: &mut Self| {
            let next = item + 1;
            if next < total_items {
                this.mode = Mode::ResolveAmbiguous {
                    item: next,
                    selected: 0,
                };
            } else {
                this.pending_ambiguous.clear();
                this.mode = Mode::Normal;
                this.rebuild_tree();
                this.update_status();
                this.status_message = "Ambiguous resolution complete.".into();
            }
        };
        match key.code {
            KeyCode::Esc => {
                self.pending_ambiguous.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down if candidates > 0 => {
                self.mode = Mode::ResolveAmbiguous {
                    item,
                    selected: (selected + 1).min(candidates - 1),
                };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::ResolveAmbiguous {
                    item,
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Char('s') => advance(self),
            KeyCode::Enter if candidates > 0 => {
                self.apply_ambiguous_pick(item, selected);
                advance(self);
            }
            _ => {}
        }
    }

    fn handle_root_switcher_key(&mut self, key: KeyEvent) {
        let Mode::RootSwitcher { selected } = &self.mode else {
            return;
        };
        let sel = *selected;
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.config.roots.len().saturating_sub(1);
                self.mode = Mode::RootSwitcher {
                    selected: (sel + 1).min(max),
                };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::RootSwitcher {
                    selected: sel.saturating_sub(1),
                };
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.switch_root(sel);
            }
            KeyCode::Char('a') => {
                self.input_editor = LineEditor::new();
                self.mode = Mode::AddRoot;
            }
            KeyCode::Char('d') if sel < self.config.roots.len() => {
                if let Err(e) = self.config.remove_root(sel) {
                    self.status_message = format!("Remove root failed: {e}");
                    return;
                }
                if self.config.roots.is_empty() {
                    self.active_root = 0;
                    self.files.clear();
                    self.rebuild_tree();
                    self.input_editor = LineEditor::new();
                    self.mode = Mode::SetupRoot;
                } else {
                    if self.active_root >= self.config.roots.len() {
                        self.active_root = self.config.roots.len() - 1;
                    }
                    let new_sel = sel.min(self.config.roots.len().saturating_sub(1));
                    self.mode = Mode::RootSwitcher { selected: new_sel };
                    if let Err(e) = self.refresh_files() {
                        self.status_message = format!("Refresh error: {e}");
                    }
                    self.update_status();
                }
            }
            _ => {}
        }
    }

    fn handle_setup_or_add_root_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if matches!(self.mode, Mode::AddRoot) {
                    self.mode = Mode::RootSwitcher {
                        selected: self.active_root,
                    };
                }
                // SetupRoot: no escape — must configure a root or Ctrl+Q to quit.
            }
            KeyCode::Enter => {
                let raw = self.input_editor.content.trim().to_string();
                if raw.is_empty() {
                    return;
                }
                let expanded = expand_tilde(&raw);
                if !expanded.is_dir() {
                    self.mode = Mode::Message(format!("Not a directory: {}", expanded.display()));
                    return;
                }
                if let Err(e) = self.config.add_root(expanded) {
                    self.mode = Mode::Message(format!("Add root failed: {e}"));
                    return;
                }
                self.active_root = self.config.roots.len() - 1;
                if let Err(e) = self.refresh_files() {
                    self.status_message = format!("Refresh error: {e}");
                }
                self.update_status();
                self.mode = Mode::Normal;
            }
            KeyCode::Char('q')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(self.mode, Mode::SetupRoot) =>
            {
                self.should_quit = true;
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('/') => {
                self.picker_editor = LineEditor::new();
                self.mode = Mode::FilePicker { selected: 0 };
                self.refresh_picker();
            }
            KeyCode::Tab => {
                self.focused_pane = match self.focused_pane {
                    PaneFocus::Tree => PaneFocus::Right,
                    PaneFocus::Right => PaneFocus::Tree,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => self.nav_down(),
            KeyCode::Char('k') | KeyCode::Up => self.nav_up(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if self.focused_pane == PaneFocus::Tree =>
            {
                self.tree_state.toggle_selected();
                self.update_preview();
                self.update_status();
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace
                if self.focused_pane == PaneFocus::Tree =>
            {
                self.tree_state.key_left();
                self.update_preview();
                self.update_status();
            }
            KeyCode::Char('u') => self.do_sync_up(),
            KeyCode::Char('d') => self.confirm_sync_down(),
            KeyCode::Char('c') => self.do_copy_url(),
            KeyCode::Char('C') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.do_copy_file_contents();
            }
            KeyCode::Char('V') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.do_paste_rich();
            }
            KeyCode::Char('e') => self.do_request_edit(),
            KeyCode::Char('o') => self.do_open_in_browser(),
            KeyCode::Char('X') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.confirm_delete_remote();
            }
            KeyCode::Char('n') => self.jump_to_next_dirty(true),
            KeyCode::Char('N') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jump_to_next_dirty(false);
            }
            KeyCode::Char('D') if !key.modifiers.contains(KeyModifiers::CONTROL) => self.do_diff(),
            KeyCode::Char('_') => self.confirm_trash_local(),
            KeyCode::Char('m') => self.start_rename(),
            KeyCode::Char('O') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_sort_menu();
            }
            KeyCode::Char('B') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_bulk_menu();
            }
            KeyCode::Char('g') => self.do_git_status(),
            KeyCode::Char('G') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.do_git_log();
            }
            KeyCode::Char('(') => self.confirm_git_pull(),
            KeyCode::Char(')') => self.confirm_git_push(),
            KeyCode::Char('=') => self.do_format_in_place(),
            KeyCode::Char('s') => self.start_replace(),
            KeyCode::Char('H') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_hydration();
            }
            KeyCode::Char('f') => self.start_remote_check(),
            KeyCode::Char('r') => {
                if let Err(e) = self.refresh_files() {
                    self.status_message = format!("Refresh error: {e}");
                } else {
                    self.status_message = "Refreshed.".into();
                }
            }
            KeyCode::Char('R') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = Mode::RootSwitcher {
                    selected: self.active_root,
                };
            }
            KeyCode::Char('I') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input_editor = LineEditor::new();
                self.mode = Mode::GdocUrl;
            }
            KeyCode::PageDown => self.scroll_right_pane(10, true),
            KeyCode::PageUp => self.scroll_right_pane(10, false),
            // Last: try user-configured aliases. Only single-char keys
            // without Ctrl held can fire an alias (Ctrl-letter combinations
            // are reserved for future use and would surprise users).
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(cmd) = self.config.aliases.get(&c.to_string()).cloned() {
                    self.pending_alias = Some(cmd);
                }
            }
            _ => {}
        }
    }

    fn nav_down(&mut self) {
        match self.focused_pane {
            PaneFocus::Tree => {
                self.tree_state.key_down();
                self.update_preview();
                self.update_status();
            }
            PaneFocus::Right => self.scroll_right_pane(1, true),
        }
    }

    fn nav_up(&mut self) {
        match self.focused_pane {
            PaneFocus::Tree => {
                self.tree_state.key_up();
                self.update_preview();
                self.update_status();
            }
            PaneFocus::Right => self.scroll_right_pane(1, false),
        }
    }

    fn confirm_sync_down(&mut self) {
        let Some(ref rel) = self.selected_file() else {
            return;
        };
        let has_entry = self
            .current_root()
            .map(|r| self.store.get(r, rel).is_some())
            .unwrap_or(false);
        if has_entry {
            self.mode = Mode::Confirm {
                message: format!(
                    "Pull remote content for {rel}? Local changes will be overwritten."
                ),
                action: ConfirmAction::SyncDown,
            };
        } else {
            self.status_message = "No gist mapped for this file.".into();
        }
    }

    pub fn handle_async_event(&mut self, event: AsyncEvent) {
        match event {
            AsyncEvent::PushDone {
                root,
                rel_path,
                result,
            } => match result {
                Ok(entry) => {
                    let url = entry.url.clone();
                    self.store.insert(&root, rel_path.clone(), entry);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Push ok but store save failed: {e}");
                        return;
                    }
                    self.refresh_status_for(&rel_path);
                    self.rebuild_tree();
                    self.status_message = format!("Pushed {rel_path} → {url}");
                    // Follow up on a queued copy-url request, if it's for this file.
                    if self.pending_copy.as_ref().is_some_and(|p| *p == rel_path) {
                        self.pending_copy = None;
                        self.copy_to_clipboard(&url);
                    }
                }
                Err(e) => {
                    self.pending_copy = None;
                    self.status_message = format!("Push failed: {e}");
                }
            },
            AsyncEvent::PullDone {
                root,
                rel_path,
                expected_local_sha256,
                result,
            } => match result {
                Ok((content, entry)) => {
                    let path = root.join(&rel_path);
                    // Refuse to clobber edits made while the pull was in
                    // flight (e.g. via `e` between confirm and completion).
                    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
                    if sync::sha256_hex(&on_disk) != expected_local_sha256 {
                        self.status_message = format!(
                            "{rel_path} changed on disk during pull — not overwriting. Pull again to retry."
                        );
                        return;
                    }
                    if let Err(e) = std::fs::write(&path, &content) {
                        self.status_message = format!("Write failed: {e}");
                        return;
                    }
                    self.store.insert(&root, rel_path.clone(), entry);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Pull ok but store save failed: {e}");
                        return;
                    }
                    self.refresh_status_for(&rel_path);
                    self.update_preview();
                    self.rebuild_tree();
                    self.status_message = format!("Pulled {rel_path}");
                }
                Err(e) => {
                    self.status_message = format!("Pull failed: {e}");
                }
            },
            AsyncEvent::PushBlocked {
                root,
                rel_path,
                remote_sha256,
                remote_updated_at,
            } => {
                // Record the observed divergence so the tree shows the real
                // state (RemoteNewer/Conflict) even if the user declines.
                if let Some(entry) = self.store.get(&root, &rel_path).cloned() {
                    let mut updated = entry;
                    updated.remote_sha256 = remote_sha256;
                    updated.remote_updated_at = Some(remote_updated_at);
                    self.store.insert(&root, rel_path.clone(), updated);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Store save failed: {e}");
                    }
                    self.refresh_status_for(&rel_path);
                    self.rebuild_tree();
                    self.update_status();
                }
                self.mode = Mode::Confirm {
                    message: format!(
                        "Remote gist for {rel_path} changed since last sync. Force push (overwrites remote — use D to diff first)?"
                    ),
                    action: ConfirmAction::ForcePush {
                        rel_path: rel_path.clone(),
                    },
                };
                self.status_message = format!("Push blocked: remote changed for {rel_path}");
            }
            AsyncEvent::RemoteCheckProgress { done, total } => {
                self.status_message = format!("Checking remote... {done}/{total}");
            }
            AsyncEvent::RemoteCheckDone {
                root,
                started,
                result,
            } => match result {
                Ok(outcome) => {
                    let mut applied = 0usize;
                    for (rel, refreshed) in outcome.updated {
                        let current = self.store.get(&root, &rel);
                        if crate::remote::should_apply_update(current, &refreshed, started) {
                            self.store.insert(&root, rel.clone(), refreshed);
                            self.refresh_status_for(&rel);
                            applied += 1;
                        }
                    }
                    if applied > 0
                        && let Err(e) = self.store.save()
                    {
                        self.status_message = format!("Remote check: store save failed: {e}");
                        return;
                    }
                    self.rebuild_tree();
                    self.update_status();
                    let mut msg = format!(
                        "Remote check: {} mapped file(s), {} changed remotely",
                        outcome.checked, outcome.divergent
                    );
                    if !outcome.missing.is_empty() {
                        msg.push_str(&format!(
                            ", {} gist(s) deleted remotely ({})",
                            outcome.missing.len(),
                            outcome.missing.join(", ")
                        ));
                    }
                    self.status_message = msg;
                }
                Err(e) => {
                    self.status_message = format!("Remote check failed: {e}");
                }
            },
            AsyncEvent::HydrationUpdate(progress) => {
                self.mode = Mode::Hydrating {
                    progress: Some(progress),
                    done: false,
                };
            }
            AsyncEvent::HydrationDone(result) => match result {
                Ok(data) => {
                    // Merge hydration's discovered mappings into the live store
                    // rather than reloading from disk (which would clobber any
                    // concurrent push/pull that completed during hydration).
                    self.store.merge_from(&data.store);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Hydration ok but save failed: {e}");
                    }
                    let matched = data.matched;
                    let ambiguous_count = data.ambiguous.len();
                    if let Mode::Hydrating { progress, done } = &mut self.mode {
                        if let Some(p) = progress {
                            p.phase = format!(
                                "Complete! Matched {matched} files, {ambiguous_count} ambiguous. Press any key."
                            );
                        }
                        *done = true;
                    }
                    // Stash ambiguous matches for the resolver UI (added later).
                    self.pending_ambiguous = data.ambiguous;
                }
                Err(e) => {
                    self.mode = Mode::Message(format!("Hydration error: {e}"));
                }
            },
            AsyncEvent::StatusCheck {
                root,
                rel_path,
                started,
                result,
            } => match result {
                Ok(full) => {
                    // A diff fetch is also a remote observation — persist it
                    // so the tree reflects any divergence it revealed. Skip
                    // if the entry synced while the fetch was in flight.
                    if let Some(entry) = self
                        .store
                        .get(&root, &rel_path)
                        .filter(|e| e.last_synced <= started)
                        .cloned()
                    {
                        let mut updated = entry;
                        updated.remote_sha256 = full.remote_sha256;
                        updated.remote_updated_at = Some(full.remote_updated_at);
                        self.store.insert(&root, rel_path.clone(), updated);
                        if let Err(e) = self.store.save() {
                            self.status_message = format!("Store save failed: {e}");
                        }
                        self.refresh_status_for(&rel_path);
                        self.rebuild_tree();
                    }
                    self.status_message = format!("{} {rel_path}", full.status.icon());
                    if let Mode::Diff { remote, .. } = &mut self.mode {
                        *remote = full.remote_content;
                    }
                }
                Err(e) => {
                    self.status_message = format!("Status check failed: {e}");
                }
            },
            AsyncEvent::DeleteDone {
                root,
                rel_path,
                result,
            } => match result {
                Ok(()) => {
                    if let Some(map) = self.store.roots.get_mut(&root) {
                        map.remove(&rel_path);
                    }
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Delete ok but store save failed: {e}");
                        return;
                    }
                    self.status_message = format!("Deleted gist for {rel_path}");
                    self.rebuild_tree();
                    self.update_status();
                }
                Err(e) => {
                    self.status_message = format!("Delete failed: {e}");
                }
            },
            AsyncEvent::RenameRemoteDone { rel_path, result } => match result {
                Ok(()) => {
                    self.status_message = format!("Renamed (local + remote): {rel_path}");
                }
                Err(e) => {
                    self.status_message = format!("Renamed locally; remote rename failed: {e}");
                }
            },
            AsyncEvent::GdocFetched(result) => match result {
                Ok(content) => {
                    self.gdoc_content = Some(content);
                    self.input_editor = LineEditor::new();
                    self.mode = Mode::GdocFilename;
                }
                Err(e) => {
                    self.mode = Mode::Message(format!("Google Doc error: {e}"));
                }
            },
        }
    }

    fn do_sync_up(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        self.do_sync_up_for(rel, false);
    }

    /// Inner push implementation, parameterized by rel_path so bulk-push
    /// can call it once per dirty file. Unless `force` is set, updates are
    /// refused when the remote changed since the last sync; a PushBlocked
    /// event then records the divergence and prompts for a force-push.
    fn do_sync_up_for(&mut self, rel: String, force: bool) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No active root.".into();
            return;
        };
        let path = self.abs_path(&rel);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Read {rel}: {e}");
                return;
            }
        };
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let store_snapshot = self.store.get(&root, &rel).cloned();
        let tx = self.async_tx.clone();
        let rel_clone = rel.clone();
        let root_clone = root.clone();

        self.status_message = format!("Pushing {rel}...");

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let result =
                sync::push(&client, store_snapshot.as_ref(), &filename, &content, force).await;
            let event = match result {
                Ok(sync::PushOutcome::Pushed(entry)) => AsyncEvent::PushDone {
                    root: root_clone,
                    rel_path: rel_clone,
                    result: Ok(entry),
                },
                Ok(sync::PushOutcome::RemoteChanged {
                    remote_sha256,
                    remote_updated_at,
                }) => AsyncEvent::PushBlocked {
                    root: root_clone,
                    rel_path: rel_clone,
                    remote_sha256,
                    remote_updated_at,
                },
                Err(e) => AsyncEvent::PushDone {
                    root: root_clone,
                    rel_path: rel_clone,
                    result: Err(e.to_string()),
                },
            };
            let _ = tx.send(event);
        });
    }

    fn do_sync_down(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        self.do_sync_down_for(rel);
    }

    /// Inner pull implementation, parameterized by rel_path so bulk-pull
    /// can call it once per remote-newer file.
    fn do_sync_down_for(&mut self, rel: String) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No active root.".into();
            return;
        };
        let Some(entry) = self.store.get(&root, &rel).cloned() else {
            self.status_message = format!("No gist mapped for {rel}.");
            return;
        };
        let filename = self
            .abs_path(&rel)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // Snapshot the local content's hash now; if the file changes on disk
        // while the pull is in flight, the PullDone handler refuses to
        // overwrite those edits.
        let local_now = std::fs::read_to_string(self.abs_path(&rel)).unwrap_or_default();
        let expected_local_sha256 = sync::sha256_hex(&local_now);
        let tx = self.async_tx.clone();
        let rel_clone = rel.clone();
        let root_clone = root.clone();

        self.status_message = format!("Pulling {rel}...");

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::pull(&client, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::PullDone {
                root: root_clone,
                rel_path: rel_clone,
                expected_local_sha256,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    fn do_request_edit(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let path = self.abs_path(&rel);
        if !path.is_file() {
            self.status_message = format!("Not a file: {}", path.display());
            return;
        }
        self.pending_editor = Some(path);
    }

    fn do_open_in_browser(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(entry) = self
            .current_root()
            .and_then(|r| self.store.get(r, &rel))
            .cloned()
        else {
            self.status_message = "No gist mapped for this file.".into();
            return;
        };
        match open::that(&entry.url) {
            Ok(()) => self.status_message = format!("Opened {}", entry.url),
            Err(e) => self.status_message = format!("Open failed: {e}"),
        }
    }

    fn confirm_delete_remote(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        let Some(entry) = self.store.get(&root, &rel).cloned() else {
            self.status_message = "No gist to delete.".into();
            return;
        };
        self.mode = Mode::Confirm {
            message: format!(
                "Delete remote gist for {rel}? Local file is kept; mapping is removed."
            ),
            action: ConfirmAction::DeleteRemote {
                rel_path: rel,
                root,
                gist_id: entry.gist_id,
            },
        };
    }

    fn do_delete_remote(&mut self, rel_path: String, root: PathBuf, gist_id: String) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let tx = self.async_tx.clone();
        self.status_message = format!("Deleting gist for {rel_path}...");
        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let result = client.delete(&gist_id).await.map_err(|e| e.to_string());
            let _ = tx.send(AsyncEvent::DeleteDone {
                root,
                rel_path,
                result,
            });
        });
    }

    /// Prompt to move the selected file to the OS trash. The remote gist (if
    /// any) is left intact — restore-from-trash + hydration will re-link it.
    fn confirm_trash_local(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        let has_gist = self.store.get(&root, &rel).is_some();
        let suffix = if has_gist {
            " The remote gist is kept; the mapping is dropped."
        } else {
            ""
        };
        self.mode = Mode::Confirm {
            message: format!("Move {rel} to the system trash?{suffix}"),
            action: ConfirmAction::TrashLocal {
                rel_path: rel,
                root,
            },
        };
    }

    /// Actually trash the file and prune its store mapping. Synchronous —
    /// `trash::delete` is a quick OS call.
    fn do_trash_local(&mut self, rel_path: String, root: PathBuf) {
        let abs = root.join(&rel_path);
        match trash::delete(&abs) {
            Ok(()) => {
                self.store.remove(&root, &rel_path);
                if let Err(e) = self.store.save() {
                    self.status_message = format!("Trashed but store save failed: {e}");
                    return;
                }
                if let Err(e) = self.refresh_files() {
                    self.status_message = format!("Trashed but refresh failed: {e}");
                    return;
                }
                self.status_message = format!("Moved {rel_path} to trash.");
            }
            Err(e) => {
                self.status_message = format!("Trash failed: {e}");
            }
        }
    }

    // ── Format in place ─────────────────────────────────────────────────────

    /// Toggle the selected file between canonical compact and pretty form,
    /// writing back to disk. Direction is decided by comparing the file to
    /// its compact serialization: if they match, prettify; otherwise compact.
    /// Any "almost compact" intermediate (stray spaces, etc.) is normalized
    /// to compact on the first press; a second press flips to pretty.
    /// Currently only `.json` is supported.
    fn do_format_in_place(&mut self) {
        let Some(rel) = self.selected_file() else {
            self.status_message = "No file selected.".into();
            return;
        };
        let ext = std::path::Path::new(&rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext != "json" {
            self.status_message = format!("Format not supported for .{ext} files.");
            return;
        }
        let abs = self.abs_path(&rel);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Read error: {e}");
                return;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                self.status_message = format!("Invalid JSON, not formatted: {e}");
                return;
            }
        };
        let compact = match serde_json::to_string(&value) {
            Ok(s) => s,
            Err(e) => {
                self.status_message = format!("Serialize failed: {e}");
                return;
            }
        };
        // The file is "canonical compact" iff it equals the compact form,
        // optionally with a single trailing newline.
        let is_compact = content == compact || content == format!("{compact}\n");

        let (new_text, verb) = if is_compact {
            let pretty = match serde_json::to_string_pretty(&value) {
                Ok(s) => s,
                Err(e) => {
                    self.status_message = format!("Serialize failed: {e}");
                    return;
                }
            };
            (format!("{pretty}\n"), "Prettified")
        } else {
            (format!("{compact}\n"), "Compacted")
        };

        if new_text == content {
            self.status_message = format!("{rel}: already canonical.");
            return;
        }
        let delta = new_text.len() as isize - content.len() as isize;
        if let Err(e) = std::fs::write(&abs, &new_text) {
            self.status_message = format!("Write failed: {e}");
            return;
        }
        if let Err(e) = self.refresh_files() {
            self.status_message = format!("Formatted but refresh failed: {e}");
            return;
        }
        self.update_preview();
        self.update_status();
        let sign = if delta >= 0 { "+" } else { "" };
        self.status_message = format!("{verb} {rel} ({sign}{delta} bytes)");
    }

    // ── Rename / move ───────────────────────────────────────────────────────

    fn start_rename(&mut self) {
        let Some(rel) = self.selected_file() else {
            self.status_message = "No file selected.".into();
            return;
        };
        self.input_editor = LineEditor::new();
        self.input_editor.content = rel.clone();
        self.input_editor.cursor = self.input_editor.content.len();
        self.mode = Mode::Rename { old_rel: rel };
    }

    fn handle_rename_key(&mut self, key: KeyEvent) {
        let Mode::Rename { old_rel } = &self.mode else {
            return;
        };
        let old_rel = old_rel.clone();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let new_rel = self.input_editor.content.trim().to_string();
                self.mode = Mode::Normal;
                self.do_rename(old_rel, new_rel);
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    /// Carry out the rename: validate, move on disk, update the store key,
    /// and (if mapped) kick off the remote gist filename update.
    fn do_rename(&mut self, old_rel: String, new_rel: String) {
        if new_rel.is_empty() {
            self.status_message = "Empty filename — rename cancelled.".into();
            return;
        }
        if new_rel == old_rel {
            self.status_message = "No change — rename cancelled.".into();
            return;
        }
        // Disallow absolute paths or backtracking — rename stays under the root.
        if new_rel.starts_with('/') || new_rel.split('/').any(|c| c == "..") {
            self.status_message =
                "New name must be a relative path under the root (no .. or leading /).".into();
            return;
        }
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No root.".into();
            return;
        };
        let old_abs = root.join(&old_rel);
        let new_abs = root.join(&new_rel);
        if new_abs.exists() {
            self.status_message = format!("Target exists: {new_rel}");
            return;
        }
        if let Some(parent) = new_abs.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            self.status_message = format!("mkdir failed: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&old_abs, &new_abs) {
            self.status_message = format!("Rename failed: {e}");
            return;
        }

        // Update store: move the entry under the new rel_path.
        let entry = self.store.get(&root, &old_rel).cloned();
        if let Some(entry) = &entry {
            self.store.remove(&root, &old_rel);
            self.store.insert(&root, new_rel.clone(), entry.clone());
            if let Err(e) = self.store.save() {
                self.status_message = format!("Renamed locally; store save failed: {e}");
                return;
            }
        }

        // Refresh and select the new path.
        if let Err(e) = self.refresh_files() {
            self.status_message = format!("Renamed; refresh failed: {e}");
            return;
        }
        self.jump_to(&new_rel);

        // If the file was mapped to a gist, push the rename remotely too.
        // GitHub's gist filename is just the basename (we set it that way on
        // upload), so we only need to touch the remote if the basename changed.
        if let Some(entry) = entry {
            let old_base = std::path::Path::new(&old_rel)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let new_base = std::path::Path::new(&new_rel)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if old_base != new_base {
                let Some(token) = self.token.clone() else {
                    self.status_message = "Renamed locally; no token to update remote gist.".into();
                    return;
                };
                let tx = self.async_tx.clone();
                let new_rel_clone = new_rel.clone();
                self.status_message = "Renamed locally; updating remote gist...".to_string();
                let gist_id = entry.gist_id.clone();
                self.spawn_tracked(async move {
                    let client = gist_rs::GistClient::new(token);
                    let result = client
                        .rename_file(&gist_id, &old_base, &new_base)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AsyncEvent::RenameRemoteDone {
                        rel_path: new_rel_clone,
                        result,
                    });
                });
            } else {
                self.status_message = format!("Renamed: {old_rel} → {new_rel}");
            }
        } else {
            self.status_message = format!("Renamed: {old_rel} → {new_rel}");
        }
    }

    // ── Find-and-replace flow ───────────────────────────────────────────────

    /// Scope used by find-and-replace: the directory the user is currently
    /// "inside" in the tree. A selected file → its parent; a selected dir →
    /// that dir; nothing meaningful → the active root.
    pub fn replace_scope(&self) -> Option<PathBuf> {
        let root = self.current_root()?.clone();
        if let Some(rel) = self.selected_file() {
            let p = std::path::Path::new(&rel);
            return Some(p.parent().map(|p| root.join(p)).unwrap_or(root));
        }
        let selected = self.tree_state.selected();
        if let Some(id) = selected.last() {
            // A directory id is a rel_path like "rp-posts" or "RHoD/rp-posts".
            if !self.tree_file_ids.contains(id) {
                return Some(root.join(id));
            }
        }
        Some(root)
    }

    /// Display label for the scope — e.g. "Red Hand of Doom/rp-posts" or
    /// "(root)" for the active root itself.
    pub fn replace_scope_label(&self) -> String {
        let Some(root) = self.current_root() else {
            return "(no root)".into();
        };
        let Some(scope) = self.replace_scope() else {
            return "(no root)".into();
        };
        let rel = scope.strip_prefix(root).unwrap_or(&scope);
        let s = rel.to_string_lossy();
        if s.is_empty() {
            "(root)".into()
        } else {
            s.into_owned()
        }
    }

    fn start_replace(&mut self) {
        if self.current_root().is_none() {
            self.status_message = "No root directory configured.".into();
            return;
        }
        self.replace_query.clear();
        self.replace_target.clear();
        self.replace_matches.clear();
        self.replace_checked.clear();
        self.input_editor = LineEditor::new();
        self.mode = Mode::ReplaceQuery;
    }

    fn handle_replace_query_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let q = self.input_editor.content.trim().to_string();
                if q.is_empty() {
                    self.status_message = "Search string cannot be empty.".into();
                    self.mode = Mode::Normal;
                    return;
                }
                self.replace_query = q;
                self.input_editor = LineEditor::new();
                self.mode = Mode::ReplaceTarget;
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn handle_replace_target_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                // Target may legitimately be empty (delete the matches), so
                // we don't reject "".
                self.replace_target = self.input_editor.content.clone();
                self.run_replace_scan();
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn run_replace_scan(&mut self) {
        let Some(scope) = self.replace_scope() else {
            self.status_message = "No scope available.".into();
            self.mode = Mode::Normal;
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No root.".into();
            self.mode = Mode::Normal;
            return;
        };
        let matches = crate::replace::scan(&scope, &root, &self.replace_query);
        if matches.is_empty() {
            self.status_message = format!(
                "No matches for '{}' in {}",
                self.replace_query,
                self.replace_scope_label()
            );
            self.mode = Mode::Normal;
            return;
        }
        let n = matches.len();
        self.replace_checked = vec![true; n];
        self.replace_matches = matches;
        self.status_message = format!("Found {n} matches — review and apply.");
        self.mode = Mode::ReplaceReview { selected: 0 };
    }

    fn handle_replace_review_key(&mut self, key: KeyEvent) {
        let Mode::ReplaceReview { selected } = self.mode else {
            return;
        };
        let n = self.replace_matches.len();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status_message = "Replace cancelled.".into();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::ReplaceReview {
                    selected: (selected + 1).min(n.saturating_sub(1)),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::ReplaceReview {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Char(' ') => {
                if let Some(c) = self.replace_checked.get_mut(selected) {
                    *c = !*c;
                }
            }
            KeyCode::Char('a') => {
                for c in self.replace_checked.iter_mut() {
                    *c = true;
                }
            }
            KeyCode::Char('z') => {
                for c in self.replace_checked.iter_mut() {
                    *c = false;
                }
            }
            KeyCode::Enter => self.apply_replace(),
            _ => {}
        }
    }

    fn apply_replace(&mut self) {
        let to_apply: Vec<crate::replace::ReplaceMatch> = self
            .replace_matches
            .iter()
            .zip(self.replace_checked.iter())
            .filter_map(|(m, c)| if *c { Some(m.clone()) } else { None })
            .collect();
        if to_apply.is_empty() {
            self.status_message = "Nothing checked — no changes applied.".into();
            self.mode = Mode::Normal;
            return;
        }
        let result = crate::replace::apply(&to_apply, &self.replace_query, &self.replace_target);
        let n_files = result.files_changed.len();
        let mut msg = format!(
            "Replaced {} of {} ({} file{})",
            result.applied,
            to_apply.len(),
            n_files,
            if n_files == 1 { "" } else { "s" }
        );
        if result.drifted > 0 {
            msg.push_str(&format!(" — {} skipped (file changed)", result.drifted));
        }
        if !result.errors.is_empty() {
            msg.push_str(&format!(" — {} write error(s)", result.errors.len()));
        }
        if let Err(e) = self.refresh_files() {
            msg.push_str(&format!(" — refresh failed: {e}"));
        }
        self.update_preview();
        self.update_status();
        self.status_message = msg;
        self.replace_matches.clear();
        self.replace_checked.clear();
        self.mode = Mode::Normal;
    }

    /// Move the tree selection to the next file whose sync status is anything
    /// other than `Synced`. `forward` controls direction. Wraps once.
    fn jump_to_next_dirty(&mut self, forward: bool) {
        if self.current_root().is_none() {
            return;
        }
        if self.files.is_empty() {
            return;
        }

        // Find current position within the scanned-files list. If the current
        // selection is a directory or nothing, start before the first file.
        let current_id = self
            .tree_state
            .selected()
            .last()
            .cloned()
            .unwrap_or_default();
        let cur_idx = self
            .files
            .iter()
            .position(|f| f.rel_path == current_id)
            .map(|i| i as isize)
            .unwrap_or(-1);

        let n = self.files.len() as isize;
        let mut next = None;
        for step in 1..=n {
            let probe = if forward {
                ((cur_idx + step).rem_euclid(n)) as usize
            } else {
                ((cur_idx - step).rem_euclid(n)) as usize
            };
            let file = &self.files[probe];
            if !matches!(self.cached_status(&file.rel_path), sync::SyncStatus::Synced) {
                next = Some(file.rel_path.clone());
                break;
            }
        }

        let Some(rel_path) = next else {
            self.status_message = "No dirty files.".into();
            return;
        };
        self.jump_to(&rel_path);
    }

    fn do_copy_url(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let entry = self
            .current_root()
            .and_then(|r| self.store.get(r, &rel))
            .cloned();
        if let Some(entry) = entry {
            self.copy_to_clipboard(&entry.url.clone());
        } else {
            // Queue the copy and trigger a push; PushDone will follow up.
            self.pending_copy = Some(rel.clone());
            self.do_sync_up();
            self.status_message = format!("Pushing {rel} first, then copy URL...");
        }
    }

    /// Copy a URL to the system clipboard, returning true on success.
    fn copy_to_clipboard(&mut self, url: &str) -> bool {
        match arboard::Clipboard::new() {
            Ok(mut clip) => match clip.set_text(url) {
                Ok(()) => {
                    self.status_message = format!("Copied: {url}");
                    true
                }
                Err(_) => {
                    self.status_message = "Failed to copy to clipboard.".into();
                    false
                }
            },
            Err(e) => {
                self.status_message = format!("Clipboard error: {e}");
                false
            }
        }
    }

    /// Copy the currently-selected file's full contents to the system
    /// clipboard. Convenience for pasting session notes / character sheets
    /// into Claude (or anywhere else) without opening the file first.
    fn do_copy_file_contents(&mut self) {
        let Some(rel) = self.selected_file() else {
            self.status_message = "No file selected.".into();
            return;
        };
        let abs = self.abs_path(&rel);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Read error: {e}");
                return;
            }
        };
        let bytes = content.len();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(content)) {
            Ok(()) => {
                self.status_message = format!("Copied {bytes} bytes from {rel}");
            }
            Err(e) => {
                self.status_message = format!("Clipboard error: {e}");
            }
        }
    }

    /// Read the clipboard. If it has rich HTML (from a browser, doc editor,
    /// etc.), run it through htmd to produce markdown; otherwise fall back
    /// to plain text. Stash the result and prompt for a filename, reusing
    /// the existing GdocFilename flow (which is purely "save this content
    /// under root with this name").
    fn do_paste_rich(&mut self) {
        let mut clip = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Clipboard error: {e}");
                return;
            }
        };
        let content = match clip.get().html() {
            Ok(html) => match htmd::convert(&html) {
                Ok(md) => md,
                Err(e) => {
                    self.status_message = format!("HTML→Markdown failed: {e}");
                    return;
                }
            },
            Err(_) => match clip.get_text() {
                Ok(text) if !text.is_empty() => text,
                Ok(_) => {
                    self.status_message = "Clipboard is empty.".into();
                    return;
                }
                Err(e) => {
                    self.status_message = format!("Clipboard read failed: {e}");
                    return;
                }
            },
        };
        self.gdoc_content = Some(content);
        self.input_editor = LineEditor::new();
        self.mode = Mode::GdocFilename;
    }

    fn do_diff(&mut self) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No active root.".into();
            return;
        };
        let Some(entry) = self.store.get(&root, &rel).cloned() else {
            self.status_message = "No gist to diff against.".into();
            return;
        };
        let local_content = std::fs::read_to_string(self.abs_path(&rel)).unwrap_or_default();
        let filename = self
            .abs_path(&rel)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let tx = self.async_tx.clone();
        let gist_id = entry.gist_id.clone();
        let local_for_task = local_content.clone();
        let started = chrono::Utc::now();

        self.status_message = "Fetching remote for diff...".into();

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::full_status(&client, &local_for_task, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::StatusCheck {
                root,
                rel_path: rel,
                started,
                result: result.map_err(|e| e.to_string()),
            });
        });

        // Start with local-only diff; remote side will be fetched async
        self.diff_scroll = 0;
        self.mode = Mode::Diff {
            local: local_content,
            remote: format!("(fetching remote content for {gist_id}...)"),
        };
    }

    /// Kick off a bulk remote check (`f`): list all gists once, fetch content
    /// for any whose `updated_at` moved since we last looked, and record the
    /// observed remote hashes. This is what makes remote edits show up as
    /// ⬇️/❗ in the tree without pulling anything.
    fn start_remote_check(&mut self) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No active root.".into();
            return;
        };
        let entries = match self.store.files_for_root(&root) {
            Some(map) if !map.is_empty() => map.clone(),
            _ => {
                self.status_message = "No gist-mapped files to check.".into();
                return;
            }
        };
        let started = chrono::Utc::now();
        let tx = self.async_tx.clone();

        self.status_message = format!("Checking remote for {} mapped file(s)...", entries.len());

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let tx2 = tx.clone();
            let result = crate::remote::check_remote(&client, &entries, move |done, total| {
                let _ = tx2.send(AsyncEvent::RemoteCheckProgress { done, total });
            })
            .await;
            let _ = tx.send(AsyncEvent::RemoteCheckDone {
                root,
                started,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    fn start_hydration(&mut self) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.current_root().cloned() else {
            self.status_message = "No active root.".into();
            return;
        };

        self.mode = Mode::Hydrating {
            progress: None,
            done: false,
        };
        let tx = self.async_tx.clone();
        let files = self.files.clone();
        // Snapshot only this root's mappings; we'll merge results back when done.
        let mut store = Store::default();
        if let Some(map) = self.store.files_for_root(&root) {
            for (rel, entry) in map {
                store.insert(&root, rel.clone(), entry.clone());
            }
        }

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let tx2 = tx.clone();
            let result =
                crate::hydrate::hydrate(&client, &mut store, &root, &files, move |progress| {
                    let _ = tx2.send(AsyncEvent::HydrationUpdate(progress));
                })
                .await;
            let payload = result.map(|outcome| crate::event::HydrationDoneData {
                matched: outcome.matched,
                ambiguous: outcome.ambiguous,
                store: Box::new(store),
            });
            let _ = tx.send(AsyncEvent::HydrationDone(
                payload.map_err(|e| e.to_string()),
            ));
        });
    }

    /// Apply a user pick from the ambiguous resolver: write the chosen gist mapping
    /// into the store for the current root.
    fn apply_ambiguous_pick(&mut self, item: usize, candidate: usize) {
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        let Some(am) = self.pending_ambiguous.get(item) else {
            return;
        };
        let Some(cand) = am.candidates.get(candidate) else {
            return;
        };
        let entry = FileEntry {
            gist_id: cand.gist_id.clone(),
            url: cand.url.clone(),
            local_sha256: am.local_hash.clone(),
            remote_sha256: am.local_hash.clone(),
            last_synced: chrono::Utc::now(),
            remote_updated_at: None,
        };
        let rel = am.local_path.clone();
        self.store.insert(&root, rel.clone(), entry.clone());
        if let Err(e) = self.store.save() {
            self.status_message = format!("Pick saved in memory but disk save failed: {e}");
        } else {
            self.status_message = format!("Mapped {rel} → {}", cand.url);
        }
        // The candidates here failed the content match, so the remote almost
        // certainly differs from local — fetch its real hash in the
        // background so the tree shows the divergence instead of a
        // fabricated "Synced".
        if let Some(token) = self.token.clone() {
            let filename = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let local_content = std::fs::read_to_string(self.abs_path(&rel)).unwrap_or_default();
            let started = chrono::Utc::now();
            let tx = self.async_tx.clone();
            let rel_clone = rel.clone();
            self.spawn_tracked(async move {
                let client = gist_rs::GistClient::new(token);
                let result = sync::full_status(&client, &local_content, &entry, &filename).await;
                let _ = tx.send(AsyncEvent::StatusCheck {
                    root,
                    rel_path: rel_clone,
                    started,
                    result: result.map_err(|e| e.to_string()),
                });
            });
        }
    }

    fn start_gdoc_fetch(&mut self, url: &str) {
        let Some(doc_id) = crate::gdoc::extract_doc_id(url) else {
            self.mode = Mode::Message("Invalid Google Doc URL.".into());
            return;
        };
        let tx = self.async_tx.clone();
        self.status_message = "Fetching Google Doc...".into();
        self.mode = Mode::Normal; // temporary, will switch to GdocFilename on result

        self.spawn_tracked(async move {
            let result = crate::gdoc::fetch_doc_markdown(&doc_id).await;
            let _ = tx.send(AsyncEvent::GdocFetched(result.map_err(|e| e.to_string())));
        });
    }

    fn save_gdoc_import(&mut self) {
        let Some(content) = self.gdoc_content.take() else {
            self.mode = Mode::Message("No content to save.".into());
            return;
        };
        let name = self.input_editor.content.trim().to_string();
        if name.is_empty() {
            self.mode = Mode::Message("Filename cannot be empty.".into());
            return;
        }

        let Some(root) = self.current_root().cloned() else {
            self.mode = Mode::Message("No root directory configured.".into());
            return;
        };

        // Save to the currently selected directory (or root)
        let dir = if let Some(selected) = self.selected_file() {
            // Go up to the parent directory
            let path = std::path::Path::new(&selected);
            path.parent().map(|p| root.join(p)).unwrap_or(root.clone())
        } else {
            // Use the selected tree node as directory
            let selected = self.tree_state.selected();
            if let Some(id) = selected.last() {
                root.join(id)
            } else {
                root.clone()
            }
        };

        let filename = format!("{name}.md");
        let path = dir.join(&filename);

        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.mode = Mode::Message(format!("Failed to create directory: {e}"));
            return;
        }

        if let Err(e) = std::fs::write(&path, &content) {
            self.mode = Mode::Message(format!("Failed to write file: {e}"));
            return;
        }

        self.mode = Mode::Normal;
        self.status_message = format!("Saved: {}", path.display());
        if let Err(e) = self.refresh_files() {
            self.status_message = format!("Saved but refresh failed: {e}");
        }
    }
}

// ── Sort menu (free-standing impl block to keep diff readable) ─────────────

impl App {
    fn start_sort_menu(&mut self) {
        use crate::config::SortMode;
        let current = self.config.sort.mode;
        let selected = SortMode::all()
            .iter()
            .position(|m| *m == current)
            .unwrap_or(0);
        self.mode = Mode::SortMenu { selected };
    }

    fn handle_sort_menu_key(&mut self, key: KeyEvent) {
        use crate::config::SortMode;
        let Mode::SortMenu { selected } = self.mode else {
            return;
        };
        let all = SortMode::all();
        let n = all.len();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::SortMenu {
                    selected: (selected + 1).min(n - 1),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::SortMenu {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Enter => {
                let chosen = all[selected];
                self.config.sort.mode = chosen;
                if let Err(e) = self.config.save() {
                    self.status_message = format!("Saved sort but config save failed: {e}");
                } else {
                    self.status_message = format!("Sort: {}", chosen.label());
                }
                self.rebuild_tree();
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    // ── Git integration ────────────────────────────────────────────────────

    /// Common helper: return the active repo root if we have one, else show
    /// a status message and return None. Use at the top of each `g*` handler.
    fn git_root_or_warn(&mut self) -> Option<PathBuf> {
        match self.git_repo_root.clone() {
            Some(p) => Some(p),
            None => {
                self.status_message = "Not in a git repo (root has no .git ancestor).".into();
                None
            }
        }
    }

    fn do_git_status(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        // We suspend the TUI and run `git status` so the user sees the full
        // colored output in their scrollback.
        self.pending_alias = Some(format!("git -C {} status", shell_quote(&repo)));
    }

    fn do_git_log(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        let Some(rel) = self.selected_file() else {
            // No file selected — show repo-wide log instead.
            self.pending_alias = Some(format!("git -C {} log", shell_quote(&repo)));
            return;
        };
        // Translate root-relative path to repo-relative (they're the same when
        // root == repo).
        let abs = self.abs_path(&rel);
        let repo_rel = abs
            .strip_prefix(&repo)
            .map(|p| p.to_path_buf())
            .unwrap_or(abs);
        self.pending_alias = Some(format!(
            "git -C {} log -p -- {}",
            shell_quote(&repo),
            shell_quote(&repo_rel),
        ));
    }

    fn confirm_git_pull(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        let cmd = format!("git -C {} pull --rebase", shell_quote(&repo));
        self.mode = Mode::Confirm {
            message: format!("Run `git pull --rebase` in {}?", repo.display()),
            action: ConfirmAction::RunShell { cmd },
        };
    }

    fn confirm_git_push(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        let cmd = format!("git -C {} push", shell_quote(&repo));
        self.mode = Mode::Confirm {
            message: format!("Run `git push` in {}?", repo.display()),
            action: ConfirmAction::RunShell { cmd },
        };
    }

    // ── Bulk operations menu ──────────────────────────────────────────────

    fn start_bulk_menu(&mut self) {
        if self.current_root().is_none() {
            self.status_message = "No root.".into();
            return;
        }
        self.mode = Mode::BulkMenu { selected: 0 };
    }

    /// The four bulk-menu options, in display order, each precomputed with
    /// the set of rel_paths it would touch (so the user sees an accurate count
    /// in the menu and in the confirm dialog).
    pub fn bulk_options(&self) -> Vec<BulkAction> {
        let mut push_dirty: Vec<String> = Vec::new();
        let mut pull_newer: Vec<String> = Vec::new();
        let mut format_json: Vec<String> = Vec::new();
        if self.current_root().is_some() {
            for f in &self.files {
                match self.cached_status(&f.rel_path) {
                    sync::SyncStatus::LocalNewer
                    | sync::SyncStatus::Conflict
                    | sync::SyncStatus::NotGisted => push_dirty.push(f.rel_path.clone()),
                    sync::SyncStatus::RemoteNewer => pull_newer.push(f.rel_path.clone()),
                    sync::SyncStatus::Synced => {}
                }
                // The JSON canonicality check still needs a fresh read since
                // we don't cache file content. This branch only fires on
                // .json files (13 in the current corpus), so it's cheap.
                if f.rel_path.ends_with(".json")
                    && let Ok(content) = std::fs::read_to_string(&f.abs_path)
                    && let Ok(value) = serde_json::from_str::<serde_json::Value>(&content)
                {
                    let compact = serde_json::to_string(&value).unwrap_or_default();
                    if content != compact && content != format!("{compact}\n") {
                        let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
                        if content != pretty && content != format!("{pretty}\n") {
                            format_json.push(f.rel_path.clone());
                        }
                    }
                }
            }
        }
        // Orphan detection: store entries with no matching scanned file.
        let mut orphans: Vec<String> = Vec::new();
        if let Some(root) = self.current_root()
            && let Some(files_for_root) = self.store.files_for_root(root)
        {
            let live: std::collections::HashSet<&str> =
                self.files.iter().map(|f| f.rel_path.as_str()).collect();
            for rel in files_for_root.keys() {
                if !live.contains(rel.as_str()) {
                    orphans.push(rel.clone());
                }
            }
        }
        vec![
            BulkAction::PushAllDirty { rels: push_dirty },
            BulkAction::PullAllRemoteNewer { rels: pull_newer },
            BulkAction::FormatAllJson { rels: format_json },
            BulkAction::PruneOrphans { rels: orphans },
        ]
    }

    fn handle_bulk_menu_key(&mut self, key: KeyEvent) {
        let Mode::BulkMenu { selected } = self.mode else {
            return;
        };
        let opts = self.bulk_options();
        let n = opts.len();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::BulkMenu {
                    selected: (selected + 1).min(n.saturating_sub(1)),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::BulkMenu {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Enter => {
                let Some(action) = opts.into_iter().nth(selected) else {
                    self.mode = Mode::Normal;
                    return;
                };
                if action.count() == 0 {
                    self.status_message = format!("{}: nothing to do.", action.label());
                    self.mode = Mode::Normal;
                    return;
                }
                let label = action.label();
                let count = action.count();
                let detail = match &action {
                    BulkAction::PushAllDirty { .. } => "Local content overwrites remote.",
                    BulkAction::PullAllRemoteNewer { .. } => "Remote content overwrites local.",
                    BulkAction::FormatAllJson { .. } => "Rewrites each file to pretty form.",
                    BulkAction::PruneOrphans { .. } => "Drops store entries for missing files.",
                };
                self.mode = Mode::Confirm {
                    message: format!("{label}: {count} file(s). {detail}"),
                    action: ConfirmAction::Bulk(action),
                };
            }
            _ => {}
        }
    }

    fn run_bulk(&mut self, action: BulkAction) {
        match action {
            BulkAction::PushAllDirty { rels } => {
                let n = rels.len();
                self.status_message = format!("Pushing {n} files...");
                for rel in rels {
                    self.do_sync_up_for(rel, false);
                }
            }
            BulkAction::PullAllRemoteNewer { rels } => {
                let n = rels.len();
                self.status_message = format!("Pulling {n} files...");
                for rel in rels {
                    self.do_sync_down_for(rel);
                }
            }
            BulkAction::FormatAllJson { rels } => {
                let mut ok = 0usize;
                let mut errs = 0usize;
                for rel in &rels {
                    let abs = self.abs_path(rel);
                    let Ok(content) = std::fs::read_to_string(&abs) else {
                        errs += 1;
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
                        errs += 1;
                        continue;
                    };
                    let pretty = serde_json::to_string_pretty(&value)
                        .map(|s| format!("{s}\n"))
                        .unwrap_or_default();
                    if pretty != content {
                        match std::fs::write(&abs, &pretty) {
                            Ok(()) => ok += 1,
                            Err(_) => errs += 1,
                        }
                    }
                }
                if let Err(e) = self.refresh_files() {
                    self.status_message = format!("Formatted {ok}; refresh failed: {e}");
                    return;
                }
                self.update_preview();
                self.update_status();
                self.status_message = if errs == 0 {
                    format!("Formatted {ok} JSON file(s).")
                } else {
                    format!("Formatted {ok}; {errs} error(s).")
                };
            }
            BulkAction::PruneOrphans { rels } => {
                let Some(root) = self.current_root().cloned() else {
                    self.status_message = "No root.".into();
                    return;
                };
                let n = rels.len();
                for rel in &rels {
                    self.store.remove(&root, rel);
                }
                if let Err(e) = self.store.save() {
                    self.status_message = format!("Pruned {n}; store save failed: {e}");
                    return;
                }
                self.update_status();
                self.status_message = format!("Pruned {n} orphan(s).");
            }
        }
    }
}

/// Bucket a cached sync status for sort ordering. Lower = appears first.
/// Files missing from the cache (e.g. newly-added between refreshes) sort
/// as NotGisted.
fn status_rank_cached(status: Option<sync::SyncStatus>) -> u8 {
    match status.unwrap_or(sync::SyncStatus::NotGisted) {
        sync::SyncStatus::Conflict => 0,
        sync::SyncStatus::LocalNewer => 1,
        sync::SyncStatus::RemoteNewer => 2,
        sync::SyncStatus::NotGisted => 3,
        sync::SyncStatus::Synced => 4,
    }
}

/// Check whether `(x, y)` falls inside the given rect.
fn rect_contains(rect: &Rect, x: u16, y: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && x >= rect.x
        && x < rect.x + rect.width
        && y >= rect.y
        && y < rect.y + rect.height
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
