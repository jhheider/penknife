use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
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
    Search,
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
}

#[derive(Debug)]
pub enum ConfirmAction {
    SyncDown,
}

pub struct App {
    pub config: Config,
    pub store: Store,
    pub files: Vec<ScannedFile>,
    pub tree_items: Vec<tui_tree_widget::TreeItem<'static, String>>,
    pub tree_identifiers: Vec<String>,
    pub tree_state: TreeState<String>,
    pub mode: Mode,
    pub preview_content: String,
    pub status_message: String,
    pub status_color: Color,
    pub search_editor: LineEditor,
    pub search_filter: String,
    pub input_editor: LineEditor,
    pub gdoc_content: Option<String>,
    pub should_quit: bool,
    pub async_tx: AsyncSender,
    pub token: Option<String>,
    pub active_root: usize,
    pub pending_ambiguous: Vec<AmbiguousMatch>,
}

impl App {
    pub fn new(async_tx: AsyncSender) -> Result<Self> {
        let config = Config::load()?;
        let store = Store::load()?;

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
            tree_state: TreeState::default(),
            mode: start_mode,
            preview_content: String::new(),
            status_message: String::new(),
            status_color: Color::White,
            search_editor: LineEditor::new(),
            search_filter: String::new(),
            input_editor: LineEditor::new(),
            gdoc_content: None,
            should_quit: false,
            async_tx,
            token,
            active_root: 0,
            pending_ambiguous: Vec::new(),
        };
        app.rebuild_tree();
        app.update_status();
        Ok(app)
    }

    /// Get the current root directory, if any.
    fn current_root(&self) -> Option<&PathBuf> {
        self.config.roots.get(self.active_root)
    }

    pub fn rebuild_tree(&mut self) {
        let root = self.config.roots.get(self.active_root);
        let (items, ids) = tree::build_tree(
            &self.files,
            &self.store,
            root.map(|r| r.as_path()),
            &self.search_filter,
        );
        self.tree_items = items;
        self.tree_identifiers = ids;
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
        if selected.is_empty() {
            return None;
        }
        let id = selected.last()?.clone();
        // It's a file if it ends with .md
        if id.ends_with(".md") { Some(id) } else { None }
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
    }

    pub fn update_status(&mut self) {
        let current_root = self.current_root().cloned();
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
        } else {
            self.status_color = Color::White;
            let total = self.files.len();
            let tracked = current_root
                .as_ref()
                .and_then(|r| self.store.files_for_root(r))
                .map(|m| m.len())
                .unwrap_or(0);
            let root_label = current_root
                .map(|r| r.display().to_string())
                .unwrap_or_else(|| "(no root)".into());
            self.status_message = format!("{total} files | {tracked} tracked | 📂 {root_label}");
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match &self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                return;
            }
            Mode::Message(_) => {
                self.mode = Mode::Normal;
                return;
            }
            Mode::Search => {
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.search_filter.clear();
                        self.rebuild_tree();
                    }
                    KeyCode::Enter => {
                        self.search_filter = self.search_editor.content.clone();
                        self.mode = Mode::Normal;
                        self.rebuild_tree();
                    }
                    _ => {
                        self.search_editor.handle_key(key);
                        // Live filter as you type
                        self.search_filter = self.search_editor.content.clone();
                        self.rebuild_tree();
                    }
                }
                return;
            }
            Mode::GdocUrl => {
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Enter => {
                        let url = self.input_editor.content.clone();
                        self.start_gdoc_fetch(&url);
                    }
                    _ => {
                        self.input_editor.handle_key(key);
                    }
                }
                return;
            }
            Mode::GdocFilename => {
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.gdoc_content = None;
                    }
                    KeyCode::Enter => {
                        self.save_gdoc_import();
                    }
                    _ => {
                        self.input_editor.handle_key(key);
                    }
                }
                return;
            }
            Mode::Confirm { action, .. } => {
                let action_copy = match action {
                    ConfirmAction::SyncDown => ConfirmAction::SyncDown,
                };
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        match action_copy {
                            ConfirmAction::SyncDown => self.do_sync_down(),
                        }
                        self.mode = Mode::Normal;
                    }
                    _ => {
                        self.mode = Mode::Normal;
                    }
                }
                return;
            }
            Mode::Hydrating { done, .. } => {
                if *done {
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
                return;
            }
            Mode::Diff { .. } => {
                // Any key exits diff
                self.mode = Mode::Normal;
                return;
            }
            Mode::ResolveAmbiguous { item, selected } => {
                let item = *item;
                let selected = *selected;
                let total_items = self.pending_ambiguous.len();
                let candidates = self
                    .pending_ambiguous
                    .get(item)
                    .map(|m| m.candidates.len())
                    .unwrap_or(0);
                match key.code {
                    KeyCode::Esc => {
                        self.pending_ambiguous.clear();
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Char('j') | KeyCode::Down if candidates > 0 => {
                        let new_sel = (selected + 1).min(candidates - 1);
                        self.mode = Mode::ResolveAmbiguous {
                            item,
                            selected: new_sel,
                        };
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let new_sel = selected.saturating_sub(1);
                        self.mode = Mode::ResolveAmbiguous {
                            item,
                            selected: new_sel,
                        };
                    }
                    KeyCode::Char('s') => {
                        // Skip this one
                        let next = item + 1;
                        if next < total_items {
                            self.mode = Mode::ResolveAmbiguous {
                                item: next,
                                selected: 0,
                            };
                        } else {
                            self.pending_ambiguous.clear();
                            self.mode = Mode::Normal;
                            self.status_message = "Ambiguous resolution complete.".into();
                        }
                    }
                    KeyCode::Enter if candidates > 0 => {
                        self.apply_ambiguous_pick(item, selected);
                        let next = item + 1;
                        if next < total_items {
                            self.mode = Mode::ResolveAmbiguous {
                                item: next,
                                selected: 0,
                            };
                        } else {
                            self.pending_ambiguous.clear();
                            self.mode = Mode::Normal;
                            self.rebuild_tree();
                            self.update_status();
                            self.status_message = "Ambiguous resolution complete.".into();
                        }
                    }
                    _ => {}
                }
                return;
            }
            Mode::RootSwitcher { selected } => {
                let sel = *selected;
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                    }
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
                return;
            }
            Mode::SetupRoot | Mode::AddRoot => {
                match key.code {
                    KeyCode::Esc => {
                        if matches!(self.mode, Mode::AddRoot) {
                            // Go back to root switcher
                            self.mode = Mode::RootSwitcher {
                                selected: self.active_root,
                            };
                        }
                        // SetupRoot: no escape if no roots (must enter one)
                        // But allow quit
                    }
                    KeyCode::Enter => {
                        let raw = self.input_editor.content.trim().to_string();
                        if raw.is_empty() {
                            return;
                        }
                        let expanded = if let Some(rest) = raw.strip_prefix('~') {
                            if let Some(home) = dirs::home_dir() {
                                home.join(rest.strip_prefix('/').unwrap_or(rest))
                            } else {
                                PathBuf::from(&raw)
                            }
                        } else {
                            PathBuf::from(&raw)
                        };
                        if !expanded.is_dir() {
                            self.mode =
                                Mode::Message(format!("Not a directory: {}", expanded.display()));
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
                return;
            }
            Mode::Normal => {}
        }

        // Normal mode keybindings
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('/') => {
                self.search_editor = LineEditor::new();
                self.mode = Mode::Search;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.tree_state.key_down();
                self.update_preview();
                self.update_status();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.tree_state.key_up();
                self.update_preview();
                self.update_status();
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                // If it's a directory, toggle open; if file, already previewed
                self.tree_state.toggle_selected();
                self.update_preview();
                self.update_status();
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => {
                self.tree_state.key_left();
                self.update_preview();
                self.update_status();
            }
            KeyCode::Char('u') => self.do_sync_up(),
            KeyCode::Char('d') => {
                if let Some(ref rel) = self.selected_file() {
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
            }
            KeyCode::Char('c') => self.do_copy_url(),
            KeyCode::Char('D') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.do_diff();
            }
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
            KeyCode::Char('I') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input_editor = LineEditor::new();
                self.mode = Mode::GdocUrl;
            }
            KeyCode::Tab => {
                self.mode = Mode::RootSwitcher {
                    selected: self.active_root,
                };
            }
            _ => {}
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
                }
                Err(e) => {
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

        tokio::spawn(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::push(
                &client,
                store_snapshot.as_ref(),
                &rel_clone,
                &filename,
                &content,
            )
            .await;
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

        tokio::spawn(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::pull(&client, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::PullDone {
                root: root_clone,
                rel_path: rel_clone,
                result: result.map_err(|e| e.to_string()),
            });
        });
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
            match arboard::Clipboard::new() {
                Ok(mut clip) => {
                    let url = entry.url.clone();
                    if clip.set_text(&url).is_ok() {
                        self.status_message = format!("Copied: {url}");
                    } else {
                        self.status_message = "Failed to copy to clipboard.".into();
                    }
                }
                Err(e) => {
                    self.status_message = format!("Clipboard error: {e}");
                }
            }
        } else {
            // Auto-push first, then copy
            self.do_sync_up();
            self.status_message = format!("Pushing {rel} first, then copy URL...");
        }
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

        tokio::spawn(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::full_status(&client, &local_for_task, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::StatusCheck {
                rel_path: rel,
                result: result.map_err(|e| e.to_string()),
            });
        });

        // Start with local-only diff; remote side will be fetched async
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

        tokio::spawn(async move {
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

        tokio::spawn(async move {
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
