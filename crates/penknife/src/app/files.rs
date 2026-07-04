//! Local file operations: rename/move, trash, find-and-replace, clipboard
//! import/export, Google Doc import, the git menu's shell-out commands, and
//! the bulk-operations menu's actions.

use std::path::PathBuf;

use super::{App, BulkAction, Mode, shell_quote};
use crate::event::AsyncEvent;
use crate::sync;
use crate::ui::input::LineEditor;

impl App {
    pub(crate) fn do_request_edit(&mut self) {
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

    /// Prompt to move the selected file to the OS trash. The remote gist (if
    /// any) is left intact - restore-from-trash + hydration will re-link it.
    pub(crate) fn confirm_trash_local(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
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
            action: super::ConfirmAction::TrashLocal {
                rel_path: rel,
                root,
            },
        };
    }

    /// Actually trash the file and prune its store mapping. Synchronous -
    /// `trash::delete` is a quick OS call.
    pub(crate) fn do_trash_local(&mut self, rel_path: String, root: PathBuf) {
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

    // ── Rename / move ───────────────────────────────────────────────────────

    pub(crate) fn start_rename(&mut self) {
        let Some(rel) = self.selected_file() else {
            self.status_message = "No file selected.".into();
            return;
        };
        self.input_editor = LineEditor::new();
        self.input_editor.content = rel.clone();
        self.input_editor.cursor = self.input_editor.content.len();
        self.mode = Mode::Rename { old_rel: rel };
    }

    /// Carry out the rename: validate, move on disk, update the store key,
    /// and (if mapped) kick off the remote gist filename update.
    pub(crate) fn do_rename(&mut self, old_rel: String, new_rel: String) {
        if new_rel.is_empty() {
            self.status_message = "Empty filename - rename cancelled.".into();
            return;
        }
        if new_rel == old_rel {
            self.status_message = "No change - rename cancelled.".into();
            return;
        }
        // Disallow absolute paths or backtracking - rename stays under the root.
        if new_rel.starts_with('/') || new_rel.split('/').any(|c| c == "..") {
            self.status_message =
                "New name must be a relative path under the root (no .. or leading /).".into();
            return;
        }
        let Some(root) = self.active_root_path() else {
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
        let root = self.active_root_path()?;
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

    /// Display label for the scope - e.g. "Red Hand of Doom/rp-posts" or
    /// "(root)" for the active root itself.
    pub fn replace_scope_label(&self) -> String {
        let Some(root) = self.active_root_path() else {
            return "(no root)".into();
        };
        let Some(scope) = self.replace_scope() else {
            return "(no root)".into();
        };
        let rel = scope.strip_prefix(&root).unwrap_or(&scope);
        let s = rel.to_string_lossy();
        if s.is_empty() {
            "(root)".into()
        } else {
            s.into_owned()
        }
    }

    pub(crate) fn start_replace(&mut self) {
        if self.active_root_path().is_none() {
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

    pub(crate) fn start_search(&mut self) {
        if self.active_root_path().is_none() {
            self.status_message = "No root directory configured.".into();
            return;
        }
        self.search_query.clear();
        self.search_matches.clear();
        self.input_editor = LineEditor::new();
        self.mode = Mode::SearchQuery;
    }

    /// Run the content scan for find-in-files. Same scope rules as replace:
    /// the selected directory if one is highlighted, otherwise the root.
    pub(crate) fn run_search_scan(&mut self) {
        let Some(scope) = self.replace_scope() else {
            self.status_message = "No scope available.".into();
            self.mode = Mode::Normal;
            return;
        };
        let Some(root) = self.active_root_path() else {
            self.status_message = "No root.".into();
            self.mode = Mode::Normal;
            return;
        };
        self.search_matches = crate::replace::scan(&scope, &root, &self.search_query);
        if self.search_matches.is_empty() {
            self.status_message = format!(
                "No matches for '{}' in {}",
                self.search_query,
                self.replace_scope_label()
            );
            self.mode = Mode::Normal;
            return;
        }
        self.mode = Mode::SearchResults { selected: 0 };
    }

    pub(crate) fn run_replace_scan(&mut self) {
        let Some(scope) = self.replace_scope() else {
            self.status_message = "No scope available.".into();
            self.mode = Mode::Normal;
            return;
        };
        let Some(root) = self.active_root_path() else {
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
        self.status_message = format!("Found {n} matches - review and apply.");
        self.mode = Mode::ReplaceReview { selected: 0 };
    }

    pub(crate) fn apply_replace(&mut self) {
        let to_apply: Vec<crate::replace::ReplaceMatch> = self
            .replace_matches
            .iter()
            .zip(self.replace_checked.iter())
            .filter_map(|(m, c)| if *c { Some(m.clone()) } else { None })
            .collect();
        if to_apply.is_empty() {
            self.status_message = "Nothing checked - no changes applied.".into();
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
            msg.push_str(&format!(" - {} skipped (file changed)", result.drifted));
        }
        if !result.errors.is_empty() {
            msg.push_str(&format!(" - {} write error(s)", result.errors.len()));
        }
        if let Err(e) = self.refresh_files() {
            msg.push_str(&format!(" - refresh failed: {e}"));
        }
        self.update_preview();
        self.update_status();
        self.status_message = msg;
        self.replace_matches.clear();
        self.replace_checked.clear();
        self.mode = Mode::Normal;
    }

    // ── Clipboard ───────────────────────────────────────────────────────────

    /// Copy the currently-selected file's full contents to the system
    /// clipboard. Convenience for pasting session notes / character sheets
    /// into Claude (or anywhere else) without opening the file first.
    pub(crate) fn do_copy_file_contents(&mut self) {
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
    pub(crate) fn do_paste_rich(&mut self) {
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

    // ── Google Doc import ───────────────────────────────────────────────────

    pub(crate) fn start_gdoc_fetch(&mut self, url: &str) {
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

    pub(crate) fn save_gdoc_import(&mut self) {
        let Some(content) = self.gdoc_content.take() else {
            self.mode = Mode::Message("No content to save.".into());
            return;
        };
        let name = self.input_editor.content.trim().to_string();
        if name.is_empty() {
            self.mode = Mode::Message("Filename cannot be empty.".into());
            return;
        }

        let Some(root) = self.active_root_path() else {
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

        // A gist import carries its mapping; record it so the file lands
        // already linked (and synced) rather than as a stranger.
        if let Some(entry) = self.pending_import_entry.take()
            && let Ok(rel) = path.strip_prefix(&root)
        {
            let rel_path = rel.to_string_lossy().to_string();
            let gist_url = entry.url.clone();
            self.store.insert(&root, rel_path, entry);
            if let Err(e) = self.store.save() {
                self.status_message = format!("Saved; store save failed: {e}");
            } else {
                self.status_message = format!("Saved and linked to {gist_url}");
            }
        }

        if let Err(e) = self.refresh_files() {
            self.status_message = format!("Saved but refresh failed: {e}");
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

    /// Open the git menu for the active root. Requires the root to be inside
    /// a git repo; otherwise says so instead of showing dead options.
    pub(crate) fn open_git_menu(&mut self) {
        if self.git_repo_root.is_none() {
            self.status_message = "Active root is not inside a git repository.".into();
            return;
        }
        self.mode = Mode::GitMenu { selected: 0 };
    }

    pub(crate) fn do_git_status(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        // We suspend the TUI and run `git status` so the user sees the full
        // colored output in their scrollback.
        self.pending_alias = Some(format!("git -C {} status", shell_quote(&repo)));
    }

    pub(crate) fn do_git_log(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        let Some(rel) = self.selected_file() else {
            // No file selected - show repo-wide log instead.
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

    pub(crate) fn confirm_git_pull(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        let cmd = format!("git -C {} pull --rebase", shell_quote(&repo));
        self.mode = Mode::Confirm {
            message: format!("Run `git pull --rebase` in {}?", repo.display()),
            action: super::ConfirmAction::RunShell { cmd },
        };
    }

    pub(crate) fn confirm_git_push(&mut self) {
        let Some(repo) = self.git_root_or_warn() else {
            return;
        };
        let cmd = format!("git -C {} push", shell_quote(&repo));
        self.mode = Mode::Confirm {
            message: format!("Run `git push` in {}?", repo.display()),
            action: super::ConfirmAction::RunShell { cmd },
        };
    }

    // ── Bulk operations ────────────────────────────────────────────────────

    /// The four bulk-menu options, in display order, each precomputed with
    /// the set of rel_paths it would touch (so the user sees an accurate count
    /// in the menu and in the confirm dialog).
    pub fn bulk_options(&self) -> Vec<BulkAction> {
        let mut push_dirty: Vec<String> = Vec::new();
        let mut pull_newer: Vec<String> = Vec::new();
        let mut format_json: Vec<String> = Vec::new();
        if self.active_root_path().is_some() {
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
        if let Some(root) = self.active_root_path()
            && let Some(files_for_root) = self.store.files_for_root(&root)
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

    pub(crate) fn run_bulk(&mut self, action: BulkAction) {
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
                let Some(root) = self.active_root_path() else {
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
