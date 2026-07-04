//! Key dispatch: one handler per mode, plus the Normal-mode binding table.
//! Handlers translate keys into mode transitions and calls into the
//! [`super::gist`] / [`super::files`] / [`super::view`] operations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, BulkAction, ConfirmAction, DeleteChoice, GIT_MENU_LABELS, Mode, PaneFocus};
use crate::ui::input::LineEditor;

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) {
        match &self.mode {
            Mode::Help | Mode::Message(_) => {
                self.mode = Mode::Normal;
            }
            Mode::FilePicker { .. } => self.handle_picker_key(key),
            Mode::GdocUrl => self.handle_gdoc_url_key(key),
            Mode::GdocFilename => self.handle_gdoc_filename_key(key),
            Mode::Confirm { .. } => self.handle_confirm_key(key),
            Mode::Diff { .. } => self.handle_diff_key(key),
            Mode::ResolveAmbiguous { .. } => self.handle_resolve_ambiguous_key(key),
            Mode::RootSwitcher { .. } => self.handle_root_switcher_key(key),
            Mode::SetupRoot | Mode::AddRoot => self.handle_setup_or_add_root_key(key),
            Mode::SearchQuery => self.handle_search_query_key(key),
            Mode::SearchResults { .. } => self.handle_search_results_key(key),
            Mode::ReplaceQuery => self.handle_replace_query_key(key),
            Mode::ReplaceTarget => self.handle_replace_target_key(key),
            Mode::ReplaceReview { .. } => self.handle_replace_review_key(key),
            Mode::Rename { .. } => self.handle_rename_key(key),
            Mode::LinkGist { .. } => self.handle_link_gist_key(key),
            Mode::SortMenu { .. } => self.handle_sort_menu_key(key),
            Mode::BulkMenu { .. } => self.handle_bulk_menu_key(key),
            Mode::DeleteMenu { .. } => self.handle_delete_menu_key(key),
            Mode::GitMenu { .. } => self.handle_git_menu_key(key),
            Mode::PublishMenu { .. } => self.handle_publish_menu_key(key),
            Mode::GdocAuth { .. } => self.handle_gdoc_auth_key(key),
            Mode::Normal => self.handle_normal_key(key),
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
                self.open_delete_menu();
            }
            KeyCode::Char('n') => self.jump_to_next_dirty(true),
            KeyCode::Char('N') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jump_to_next_dirty(false);
            }
            KeyCode::Char('D') if !key.modifiers.contains(KeyModifiers::CONTROL) => self.do_diff(),
            KeyCode::Char('m') => self.start_rename(),
            KeyCode::Char('L') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_manual_link();
            }
            KeyCode::Char('O') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_sort_menu();
            }
            KeyCode::Char('B') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_bulk_menu();
            }
            KeyCode::Char('g') => self.open_git_menu(),
            KeyCode::Char('p') => self.open_publish_menu(),
            KeyCode::Char('s') => self.start_replace(),
            KeyCode::Char('f') => self.start_search(),
            KeyCode::Char('M') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.pending_ambiguous.is_empty() {
                    self.status_message = "No ambiguous matches to resolve.".into();
                } else {
                    self.mode = Mode::ResolveAmbiguous {
                        item: 0,
                        selected: 0,
                    };
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

    fn handle_gdoc_url_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let url = self.input_editor.content.clone();
                self.start_import_from_url(&url);
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
                self.pending_import_entry = None;
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
        // alone - only an explicit confirm/cancel dismisses.
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
                    remote_id,
                } => self.do_delete_remote(rel_path, root, remote_id),
                ConfirmAction::TrashLocal { rel_path, root } => {
                    self.do_trash_local(rel_path, root);
                }
                ConfirmAction::GdocUnpublish {
                    rel_path,
                    root,
                    remote_id,
                } => {
                    self.do_gdoc_unpublish(rel_path, root, remote_id);
                }
                ConfirmAction::DeleteBoth {
                    rel_path,
                    root,
                    remote_id,
                } => {
                    // Remote delete is async (DeleteDone prunes the store
                    // entry); the trash is immediate. If the remote delete
                    // later fails, its error lands in the status bar and the
                    // gist survives; the local file is already in the trash
                    // and recoverable either way.
                    self.do_delete_remote(rel_path.clone(), root.clone(), remote_id);
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
                // SetupRoot: no escape - must configure a root or Ctrl+Q to quit.
            }
            KeyCode::Enter => {
                let raw = self.input_editor.content.trim().to_string();
                if raw.is_empty() {
                    return;
                }
                let expanded = super::expand_tilde(&raw);
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

    fn handle_link_gist_key(&mut self, key: KeyEvent) {
        let Mode::LinkGist { rel_path } = &self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let rel_path = rel_path.clone();
                let raw = self.input_editor.content.trim().to_string();
                self.mode = Mode::Normal;
                self.link_gist_to(rel_path, &raw);
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn handle_delete_menu_key(&mut self, key: KeyEvent) {
        let Mode::DeleteMenu { selected } = self.mode else {
            return;
        };
        let opts = self.delete_options();
        let max = opts.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::DeleteMenu {
                    selected: (selected + 1).min(max),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::DeleteMenu {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Enter => {
                let Some(choice) = opts.get(selected).copied() else {
                    self.mode = Mode::Normal;
                    return;
                };
                match choice {
                    DeleteChoice::Remote => self.confirm_delete_remote(),
                    DeleteChoice::Local => self.confirm_trash_local(),
                    DeleteChoice::Both => self.confirm_delete_both(),
                }
            }
            _ => {}
        }
    }

    fn handle_publish_menu_key(&mut self, key: KeyEvent) {
        let Mode::PublishMenu { selected } = self.mode else {
            return;
        };
        let opts = self.publish_options();
        let max = opts.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::PublishMenu {
                    selected: (selected + 1).min(max),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::PublishMenu {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Enter => {
                let Some(choice) = opts.get(selected).copied() else {
                    self.mode = Mode::Normal;
                    return;
                };
                self.run_publish_choice(choice);
            }
            _ => {}
        }
    }

    fn handle_gdoc_auth_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            // Abort the poll task; the device code dies on Google's side.
            if let Some(h) = self.gdoc_auth_abort.take() {
                h.abort();
            }
            self.pending_publish = None;
            self.mode = Mode::Normal;
            self.status_message = "Google sign-in cancelled.".into();
        }
    }

    fn handle_git_menu_key(&mut self, key: KeyEvent) {
        let Mode::GitMenu { selected } = self.mode else {
            return;
        };
        let max = GIT_MENU_LABELS.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::GitMenu {
                    selected: (selected + 1).min(max),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::GitMenu {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                match selected {
                    0 => self.do_git_status(),
                    1 => self.do_git_log(),
                    2 => self.confirm_git_pull(),
                    3 => self.confirm_git_push(),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn handle_search_query_key(&mut self, key: KeyEvent) {
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
                self.search_query = q;
                self.input_editor = LineEditor::new();
                self.run_search_scan();
            }
            _ => {
                self.input_editor.handle_key(key);
            }
        }
    }

    fn handle_search_results_key(&mut self, key: KeyEvent) {
        let Mode::SearchResults { selected } = self.mode else {
            return;
        };
        let max = self.search_matches.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                if let Some(m) = self.search_matches.get(selected) {
                    let rel_path = m.rel_path.clone();
                    self.mode = Mode::Normal;
                    self.jump_to(&rel_path);
                } else {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode = Mode::SearchResults {
                    selected: (selected + 1).min(max),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode = Mode::SearchResults {
                    selected: selected.saturating_sub(1),
                };
            }
            KeyCode::PageDown => {
                self.mode = Mode::SearchResults {
                    selected: (selected + 10).min(max),
                };
            }
            KeyCode::PageUp => {
                self.mode = Mode::SearchResults {
                    selected: selected.saturating_sub(10),
                };
            }
            _ => {}
        }
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

    fn start_bulk_menu(&mut self) {
        if self.active_root_path().is_none() {
            self.status_message = "No root.".into();
            return;
        }
        self.mode = Mode::BulkMenu { selected: 0 };
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
}
