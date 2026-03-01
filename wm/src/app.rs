use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use tui_tree_widget::TreeState;

use crate::config::Config;
use crate::error::Result;
use crate::event::{AsyncEvent, AsyncSender};
use crate::hydrate::HydrationProgress;
use crate::scanner::{self, ScannedFile};
use crate::store::Store;
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
        let (items, ids) = tree::build_tree(&self.files, &self.store, &self.search_filter);
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
            let _ = self.refresh_files();
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
        if let Some(ref rel) = self.selected_file() {
            let entry = self.store.get(rel);
            let status = if entry.is_some() {
                let content = std::fs::read_to_string(self.abs_path(rel)).unwrap_or_default();
                sync::local_status(&content, entry)
            } else {
                sync::SyncStatus::NotGisted
            };
            self.status_color = status.color();
            let url = entry.map(|e| e.url.as_str()).unwrap_or("no gist");
            self.status_message = format!("{} {} | {url}", status.icon(), rel);
        } else {
            self.status_color = Color::White;
            let total = self.files.len();
            let tracked = self.store.files.len();
            let root_label = self
                .current_root()
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
                    self.mode = Mode::Normal;
                    let _ = self.refresh_files();
                    self.update_status();
                }
                return;
            }
            Mode::Diff { .. } => {
                // Any key exits diff
                self.mode = Mode::Normal;
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
                    KeyCode::Char('d') => {
                        if sel < self.config.roots.len() {
                            let _ = self.config.remove_root(sel);
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
                                let _ = self.refresh_files();
                                self.update_status();
                            }
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
                        let _ = self.config.add_root(expanded);
                        self.active_root = self.config.roots.len() - 1;
                        let _ = self.refresh_files();
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
                    if self.store.get(rel).is_some() {
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
            AsyncEvent::PushDone { rel_path, result } => match result {
                Ok(entry) => {
                    let url = entry.url.clone();
                    self.store.insert(rel_path.clone(), entry);
                    let _ = self.store.save();
                    self.status_message = format!("Pushed {rel_path} → {url}");
                    self.rebuild_tree();
                }
                Err(e) => {
                    self.status_message = format!("Push failed: {e}");
                }
            },
            AsyncEvent::PullDone { rel_path, result } => match result {
                Ok((content, entry)) => {
                    let path = self.abs_path(&rel_path);
                    if let Err(e) = std::fs::write(&path, &content) {
                        self.status_message = format!("Write failed: {e}");
                        return;
                    }
                    self.store.insert(rel_path.clone(), entry);
                    let _ = self.store.save();
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
            AsyncEvent::HydrationDone(result) => {
                match result {
                    Ok(count) => {
                        // Reload store from disk — hydration ran on a cloned store
                        if let Ok(reloaded) = Store::load() {
                            self.store = reloaded;
                        }
                        if let Mode::Hydrating { progress, done } = &mut self.mode {
                            if let Some(p) = progress {
                                p.phase =
                                    format!("Complete! Matched {count} files. Press any key.");
                            }
                            *done = true;
                        }
                    }
                    Err(e) => {
                        self.mode = Mode::Message(format!("Hydration error: {e}"));
                    }
                }
            }
            AsyncEvent::StatusCheck { rel_path, result } => match result {
                Ok((status, _remote_content)) => {
                    self.status_message = format!("{} {rel_path}", status.icon());
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
        let store_snapshot = self.store.get(&rel).cloned();
        let tx = self.async_tx.clone();
        let rel_clone = rel.clone();

        self.status_message = format!("Pushing {rel}...");

        // Build a mini store for the push function
        tokio::spawn(async move {
            let client = gist_rs::GistClient::new(token);
            let mut temp_store = Store {
                version: 1,
                files: std::collections::BTreeMap::new(),
            };
            if let Some(entry) = store_snapshot {
                temp_store.insert(rel_clone.clone(), entry);
            }
            let result = sync::push(&client, &temp_store, &rel_clone, &filename, &content).await;
            let _ = tx.send(AsyncEvent::PushDone {
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
        let Some(entry) = self.store.get(&rel).cloned() else {
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

        self.status_message = format!("Pulling {rel}...");

        tokio::spawn(async move {
            let client = gist_rs::GistClient::new(token);
            let result = sync::pull(&client, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::PullDone {
                rel_path: rel_clone,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    fn do_copy_url(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        if let Some(entry) = self.store.get(&rel) {
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
        let Some(entry) = self.store.get(&rel).cloned() else {
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

        self.mode = Mode::Hydrating {
            progress: None,
            done: false,
        };
        let tx = self.async_tx.clone();
        let files = self.files.clone();
        let mut store = Store {
            version: self.store.version,
            files: self.store.files.clone(),
        };

        tokio::spawn(async move {
            let client = gist_rs::GistClient::new(token);
            let tx2 = tx.clone();
            let result = crate::hydrate::hydrate(&client, &mut store, &files, move |progress| {
                let _ = tx2.send(AsyncEvent::HydrationUpdate(progress));
            })
            .await;
            let _ = tx.send(AsyncEvent::HydrationDone(result.map_err(|e| e.to_string())));
        });
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
        let _ = self.refresh_files();
    }
}
