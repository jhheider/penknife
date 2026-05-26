use std::collections::HashSet;
use std::path::PathBuf;

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
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    SyncDown,
    DeleteRemote {
        rel_path: String,
        root: PathBuf,
        gist_id: String,
    },
    TrashLocal {
        rel_path: String,
        root: PathBuf,
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
    'H', 'r', 'R', 'I', 's', 'm', '=',
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
            scanner::scan_directory(&config.roots[0]).unwrap_or_default()
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
        };
        app.rebuild_tree();
        app.update_status();
        if let Some(w) = alias_warning {
            app.status_message = w;
        }
        Ok(app)
    }

    /// Get the current root directory, if any.
    fn current_root(&self) -> Option<&PathBuf> {
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
        let root = self.config.roots.get(self.active_root);
        let built = tree::build_tree(&self.files, &self.store, root.map(|r| r.as_path()));
        self.tree_items = built.items;
        self.tree_identifiers = built.identifiers;
        self.tree_file_ids = built.file_ids;
    }

    pub fn refresh_files(&mut self) -> Result<()> {
        if let Some(root) = self.current_root() {
            self.files = scanner::scan_directory(root)?;
        } else {
            self.files.clear();
        }
        self.rebuild_tree();
        self.update_preview();
        Ok(())
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
            let status = if entry.is_some() {
                let content = std::fs::read_to_string(self.abs_path(rel)).unwrap_or_default();
                sync::local_status(&content, entry.as_ref())
            } else {
                sync::SyncStatus::NotGisted
            };
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
    /// dashboard and the jump-to-next-dirty navigation.
    fn status_counts(&self) -> StatusCounts {
        let mut c = StatusCounts::default();
        let Some(root) = self.current_root() else {
            c.not_gisted = self.files.len();
            return c;
        };
        for file in &self.files {
            let s = match self.store.get(root, &file.rel_path) {
                Some(entry) => {
                    let content = std::fs::read_to_string(&file.abs_path).unwrap_or_default();
                    sync::local_status(&content, Some(entry))
                }
                None => sync::SyncStatus::NotGisted,
            };
            match s {
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
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            match action_copy {
                ConfirmAction::SyncDown => self.do_sync_down(),
                ConfirmAction::DeleteRemote {
                    rel_path,
                    root,
                    gist_id,
                } => self.do_delete_remote(rel_path, root, gist_id),
                ConfirmAction::TrashLocal { rel_path, root } => {
                    self.do_trash_local(rel_path, root);
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
            KeyCode::Char('=') => self.do_format_in_place(),
            KeyCode::Char('s') => self.start_replace(),
            KeyCode::Char('H') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_hydration();
            }
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
                    self.status_message = format!("Pushed {rel_path} → {url}");
                    self.rebuild_tree();
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
                result,
            } => match result {
                Ok((content, entry)) => {
                    let path = root.join(&rel_path);
                    if let Err(e) = std::fs::write(&path, &content) {
                        self.status_message = format!("Write failed: {e}");
                        return;
                    }
                    self.store.insert(&root, rel_path.clone(), entry);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Pull ok but store save failed: {e}");
                        return;
                    }
                    self.status_message = format!("Pulled {rel_path}");
                    self.update_preview();
                    self.rebuild_tree();
                }
                Err(e) => {
                    self.status_message = format!("Pull failed: {e}");
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
            AsyncEvent::StatusCheck { rel_path, result } => match result {
                Ok((status, remote_content)) => {
                    self.status_message = format!("{} {rel_path}", status.icon());
                    if let Mode::Diff { remote, .. } = &mut self.mode {
                        *remote = remote_content;
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
        let path = self.abs_path(&rel);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Read error: {e}");
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
            let result = sync::push(&client, store_snapshot.as_ref(), &filename, &content).await;
            let _ = tx.send(AsyncEvent::PushDone {
                root: root_clone,
                rel_path: rel_clone,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    fn do_sync_down(&mut self) {
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
            self.status_message = "No gist mapped for this file.".into();
            return;
        };
        let filename = self
            .abs_path(&rel)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
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

    /// Reformat the selected file to its canonical pretty form, writing back
    /// to disk. Currently only `.json` is supported (parse + `to_string_pretty`).
    /// If the file isn't supported or fails to parse, the file is left alone
    /// and the status bar explains why.
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
        let pretty = match serde_json::to_string_pretty(&value) {
            Ok(s) => s,
            Err(e) => {
                self.status_message = format!("Serialize failed: {e}");
                return;
            }
        };
        // Preserve a trailing newline if the original had one (or wasn't empty).
        let with_newline = if pretty.ends_with('\n') {
            pretty
        } else {
            format!("{pretty}\n")
        };
        if with_newline == content {
            self.status_message = format!("{rel}: already canonical.");
            return;
        }
        let before = content.len();
        let after = with_newline.len();
        if let Err(e) = std::fs::write(&abs, &with_newline) {
            self.status_message = format!("Write failed: {e}");
            return;
        }
        if let Err(e) = self.refresh_files() {
            self.status_message = format!("Formatted but refresh failed: {e}");
            return;
        }
        self.update_preview();
        self.update_status();
        let delta = after as isize - before as isize;
        let sign = if delta >= 0 { "+" } else { "" };
        self.status_message = format!("Formatted {rel} ({sign}{delta} bytes)");
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
        let Some(root) = self.current_root().cloned() else {
            return;
        };
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
            let status = match self.store.get(&root, &file.rel_path) {
                Some(entry) => {
                    let content = std::fs::read_to_string(&file.abs_path).unwrap_or_default();
                    sync::local_status(&content, Some(entry))
                }
                None => sync::SyncStatus::NotGisted,
            };
            if !matches!(status, sync::SyncStatus::Synced) {
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

        self.status_message = "Fetching remote for diff...".into();

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::full_status(&client, &local_for_task, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::StatusCheck {
                rel_path: rel,
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
        };
        let rel = am.local_path.clone();
        self.store.insert(&root, rel.clone(), entry);
        if let Err(e) = self.store.save() {
            self.status_message = format!("Pick saved in memory but disk save failed: {e}");
        } else {
            self.status_message = format!("Mapped {rel} → {}", cand.url);
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

/// Check whether `(x, y)` falls inside the given rect.
fn rect_contains(rect: &Rect, x: u16, y: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && x >= rect.x
        && x < rect.x + rect.width
        && y >= rect.y
        && y < rect.y + rect.height
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
