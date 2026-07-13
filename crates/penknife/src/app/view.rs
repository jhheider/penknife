//! Tree, preview, and status-bar state: the refresh pipeline (scan →
//! status cache → git status → tree rebuild), sorting, selection
//! navigation, and mouse routing.

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::{App, Mode, PaneFocus};
use crate::scanner;
use crate::store::FileEntry;
use crate::sync;
use crate::ui::tree;

/// Shown in the preview pane for a macOS online-only (dataless) file, in place
/// of forcing a blocking cloud download to render bytes the user only glanced
/// at while browsing.
const DATALESS_PREVIEW: &str = "· Online-only file (stored in the cloud, not downloaded).\n\n\
     Open it (e / o) to download and edit it.";

/// Sync status for one scanned file, without ever forcing a cloud placeholder
/// to materialize. An online-only (dataless) tracked file is reported as
/// `Synced`: it can't have been edited locally since it was evicted to the
/// cloud (editing requires the bytes, which materializes it), so its resident
/// state matches its last sync. A later remote poll still flags it
/// `RemoteNewer` if the gist moved ahead - same as for a resident synced file.
fn status_for_file(abs_path: &std::path::Path, entry: Option<&FileEntry>) -> sync::SyncStatus {
    if entry.is_none() {
        return sync::SyncStatus::NotGisted;
    }
    if crate::fsutil::is_dataless(abs_path) {
        return sync::SyncStatus::Synced;
    }
    let content = std::fs::read_to_string(abs_path).unwrap_or_default();
    sync::local_status(&content, entry)
}

#[derive(Debug, Default, Clone, Copy)]
struct StatusCounts {
    synced: usize,
    local_newer: usize,
    remote_newer: usize,
    conflict: usize,
    not_gisted: usize,
}

