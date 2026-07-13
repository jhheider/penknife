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
            KeyCode::Char('p') => self.do_copy_rich(),
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
                    self.start_refresh(super::Refresh::User {
                        select: None,
                        done_message: None,
                    });
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
                self.files.clear();
                self.status_cache.clear();
                self.rebuild_tree();
                self.start_refresh(super::Refresh::User {
                    select: None,
                    done_message: None,
                });
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
                    self.status_message = format!("Sort saved, but saving config failed: {e}");
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
            self.status_message = "No active root.".into();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{ch, ctrl, guard, key, new_for_test, select, write_file};
    use crate::event::async_channel;
    use crate::hydrate::{AmbiguousMatch, GistCandidate};
    use crate::store::{FileEntry, GIST_BACKEND};
    use tempfile::TempDir;

    /// A temp root with three markdown files and a ready App.
    fn app_with_files() -> (TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.md", "alpha");
        write_file(dir.path(), "b.md", "beta");
        write_file(dir.path(), "sub/c.md", "gamma");
        let (tx, _rx) = async_channel();
        // Keep the receiver from dropping so sends in handlers don't matter.
        std::mem::forget(_rx);
        let app = new_for_test(dir.path(), tx);
        (dir, app)
    }

    fn entry(id: &str, local: &str, remote: &str) -> FileEntry {
        FileEntry {
            backend: GIST_BACKEND.into(),
            remote_id: id.into(),
            url: format!("https://gist.github.com/u/{id}"),
            local_sha256: local.into(),
            remote_sha256: remote.into(),
            last_synced: chrono::Utc::now(),
            remote_updated_at: None,
        }
    }

    // ── handle_normal_key ───────────────────────────────────────────────

    #[test]
    fn normal_q_quits() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn normal_question_opens_help() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('?'));
        assert!(matches!(app.mode, Mode::Help));
    }

    #[test]
    fn help_dismisses_on_any_key() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Help;
        app.handle_key(ch('x'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn message_dismisses_on_any_key() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Message("hi".into());
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn normal_slash_opens_picker_and_ranks() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        assert!(matches!(app.mode, Mode::FilePicker { selected: 0 }));
        // Empty query ranks every file.
        assert_eq!(app.picker_matches.len(), 3);
    }

    #[test]
    fn normal_tab_toggles_pane_focus() {
        let (_d, mut app) = app_with_files();
        assert_eq!(app.focused_pane, PaneFocus::Tree);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focused_pane, PaneFocus::Right);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focused_pane, PaneFocus::Tree);
    }

    #[test]
    fn normal_r_opens_root_switcher_at_active() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('R'));
        assert!(matches!(app.mode, Mode::RootSwitcher { selected } if selected == app.active_root));
    }

    #[test]
    fn normal_i_opens_gdoc_url() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('I'));
        assert!(matches!(app.mode, Mode::GdocUrl));
    }

    #[test]
    fn normal_o_opens_sort_menu_at_current() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('O'));
        // Default sort is MtimeDesc, which is index 0.
        assert!(matches!(app.mode, Mode::SortMenu { selected: 0 }));
    }

    #[test]
    fn normal_m_with_no_ambiguous_stays_normal() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('M'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status_message.contains("No ambiguous"));
    }

    #[test]
    fn normal_m_with_ambiguous_opens_resolver() {
        let (_d, mut app) = app_with_files();
        app.pending_ambiguous.push(AmbiguousMatch {
            local_path: "a.md".into(),
            local_hash: "h".into(),
            candidates: vec![GistCandidate {
                remote_id: "g1".into(),
                url: "u".into(),
                description: None,
                size: 1,
            }],
        });
        app.handle_key(ch('M'));
        assert!(matches!(
            app.mode,
            Mode::ResolveAmbiguous {
                item: 0,
                selected: 0
            }
        ));
    }

    #[test]
    fn normal_ctrl_guarded_keys_do_not_fire_action() {
        let (_d, mut app) = app_with_files();
        // Ctrl-R must not open the root switcher.
        app.handle_key(ctrl(KeyCode::Char('R')));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn normal_alias_key_queues_pending_alias() {
        let (_d, mut app) = app_with_files();
        app.config.aliases.insert("z".into(), "echo hi".into());
        app.handle_key(ch('z'));
        assert_eq!(app.pending_alias.as_deref(), Some("echo hi"));
    }

    #[test]
    fn normal_u_without_token_reports_no_token() {
        let (_d, mut app) = app_with_files();
        select(&mut app, "a.md");
        app.handle_key(ch('u'));
        assert_eq!(app.status_message, crate::app::NO_TOKEN_HINT);
    }

    // ── handle_picker_key ───────────────────────────────────────────────

    #[test]
    fn picker_down_up_clamp() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        app.handle_key(key(KeyCode::Down));
        assert!(matches!(app.mode, Mode::FilePicker { selected: 1 }));
        app.handle_key(key(KeyCode::Up));
        assert!(matches!(app.mode, Mode::FilePicker { selected: 0 }));
        // Up at the top clamps.
        app.handle_key(key(KeyCode::Up));
        assert!(matches!(app.mode, Mode::FilePicker { selected: 0 }));
    }

    #[test]
    fn picker_ctrl_n_p_move_selection() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        app.handle_key(ctrl(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::FilePicker { selected: 1 }));
        app.handle_key(ctrl(KeyCode::Char('p')));
        assert!(matches!(app.mode, Mode::FilePicker { selected: 0 }));
    }

    #[test]
    fn picker_down_clamps_at_last() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Down));
        }
        assert!(matches!(app.mode, Mode::FilePicker { selected: 2 }));
    }

    #[test]
    fn picker_esc_returns_normal() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn picker_enter_jumps_and_selects() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        // A file is now selected in the tree.
        assert!(app.selected_file().is_some());
    }

    #[test]
    fn picker_typing_refilters() {
        let (_d, mut app) = app_with_files();
        app.handle_key(ch('/'));
        app.handle_key(ch('c'));
        // Only sub/c.md matches "c" ... plus fuzzy could match others; assert
        // the query registered and at least one match remains.
        assert_eq!(app.picker_editor.content, "c");
        assert!(!app.picker_matches.is_empty());
    }

    // ── handle_confirm_key ──────────────────────────────────────────────

    #[test]
    fn confirm_y_runs_action() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Confirm {
            message: "?".into(),
            action: ConfirmAction::RunShell {
                cmd: "echo ok".into(),
            },
        };
        app.handle_key(ch('y'));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.pending_alias.as_deref(), Some("echo ok"));
    }

    #[test]
    fn confirm_enter_confirms() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Confirm {
            message: "?".into(),
            action: ConfirmAction::RunShell {
                cmd: "echo ok".into(),
            },
        };
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.pending_alias.as_deref(), Some("echo ok"));
    }

    #[test]
    fn confirm_n_cancels() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Confirm {
            message: "?".into(),
            action: ConfirmAction::RunShell {
                cmd: "echo ok".into(),
            },
        };
        app.handle_key(ch('n'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.pending_alias.is_none());
    }

    #[test]
    fn confirm_stray_key_leaves_dialog_open() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Confirm {
            message: "?".into(),
            action: ConfirmAction::RunShell {
                cmd: "echo ok".into(),
            },
        };
        app.handle_key(ch(' '));
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        app.handle_key(key(KeyCode::Left));
        assert!(matches!(app.mode, Mode::Confirm { .. }));
    }

    // ── handle_resolve_ambiguous_key ────────────────────────────────────

    fn with_ambiguous(app: &mut App) {
        app.pending_ambiguous.push(AmbiguousMatch {
            local_path: "a.md".into(),
            local_hash: "h".into(),
            candidates: vec![
                GistCandidate {
                    remote_id: "g1".into(),
                    url: "u1".into(),
                    description: None,
                    size: 1,
                },
                GistCandidate {
                    remote_id: "g2".into(),
                    url: "u2".into(),
                    description: None,
                    size: 2,
                },
            ],
        });
        app.mode = Mode::ResolveAmbiguous {
            item: 0,
            selected: 0,
        };
    }

    #[test]
    fn resolve_j_k_clamp_candidates() {
        let (_d, mut app) = app_with_files();
        with_ambiguous(&mut app);
        app.handle_key(ch('j'));
        assert!(matches!(
            app.mode,
            Mode::ResolveAmbiguous { selected: 1, .. }
        ));
        // Clamp at the last candidate.
        app.handle_key(ch('j'));
        assert!(matches!(
            app.mode,
            Mode::ResolveAmbiguous { selected: 1, .. }
        ));
        app.handle_key(ch('k'));
        assert!(matches!(
            app.mode,
            Mode::ResolveAmbiguous { selected: 0, .. }
        ));
    }

    #[test]
    fn resolve_s_skips_to_completion() {
        let (_d, mut app) = app_with_files();
        with_ambiguous(&mut app);
        // Only one item, so skip completes the resolver.
        app.handle_key(ch('s'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.pending_ambiguous.is_empty());
    }

    #[test]
    fn resolve_esc_aborts() {
        let (_d, mut app) = app_with_files();
        with_ambiguous(&mut app);
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.pending_ambiguous.is_empty());
    }

    #[test]
    fn resolve_enter_picks_and_advances() {
        let _g = guard();
        let (_d, mut app) = app_with_files();
        with_ambiguous(&mut app);
        app.handle_key(key(KeyCode::Enter));
        // Single item: after pick, resolver completes.
        assert!(matches!(app.mode, Mode::Normal));
        // Mapping recorded for a.md.
        let root = app.active_root_path().unwrap();
        assert!(app.store.get(&root, "a.md").is_some());
    }

    // ── handle_root_switcher_key ────────────────────────────────────────

    #[test]
    fn root_switcher_j_k_clamp() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::RootSwitcher { selected: 0 };
        // Only one root: down clamps at 0.
        app.handle_key(ch('j'));
        assert!(matches!(app.mode, Mode::RootSwitcher { selected: 0 }));
        app.handle_key(ch('k'));
        assert!(matches!(app.mode, Mode::RootSwitcher { selected: 0 }));
    }

    #[test]
    fn root_switcher_esc_returns_normal() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::RootSwitcher { selected: 0 };
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn root_switcher_a_opens_add_root() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::RootSwitcher { selected: 0 };
        app.handle_key(ch('a'));
        assert!(matches!(app.mode, Mode::AddRoot));
    }

    // ── handle_sort_menu_key ────────────────────────────────────────────

    #[test]
    fn sort_menu_j_k_clamp() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::SortMenu { selected: 0 };
        app.handle_key(ch('k'));
        assert!(matches!(app.mode, Mode::SortMenu { selected: 0 }));
        for _ in 0..10 {
            app.handle_key(ch('j'));
        }
        // Five sort modes: last index is 4.
        assert!(matches!(app.mode, Mode::SortMenu { selected: 4 }));
    }

    #[test]
    fn sort_menu_enter_applies_and_saves() {
        let _g = guard();
        let (_d, mut app) = app_with_files();
        app.mode = Mode::SortMenu { selected: 2 }; // AlphaAsc
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.config.sort.mode, crate::config::SortMode::AlphaAsc);
    }

    #[test]
    fn sort_menu_esc_cancels() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::SortMenu { selected: 1 };
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        // Unchanged from default.
        assert_eq!(app.config.sort.mode, crate::config::SortMode::MtimeDesc);
    }

    // ── handle_bulk_menu_key ────────────────────────────────────────────

    #[test]
    fn bulk_menu_j_k_clamp() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::BulkMenu { selected: 0 };
        for _ in 0..10 {
            app.handle_key(ch('j'));
        }
        // Four bulk options: last index 3.
        assert!(matches!(app.mode, Mode::BulkMenu { selected: 3 }));
        app.handle_key(ch('k'));
        assert!(matches!(app.mode, Mode::BulkMenu { selected: 2 }));
    }

    #[test]
    fn bulk_menu_enter_on_empty_action_reports_nothing() {
        let (_d, mut app) = app_with_files();
        // No store entries: PullAllRemoteNewer (index 1) has nothing to do.
        app.mode = Mode::BulkMenu { selected: 1 };
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status_message.contains("nothing to do"));
    }

    #[test]
    fn bulk_menu_enter_nonempty_opens_confirm() {
        let (_d, mut app) = app_with_files();
        // All three files are NotGisted, so PushAllDirty (index 0) is non-empty.
        app.mode = Mode::BulkMenu { selected: 0 };
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            app.mode,
            Mode::Confirm {
                action: ConfirmAction::Bulk(_),
                ..
            }
        ));
    }

    // ── handle_delete_menu_key ──────────────────────────────────────────

    #[test]
    fn delete_menu_local_only_when_no_gist() {
        let (_d, mut app) = app_with_files();
        select(&mut app, "a.md");
        app.mode = Mode::DeleteMenu { selected: 0 };
        // Only one option (Local); down clamps at 0.
        app.handle_key(ch('j'));
        assert!(matches!(app.mode, Mode::DeleteMenu { selected: 0 }));
        // Enter opens the trash confirm.
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            app.mode,
            Mode::Confirm {
                action: ConfirmAction::TrashLocal { .. },
                ..
            }
        ));
    }

    #[test]
    fn delete_menu_esc_returns_normal() {
        let (_d, mut app) = app_with_files();
        select(&mut app, "a.md");
        app.mode = Mode::DeleteMenu { selected: 0 };
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn delete_menu_both_when_gisted() {
        let _g = guard();
        let (_d, mut app) = app_with_files();
        let root = app.active_root_path().unwrap();
        app.store_mut()
            .insert(&root, "a.md".into(), entry("g1", "x", "x"));
        select(&mut app, "a.md");
        app.mode = Mode::DeleteMenu { selected: 2 }; // Both
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            app.mode,
            Mode::Confirm {
                action: ConfirmAction::DeleteBoth { .. },
                ..
            }
        ));
    }

    // ── handle_git_menu_key ─────────────────────────────────────────────

    #[test]
    fn git_menu_j_k_clamp_and_pull_confirm() {
        let (_d, mut app) = app_with_files();
        app.git_repo_root = Some(app.active_root_path().unwrap());
        app.mode = Mode::GitMenu { selected: 0 };
        for _ in 0..10 {
            app.handle_key(ch('j'));
        }
        // Four labels: last index 3.
        assert!(matches!(app.mode, Mode::GitMenu { selected: 3 }));
        app.handle_key(ch('k')); // index 2 = pull --rebase
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            app.mode,
            Mode::Confirm {
                action: ConfirmAction::RunShell { .. },
                ..
            }
        ));
    }

    #[test]
    fn git_menu_status_shells_out() {
        let (_d, mut app) = app_with_files();
        app.git_repo_root = Some(app.active_root_path().unwrap());
        app.mode = Mode::GitMenu { selected: 0 }; // git status
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.pending_alias.is_some());
    }

    // ── handle_search_results_key ───────────────────────────────────────

    fn with_search_results(app: &mut App) {
        app.search_matches = vec![
            crate::replace::ReplaceMatch {
                abs_path: app.abs_path("a.md"),
                rel_path: "a.md".into(),
                line: 1,
                col_byte: 0,
                line_text: "alpha".into(),
            },
            crate::replace::ReplaceMatch {
                abs_path: app.abs_path("b.md"),
                rel_path: "b.md".into(),
                line: 1,
                col_byte: 0,
                line_text: "beta".into(),
            },
        ];
        app.mode = Mode::SearchResults { selected: 0 };
    }

    #[test]
    fn search_results_j_k_clamp() {
        let (_d, mut app) = app_with_files();
        with_search_results(&mut app);
        app.handle_key(ch('j'));
        assert!(matches!(app.mode, Mode::SearchResults { selected: 1 }));
        app.handle_key(ch('j'));
        assert!(matches!(app.mode, Mode::SearchResults { selected: 1 }));
        app.handle_key(ch('k'));
        assert!(matches!(app.mode, Mode::SearchResults { selected: 0 }));
    }

    #[test]
    fn search_results_enter_jumps() {
        let (_d, mut app) = app_with_files();
        with_search_results(&mut app);
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.selected_file().as_deref(), Some("a.md"));
    }

    #[test]
    fn search_results_esc_returns_normal() {
        let (_d, mut app) = app_with_files();
        with_search_results(&mut app);
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    // ── handle_replace_review_key ───────────────────────────────────────

    fn with_replace_review(app: &mut App) {
        app.replace_matches = vec![
            crate::replace::ReplaceMatch {
                abs_path: app.abs_path("a.md"),
                rel_path: "a.md".into(),
                line: 1,
                col_byte: 0,
                line_text: "alpha".into(),
            },
            crate::replace::ReplaceMatch {
                abs_path: app.abs_path("b.md"),
                rel_path: "b.md".into(),
                line: 1,
                col_byte: 0,
                line_text: "beta".into(),
            },
        ];
        app.replace_checked = vec![true, true];
        app.mode = Mode::ReplaceReview { selected: 0 };
    }

    #[test]
    fn replace_review_space_toggles() {
        let (_d, mut app) = app_with_files();
        with_replace_review(&mut app);
        app.handle_key(ch(' '));
        assert!(!app.replace_checked[0]);
        app.handle_key(ch(' '));
        assert!(app.replace_checked[0]);
    }

    #[test]
    fn replace_review_a_checks_all_z_clears_all() {
        let (_d, mut app) = app_with_files();
        with_replace_review(&mut app);
        app.handle_key(ch('z'));
        assert_eq!(app.replace_checked, vec![false, false]);
        app.handle_key(ch('a'));
        assert_eq!(app.replace_checked, vec![true, true]);
    }

    #[test]
    fn replace_review_j_k_clamp() {
        let (_d, mut app) = app_with_files();
        with_replace_review(&mut app);
        app.handle_key(ch('j'));
        assert!(matches!(app.mode, Mode::ReplaceReview { selected: 1 }));
        app.handle_key(ch('j'));
        assert!(matches!(app.mode, Mode::ReplaceReview { selected: 1 }));
        app.handle_key(ch('k'));
        assert!(matches!(app.mode, Mode::ReplaceReview { selected: 0 }));
    }

    #[test]
    fn replace_review_esc_cancels() {
        let (_d, mut app) = app_with_files();
        with_replace_review(&mut app);
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status_message.contains("cancelled"));
    }

    // ── handle_rename_key / handle_link_gist_key ────────────────────────

    #[test]
    fn rename_esc_cancels() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Rename {
            old_rel: "a.md".into(),
        };
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn rename_enter_no_change_reports() {
        let _g = guard();
        let (_d, mut app) = app_with_files();
        select(&mut app, "a.md");
        app.input_editor.content = "a.md".into();
        app.mode = Mode::Rename {
            old_rel: "a.md".into(),
        };
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status_message.contains("No change"));
    }

    #[test]
    fn link_gist_esc_cancels() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::LinkGist {
            rel_path: "a.md".into(),
        };
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    // ── handle_diff_key ─────────────────────────────────────────────────

    #[test]
    fn diff_scroll_and_exit() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Diff {
            local: "l".into(),
            remote: "r".into(),
        };
        app.handle_key(ch('j'));
        assert_eq!(app.diff_scroll, 1);
        app.handle_key(ch('k'));
        assert_eq!(app.diff_scroll, 0);
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn diff_stray_key_exits() {
        let (_d, mut app) = app_with_files();
        app.mode = Mode::Diff {
            local: "l".into(),
            remote: "r".into(),
        };
        app.handle_key(ch('x'));
        assert!(matches!(app.mode, Mode::Normal));
    }
}