impl App {
    pub(crate) fn rebuild_tree(&mut self) {
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
                // Status rank uses the cached per-file sync state - no
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

    pub fn refresh_files(&mut self) -> color_eyre::Result<()> {
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
    pub(crate) fn refresh_status_cache(&mut self) {
        self.status_cache.clear();
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        self.status_cache.reserve(self.files.len());
        for f in &self.files {
            let entry = self.store.get(&root, &f.rel_path);
            let status = status_for_file(&f.abs_path, entry);
            self.status_cache.insert(f.rel_path.clone(), status);
        }
    }

    /// Recompute one file's cached sync status (one disk read) after a store
    /// entry changed - push/pull/check results - without rescanning the tree.
    pub(crate) fn refresh_status_for(&mut self, rel_path: &str) {
        let Some(root) = self.current_root().cloned() else {
            return;
        };
        let entry = self.store.get(&root, rel_path);
        let status = status_for_file(&self.abs_path(rel_path), entry);
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
    pub(crate) fn refresh_git_status(&mut self) {
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
    pub(crate) fn switch_root(&mut self, index: usize) {
        if index < self.config.roots.len() {
            self.active_root = index;
            if let Err(e) = self.refresh_files() {
                self.status_message = format!("Refresh failed: {e}");
            }
            self.update_status();
        }
    }

    pub fn update_preview(&mut self) {
        if let Some(ref rel) = self.selected_file() {
            let path = self.abs_path(rel);
            // Never force an online-only placeholder to download just because
            // the cursor landed on it - arrow-key browsing would otherwise
            // trigger a cascade of cloud fetches (and hang if offline). Show a
            // hint instead; opening in `$EDITOR` materializes it on demand.
            self.preview_content = if crate::fsutil::is_dataless(&path) {
                DATALESS_PREVIEW.to_string()
            } else {
                std::fs::read_to_string(&path).unwrap_or_default()
            };
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
    pub(crate) fn scroll_right_pane(&mut self, lines: u16, down: bool) {
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

    pub(crate) fn nav_down(&mut self) {
        match self.focused_pane {
            PaneFocus::Tree => {
                self.tree_state.key_down();
                self.update_preview();
                self.update_status();
            }
            PaneFocus::Right => self.scroll_right_pane(1, true),
        }
    }

    pub(crate) fn nav_up(&mut self) {
        match self.focused_pane {
            PaneFocus::Tree => {
                self.tree_state.key_up();
                self.update_preview();
                self.update_status();
            }
            PaneFocus::Right => self.scroll_right_pane(1, false),
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
            // Cached status - populated in refresh_files. One disk read per
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

    /// Select the given rel_path in the tree, expanding *every* ancestor
    /// directory so the leaf is visible. Also focuses the tree pane.
    ///
    /// Note: tui-tree-widget's `open()` and `select()` both take the *full
    /// path from root* to the target node, not just the target's own id.
    /// Passing a single-element vec only worked for top-level entries.
    pub(crate) fn jump_to(&mut self, rel_path: &str) {
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

    /// Move the tree selection to the next file whose sync status is anything
    /// other than `Synced`. `forward` controls direction. Wraps once.
    pub(crate) fn jump_to_next_dirty(&mut self, forward: bool) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{new_for_test, select, write_file};
    use crate::config::SortMode;
    use crate::event::async_channel;
    use crate::scanner::ScannedFile;
    use crate::store::{FileEntry, GIST_BACKEND};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn app3() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.md", "alpha");
        write_file(dir.path(), "b.md", "beta");
        write_file(dir.path(), "sub/c.md", "gamma");
        let (tx, rx) = async_channel();
        std::mem::forget(rx);
        let app = new_for_test(dir.path(), tx);
        (dir, app)
    }

    fn sf(rel: &str, secs: u64) -> ScannedFile {
        ScannedFile {
            rel_path: rel.to_string(),
            abs_path: PathBuf::from(format!("/x/{rel}")),
            modified: UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    fn synced_entry(content: &str) -> FileEntry {
        let h = sync::sha256_hex(content);
        FileEntry {
            backend: GIST_BACKEND.into(),
            remote_id: "g".into(),
            url: "u".into(),
            local_sha256: h.clone(),
            remote_sha256: h,
            last_synced: chrono::Utc::now(),
            remote_updated_at: None,
        }
    }

    // ── apply_sort ──────────────────────────────────────────────────────

    #[test]
    fn apply_sort_mtime_desc_and_asc() {
        let (_d, mut app) = app3();
        app.files = vec![sf("old.md", 1), sf("new.md", 9), sf("mid.md", 5)];
        app.config.sort.mode = SortMode::MtimeDesc;
        app.apply_sort();
        let order: Vec<_> = app.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(order, vec!["new.md", "mid.md", "old.md"]);

        app.config.sort.mode = SortMode::MtimeAsc;
        app.apply_sort();
        let order: Vec<_> = app.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(order, vec!["old.md", "mid.md", "new.md"]);
    }

    #[test]
    fn apply_sort_alpha_asc_and_desc() {
        let (_d, mut app) = app3();
        app.files = vec![sf("b.md", 1), sf("a.md", 2), sf("c.md", 3)];
        app.config.sort.mode = SortMode::AlphaAsc;
        app.apply_sort();
        let order: Vec<_> = app.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(order, vec!["a.md", "b.md", "c.md"]);

        app.config.sort.mode = SortMode::AlphaDesc;
        app.apply_sort();
        let order: Vec<_> = app.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(order, vec!["c.md", "b.md", "a.md"]);
    }

    #[test]
    fn apply_sort_status_orders_by_rank() {
        let (_d, mut app) = app3();
        app.files = vec![
            sf("synced.md", 1),
            sf("conflict.md", 2),
            sf("remote.md", 3),
            sf("local.md", 4),
        ];
        app.status_cache.clear();
        app.status_cache
            .insert("synced.md".into(), sync::SyncStatus::Synced);
        app.status_cache
            .insert("conflict.md".into(), sync::SyncStatus::Conflict);
        app.status_cache
            .insert("remote.md".into(), sync::SyncStatus::RemoteNewer);
        app.status_cache
            .insert("local.md".into(), sync::SyncStatus::LocalNewer);
        app.config.sort.mode = SortMode::Status;
        app.apply_sort();
        let order: Vec<_> = app.files.iter().map(|f| f.rel_path.as_str()).collect();
        // Conflict(0) < LocalNewer(1) < RemoteNewer(2) < Synced(4).
        assert_eq!(
            order,
            vec!["conflict.md", "local.md", "remote.md", "synced.md"]
        );
    }

    // ── refresh_status_cache ────────────────────────────────────────────

    #[test]
    fn refresh_status_cache_defaults_to_not_gisted() {
        let (_d, app) = app3();
        // No store entries: everything is NotGisted.
        assert_eq!(app.cached_status("a.md"), sync::SyncStatus::NotGisted);
        assert_eq!(app.cached_status("unknown.md"), sync::SyncStatus::NotGisted);
    }

    #[test]
    fn refresh_status_cache_reflects_store() {
        let (_d, mut app) = app3();
        let root = app.active_root_path().unwrap();
        app.store
            .insert(&root, "a.md".into(), synced_entry("alpha"));
        // A gist whose recorded local hash differs from disk => LocalNewer.
        app.store
            .insert(&root, "b.md".into(), synced_entry("stale-content"));
        app.refresh_status_cache();
        assert_eq!(app.cached_status("a.md"), sync::SyncStatus::Synced);
        assert_eq!(app.cached_status("b.md"), sync::SyncStatus::LocalNewer);
        assert_eq!(app.cached_status("sub/c.md"), sync::SyncStatus::NotGisted);
    }

    #[test]
    fn refresh_status_for_updates_single_entry() {
        let (_d, mut app) = app3();
        let root = app.active_root_path().unwrap();
        app.store
            .insert(&root, "a.md".into(), synced_entry("alpha"));
        app.refresh_status_for("a.md");
        assert_eq!(app.cached_status("a.md"), sync::SyncStatus::Synced);
    }

    // ── status_counts / update_status ───────────────────────────────────

    #[test]
    fn status_counts_tallies_all_buckets() {
        let (_d, mut app) = app3();
        app.status_cache.clear();
        app.status_cache
            .insert("a.md".into(), sync::SyncStatus::Synced);
        app.status_cache
            .insert("b.md".into(), sync::SyncStatus::Conflict);
        // sub/c.md left out of the cache => counts as NotGisted.
        let c = app.status_counts();
        assert_eq!(c.synced, 1);
        assert_eq!(c.conflict, 1);
        assert_eq!(c.not_gisted, 1);
    }

    #[test]
    fn update_status_dashboard_when_nothing_selected() {
        let (_d, mut app) = app3();
        // Nothing selected: status bar shows the root dashboard.
        app.update_status();
        assert!(!app.status_spans.is_empty());
        // Three NotGisted files, no conflict => DarkGray dashboard color.
        assert_eq!(app.status_color, Color::DarkGray);
    }

    #[test]
    fn update_status_conflict_colors_red() {
        let (_d, mut app) = app3();
        app.status_cache
            .insert("a.md".into(), sync::SyncStatus::Conflict);
        app.update_status();
        assert_eq!(app.status_color, Color::Red);
    }

    #[test]
    fn update_status_selected_file_shows_path() {
        let (_d, mut app) = app3();
        select(&mut app, "a.md");
        app.update_status();
        assert!(app.status_message.contains("a.md"));
        assert!(app.status_message.contains("no gist"));
    }

    // ── jump_to ─────────────────────────────────────────────────────────

    #[test]
    fn jump_to_expands_ancestors_and_selects_leaf() {
        let (_d, mut app) = app3();
        app.focused_pane = PaneFocus::Right;
        app.jump_to("sub/c.md");
        assert_eq!(app.selected_file().as_deref(), Some("sub/c.md"));
        // jump_to always refocuses the tree.
        assert_eq!(app.focused_pane, PaneFocus::Tree);
    }

    // ── jump_to_next_dirty ──────────────────────────────────────────────

    #[test]
    fn jump_to_next_dirty_finds_dirty_file() {
        let (_d, mut app) = app3();
        // All three files are NotGisted (dirty). From no selection, forward
        // lands on a file.
        app.jump_to_next_dirty(true);
        assert!(app.selected_file().is_some());
    }

    #[test]
    fn jump_to_next_dirty_no_dirty_reports() {
        let (_d, mut app) = app3();
        // Mark every file Synced so nothing is dirty.
        for f in ["a.md", "b.md", "sub/c.md"] {
            app.status_cache.insert(f.into(), sync::SyncStatus::Synced);
        }
        app.jump_to_next_dirty(true);
        assert_eq!(app.status_message, "No dirty files.");
    }

    #[test]
    fn jump_to_next_dirty_wraps_backward() {
        let (_d, mut app) = app3();
        // Only one dirty file; make the rest Synced.
        for f in ["a.md", "b.md", "sub/c.md"] {
            app.status_cache.insert(f.into(), sync::SyncStatus::Synced);
        }
        // Pick the single dirty target deterministically.
        app.config.sort.mode = SortMode::AlphaAsc;
        app.rebuild_tree();
        app.status_cache
            .insert("b.md".into(), sync::SyncStatus::LocalNewer);
        app.jump_to_next_dirty(false);
        assert_eq!(app.selected_file().as_deref(), Some("b.md"));
    }

    // ── handle_mouse ────────────────────────────────────────────────────

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_scroll_over_right_pane_scrolls_preview() {
        let (_d, mut app) = app3();
        app.tree_pane_rect = Rect::new(0, 0, 10, 20);
        app.right_pane_rect = Rect::new(10, 0, 30, 20);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 20, 5));
        assert_eq!(app.preview_scroll, 3);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 20, 5));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn mouse_left_click_right_pane_focuses_right() {
        let (_d, mut app) = app3();
        app.tree_pane_rect = Rect::new(0, 0, 10, 20);
        app.right_pane_rect = Rect::new(10, 0, 30, 20);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 20, 5));
        assert_eq!(app.focused_pane, PaneFocus::Right);
    }

    #[test]
    fn mouse_scroll_over_tree_scrolls_tree_not_preview() {
        let (_d, mut app) = app3();
        app.tree_pane_rect = Rect::new(0, 0, 10, 20);
        app.right_pane_rect = Rect::new(10, 0, 30, 20);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
        // Preview untouched; tree pane handled the scroll.
        assert_eq!(app.preview_scroll, 0);
    }

    // ── pure helpers ────────────────────────────────────────────────────

    #[test]
    fn status_rank_cached_orders_states() {
        assert_eq!(status_rank_cached(Some(sync::SyncStatus::Conflict)), 0);
        assert_eq!(status_rank_cached(Some(sync::SyncStatus::LocalNewer)), 1);
        assert_eq!(status_rank_cached(Some(sync::SyncStatus::RemoteNewer)), 2);
        assert_eq!(status_rank_cached(Some(sync::SyncStatus::NotGisted)), 3);
        assert_eq!(status_rank_cached(Some(sync::SyncStatus::Synced)), 4);
        // None sorts as NotGisted.
        assert_eq!(status_rank_cached(None), 3);
    }

    #[test]
    fn rect_contains_boundaries() {
        let r = Rect::new(2, 3, 4, 5); // x:2..6, y:3..8
        assert!(rect_contains(&r, 2, 3));
        assert!(rect_contains(&r, 5, 7));
        assert!(!rect_contains(&r, 6, 3)); // x == x+width is outside
        assert!(!rect_contains(&r, 2, 8));
        assert!(!rect_contains(&r, 1, 3));
        // A zero-area rect contains nothing.
        assert!(!rect_contains(&Rect::new(0, 0, 0, 0), 0, 0));
    }

    #[test]
    fn scroll_right_pane_saturates_at_zero() {
        let (_d, mut app) = app3();
        app.preview_scroll = 2;
        app.scroll_right_pane(5, false);
        assert_eq!(app.preview_scroll, 0);
        app.scroll_right_pane(4, true);
        assert_eq!(app.preview_scroll, 4);
    }
}
