//! Gist sync operations: push/pull/diff/remote-check/hydration/delete, the
//! URL clipboard helpers, and the application of their async results.
//!
//! The store/disk mutation rules for async results live in
//! [`crate::sync_apply`]; `handle_async_event` only renders outcomes
//! (status bar, mode transitions, tree refreshes).

use std::path::PathBuf;

use super::{App, ConfirmAction, Mode};
use crate::event::AsyncEvent;
use crate::store::{FileEntry, Store};
use crate::sync;
use crate::sync_apply::{self, PullApply};
use crate::ui::input::LineEditor;

impl App {
    pub fn handle_async_event(&mut self, event: AsyncEvent) {
        match event {
            AsyncEvent::PushDone {
                root,
                rel_path,
                result,
            } => {
                self.pending_pushes.remove(&rel_path);
                match result {
                    Ok(entry) => {
                        let url = entry.url.clone();
                        self.store_mut().insert(&root, rel_path.clone(), entry);
                        if let Err(e) = self.store.save() {
                            self.status_message =
                                format!("Pushed, but saving local state failed: {e}");
                            return;
                        }
                        self.refresh_status_for(&rel_path);
                        self.rebuild_tree();
                        // Follow up on a queued copy-url request, if it's for
                        // this file. Skip the intermediate "Pushed" message when
                        // a copy is about to supersede it - otherwise a second,
                        // later PushDone's "Pushed" line would clobber "Copied".
                        if self.pending_copy.as_ref().is_some_and(|p| *p == rel_path) {
                            self.pending_copy = None;
                            self.copy_to_clipboard(&url);
                        } else {
                            self.status_message = format!("Pushed {rel_path} → {url}");
                        }
                    }
                    Err(e) => {
                        self.pending_copy = None;
                        self.status_message = format!("Push failed: {e}{}", scope_hint(&e));
                    }
                }
            }
            AsyncEvent::PullDone {
                root,
                rel_path,
                expected_local_sha256,
                result,
            } => match result {
                Ok((content, entry)) => {
                    match sync_apply::apply_pull(
                        self.store_mut(),
                        &root,
                        &rel_path,
                        &expected_local_sha256,
                        &content,
                        entry,
                    ) {
                        Ok(PullApply::DriftRefused) => {
                            self.status_message = format!(
                                "{rel_path} changed on disk during pull - not overwriting. Pull again to retry."
                            );
                        }
                        Ok(PullApply::Applied) => {
                            if let Err(e) = self.store.save() {
                                self.status_message =
                                    format!("Pulled, but saving local state failed: {e}");
                                return;
                            }
                            self.refresh_status_for(&rel_path);
                            self.update_preview();
                            self.rebuild_tree();
                            self.status_message = format!("Pulled {rel_path}");
                        }
                        Err(e) => {
                            self.status_message = format!("Write failed: {e}");
                        }
                    }
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
                self.pending_pushes.remove(&rel_path);
                // Record the observed divergence so the tree shows the real
                // state (RemoteNewer/Conflict) even if the user declines.
                if sync_apply::record_divergence(
                    self.store_mut(),
                    &root,
                    &rel_path,
                    remote_sha256,
                    remote_updated_at,
                ) {
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Saving local state failed: {e}");
                    }
                    self.refresh_status_for(&rel_path);
                    self.rebuild_tree();
                    self.update_status();
                }
                self.mode = Mode::Confirm {
                    message: format!(
                        "Remote gist for {rel_path} changed since last sync. Force push (overwrites remote - use D to diff first)?"
                    ),
                    action: ConfirmAction::ForcePush {
                        rel_path: rel_path.clone(),
                    },
                };
                self.status_message = format!("Push blocked: remote changed for {rel_path}");
            }
            AsyncEvent::RemoteCheckDone {
                root,
                started,
                result,
            } => {
                self.remote_check_inflight = false;
                self.last_remote_poll = Some(std::time::Instant::now());
                match result {
                    Ok(outcome) => {
                        self.remote_poll_failures = 0;
                        let applied = sync_apply::apply_remote_updates(
                            self.store_mut(),
                            &root,
                            started,
                            outcome.updated,
                        );
                        for rel in &applied {
                            self.refresh_status_for(rel);
                        }
                        if !applied.is_empty()
                            && let Err(e) = self.store.save()
                        {
                            self.status_message =
                                format!("Remote check: saving local state failed: {e}");
                            return;
                        }
                        self.rebuild_tree();
                        self.update_status();
                        // Quiet when nothing moved; the icons and counts are
                        // the steady-state signal.
                        if outcome.divergent > 0 || !outcome.missing.is_empty() {
                            let mut msg = format!(
                                "Remote: {} of {} changed",
                                outcome.divergent, outcome.checked
                            );
                            if !outcome.missing.is_empty() {
                                msg.push_str(&format!(
                                    ", {} deleted ({})",
                                    outcome.missing.len(),
                                    outcome.missing.join(", ")
                                ));
                            }
                            self.status_message = msg;
                        }
                    }
                    Err(e) => {
                        self.remote_poll_failures = self.remote_poll_failures.saturating_add(1);
                        // Report the first failure; repeats just extend the
                        // backoff silently (offline shouldn't nag).
                        if self.remote_poll_failures == 1 {
                            self.status_message = format!("Remote check failed: {e}");
                        }
                    }
                }
            }
            AsyncEvent::HydrationUpdate(progress) => {
                // Hydration runs in the background; narrate inline. Phase
                // strings carry their own counts.
                self.status_message = if progress.matched > 0 {
                    format!(
                        "Hydrating: {} ({} matched)",
                        progress.phase, progress.matched
                    )
                } else {
                    format!("Hydrating: {}", progress.phase)
                };
            }
            AsyncEvent::HydrationDone(result) => match result {
                Ok(data) => {
                    // Merge hydration's discovered mappings into the live store
                    // rather than reloading from disk (which would clobber any
                    // concurrent push/pull that completed during hydration).
                    self.store_mut().merge_from(&data.store);
                    // Advance this root's incremental cursor so the next
                    // hydrate only fetches gists changed after this walk began.
                    self.store_mut()
                        .set_hydrated_cursor(&data.root, data.new_cursor);
                    if let Err(e) = self.store.save() {
                        self.status_message =
                            format!("Hydrated, but saving local state failed: {e}");
                        return;
                    }
                    self.pending_ambiguous = data.ambiguous;
                    if data.matched > 0 {
                        self.refresh_status_cache();
                        self.rebuild_tree();
                        self.update_status();
                    }
                    // Quiet unless hydration actually found something to say.
                    if !self.pending_ambiguous.is_empty() {
                        self.status_message = format!(
                            "Hydrated {} file(s); {} ambiguous match(es): press M to resolve",
                            data.matched,
                            self.pending_ambiguous.len()
                        );
                    } else if data.matched > 0 {
                        self.status_message = format!("Hydrated {} file(s)", data.matched);
                    } else if self.status_message.starts_with("Hydrating:") {
                        self.status_message.clear();
                        self.update_status();
                    }
                }
                Err(e) => {
                    self.status_message = format!("Hydration error: {e}");
                }
            },
            AsyncEvent::StatusCheck {
                root,
                rel_path,
                started,
                result,
            } => match result {
                Ok(full) => {
                    // A diff fetch is also a remote observation - persist it
                    // so the tree reflects any divergence it revealed. Skip
                    // if the entry synced while the fetch was in flight.
                    if sync_apply::record_observation(
                        self.store_mut(),
                        &root,
                        &rel_path,
                        started,
                        full.remote_sha256.clone(),
                        full.remote_updated_at,
                    ) {
                        if let Err(e) = self.store.save() {
                            self.status_message = format!("Saving local state failed: {e}");
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
                    self.store_mut().remove(&root, &rel_path);
                    if let Err(e) = self.store.save() {
                        self.status_message =
                            format!("Deleted, but saving local state failed: {e}");
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
            AsyncEvent::LinkDone {
                root,
                rel_path,
                result,
            } => match result {
                Ok(entry) => {
                    let url = entry.url.clone();
                    let synced = entry.local_sha256 == entry.remote_sha256;
                    self.store_mut().insert(&root, rel_path.clone(), entry);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Linked, but saving local state failed: {e}");
                        return;
                    }
                    self.refresh_status_for(&rel_path);
                    self.rebuild_tree();
                    self.update_status();
                    self.status_message = if synced {
                        format!("Linked {rel_path} → {url} (in sync)")
                    } else {
                        format!("Linked {rel_path} → {url} (differs - D to diff, u/d to reconcile)")
                    };
                }
                Err(e) => {
                    self.status_message = format!("Link failed: {e}");
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
            AsyncEvent::GistImportFetched(result) => match result {
                Ok(data) => {
                    self.gdoc_content = Some(data.content);
                    self.pending_import_entry = Some(data.entry);
                    // Prefill the save-as prompt with the gist's own filename
                    // (the prompt appends .md).
                    self.input_editor = LineEditor::new();
                    self.input_editor.content = data.filename.trim_end_matches(".md").to_string();
                    self.input_editor.cursor = self.input_editor.content.len();
                    self.mode = Mode::GdocFilename;
                }
                Err(e) => {
                    self.mode = Mode::Message(format!("Gist import failed: {e}"));
                }
            },
            AsyncEvent::RefreshDone {
                generation,
                root,
                files,
                status_cache,
                git_repo_root,
                git_statuses,
                select,
                only_if_changed,
                done_message,
            } => {
                self.apply_refresh(
                    generation,
                    root,
                    files,
                    status_cache,
                    git_repo_root,
                    git_statuses,
                    select,
                    only_if_changed,
                    done_message,
                );
            }
            AsyncEvent::ScanProgress { generation, count } => {
                // Ignore progress from a superseded scan.
                if generation == self.refresh_generation && self.scanning {
                    self.scan_count = count;
                }
            }
        }
    }

    /// Route the `I` import prompt. Gist URLs and bare gist IDs go to the
    /// gist importer; everything else is treated as a Google Doc URL (whose
    /// own validation rejects junk).
    pub(crate) fn start_import_from_url(&mut self, raw: &str) {
        let t = raw.trim();
        let bare_id =
            !t.is_empty() && !t.contains('/') && t.chars().all(|c| c.is_ascii_alphanumeric());
        if (t.contains("gist.github.com") || bare_id)
            && let Some(id) = parse_gist_id(t)
        {
            self.start_gist_import(id);
            return;
        }
        self.start_gdoc_fetch(t);
    }

    /// Fetch a gist for import as a new local file. Single-file gists only:
    /// a multi-file gist has no unambiguous "the content". On success the
    /// save-as prompt opens with the mapping queued; the store write lands
    /// in `save_gdoc_import` once the file exists on disk.
    fn start_gist_import(&mut self, id: String) {
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            self.mode = Mode::Normal;
            return;
        };
        let tx = self.async_tx.clone();
        self.status_message = "Fetching gist...".into();
        self.mode = Mode::Normal; // switches to the save-as prompt on result

        self.spawn_tracked(async move {
            let result = async {
                let gist = client.get(&id).await.map_err(|e| e.to_string())?;
                if gist.files.len() != 1 {
                    return Err(format!(
                        "gist has {} files; only single-file gists can be imported (use L to link)",
                        gist.files.len()
                    ));
                }
                let filename = gist.files.keys().next().cloned().unwrap_or_default();
                let content = client
                    .file_content(&gist, &filename)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "gist file has no content".to_string())?;
                let sha = sync::sha256_hex(&content);
                Ok(crate::event::GistImportData {
                    entry: FileEntry {
                        backend: crate::store::GIST_BACKEND.into(),
                        remote_id: gist.id.clone(),
                        url: gist.html_url.clone(),
                        local_sha256: sha.clone(),
                        remote_sha256: sha,
                        last_synced: chrono::Utc::now(),
                        remote_updated_at: Some(gist.updated_at),
                    },
                    content,
                    filename,
                })
            }
            .await;
            let _ = tx.send(AsyncEvent::GistImportFetched(result));
        });
    }

    pub(crate) fn do_sync_up(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        self.do_sync_up_for(rel, false);
    }

    /// Inner push implementation, parameterized by rel_path so bulk-push
    /// can call it once per dirty file. Unless `force` is set, updates are
    /// refused when the remote changed since the last sync; a PushBlocked
    /// event then records the divergence and prompts for a force-push.
    pub(crate) fn do_sync_up_for(&mut self, rel: String, force: bool) {
        // Debounce: don't stack a second push on a file that's already
        // pushing (e.g. a double `u` press). Two concurrent pushes of an
        // unmapped file would each create a separate gist. A force-push is a
        // deliberate override from the confirm dialog, so it always proceeds
        // (and PushBlocked has already cleared the guard by then anyway).
        if !force && self.pending_pushes.contains(&rel) {
            self.status_message = format!("Already pushing {rel}...");
            return;
        }
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            return;
        };
        let Some(root) = self.active_root_path() else {
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
        self.pending_pushes.insert(rel.clone());

        self.spawn_tracked(async move {
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

    pub(crate) fn confirm_sync_down(&mut self) {
        let Some(ref rel) = self.selected_file() else {
            return;
        };
        let has_entry = self
            .active_root_path()
            .map(|r| self.store.get(&r, rel).is_some())
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

    pub(crate) fn do_sync_down(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        self.do_sync_down_for(rel);
    }

    /// Inner pull implementation, parameterized by rel_path so bulk-pull
    /// can call it once per remote-newer file.
    pub(crate) fn do_sync_down_for(&mut self, rel: String) {
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            return;
        };
        let Some(root) = self.active_root_path() else {
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
            let result = sync::pull(&client, &entry, &filename).await;
            let _ = tx.send(AsyncEvent::PullDone {
                root: root_clone,
                rel_path: rel_clone,
                expected_local_sha256,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    pub(crate) fn do_open_in_browser(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(entry) = self
            .active_root_path()
            .and_then(|r| self.store.get(&r, &rel).cloned())
        else {
            self.status_message = "No gist mapped for this file.".into();
            return;
        };
        match open::that(&entry.url) {
            Ok(()) => self.status_message = format!("Opened {}", entry.url),
            Err(e) => self.status_message = format!("Open failed: {e}"),
        }
    }

    /// Open the delete menu for the selected file. Options are contextual:
    /// remote choices only appear when the file has a gist mapping.
    pub(crate) fn open_delete_menu(&mut self) {
        if self.selected_file().is_none() {
            self.status_message = "No file selected.".into();
            return;
        }
        if self.delete_options().is_empty() {
            self.status_message = "Nothing to delete.".into();
            return;
        }
        self.mode = Mode::DeleteMenu { selected: 0 };
    }

    /// The delete choices valid for the selected file, in display order.
    pub(crate) fn delete_options(&self) -> Vec<crate::app::DeleteChoice> {
        use crate::app::DeleteChoice;
        let Some(rel) = self.selected_file() else {
            return Vec::new();
        };
        let has_gist = self
            .active_root_path()
            .is_some_and(|root| self.store.get(&root, &rel).is_some());
        if has_gist {
            vec![
                DeleteChoice::Remote,
                DeleteChoice::Local,
                DeleteChoice::Both,
            ]
        } else {
            vec![DeleteChoice::Local]
        }
    }

    pub(crate) fn confirm_delete_both(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
            return;
        };
        let Some(entry) = self.store.get(&root, &rel).cloned() else {
            self.status_message = "No gist to delete.".into();
            return;
        };
        self.mode = Mode::Confirm {
            message: format!("Delete the remote gist for {rel} AND move the local file to trash?"),
            action: ConfirmAction::DeleteBoth {
                rel_path: rel,
                root,
                remote_id: entry.remote_id,
            },
        };
    }

    pub(crate) fn confirm_delete_remote(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
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
                remote_id: entry.remote_id,
            },
        };
    }

    pub(crate) fn do_delete_remote(&mut self, rel_path: String, root: PathBuf, remote_id: String) {
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            return;
        };
        let tx = self.async_tx.clone();
        self.status_message = format!("Deleting gist for {rel_path}...");
        self.spawn_tracked(async move {
            let result = client.delete(&remote_id).await.map_err(|e| e.to_string());
            let _ = tx.send(AsyncEvent::DeleteDone {
                root,
                rel_path,
                result,
            });
        });
    }

    pub(crate) fn do_copy_url(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let entry = self
            .active_root_path()
            .and_then(|r| self.store.get(&r, &rel).cloned());
        if let Some(entry) = entry {
            self.copy_to_clipboard(&entry.url.clone());
        } else if self.pending_pushes.contains(&rel) {
            // A push for this file is already in flight (e.g. `c` right after
            // `u`). Just queue the copy onto it - spawning a second push would
            // race two "Pushed" messages and clobber "Copied".
            self.pending_copy = Some(rel.clone());
            self.status_message = format!("Push in progress for {rel}; will copy URL when done...");
        } else {
            // Queue the copy and trigger a push; PushDone will follow up.
            self.pending_copy = Some(rel.clone());
            self.do_sync_up();
            self.status_message = format!("Pushing {rel} first, then copy URL...");
        }
    }

    /// Copy a URL to the system clipboard, returning true on success.
    pub(crate) fn copy_to_clipboard(&mut self, url: &str) -> bool {
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

    pub(crate) fn do_diff(&mut self) {
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            return;
        };
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
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
        let remote_id = entry.remote_id.clone();
        let local_for_task = local_content.clone();
        let started = chrono::Utc::now();

        self.status_message = "Fetching remote for diff...".into();

        self.spawn_tracked(async move {
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
            remote: format!("(fetching remote content for {remote_id}...)"),
        };
    }

    /// Kick off a background remote check: list all gists once, fetch content
    /// for any whose `updated_at` moved since we last looked, and record the
    /// observed remote hashes. This is what makes remote edits show up as
    /// remote-newer/conflict in the tree without pulling anything. Driven by
    /// the poll timer in `tick()`; silent unless something actually changed.
    pub(crate) fn start_remote_check(&mut self) {
        if self.remote_check_inflight {
            return;
        }
        let Some(client) = self.gist_client.clone() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
            return;
        };
        let entries = self.store.gist_entries_for_root(&root);
        if entries.is_empty() {
            return;
        }
        let started = chrono::Utc::now();
        let tx = self.async_tx.clone();

        self.remote_check_inflight = true;

        self.spawn_tracked(async move {
            let result = crate::remote::check_remote(&client, &entries, |_, _| {}).await;
            let _ = tx.send(AsyncEvent::RemoteCheckDone {
                root,
                started,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    pub(crate) fn start_hydration(&mut self) {
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            return;
        };
        let Some(root) = self.active_root_path() else {
            self.status_message = "No active root.".into();
            return;
        };

        let tx = self.async_tx.clone();
        let files = self.files.clone();
        // Incremental cursor: only walk gists updated since this root's last
        // hydrate. None on a root's first walk → full. Capture the new cursor
        // *now*, before listing, so anything created during the walk is
        // re-examined next time rather than skipped.
        let since = self.store.hydrated_cursor(&root);
        let new_cursor = chrono::Utc::now();
        // Snapshot only this root's mappings; we'll merge results back when done.
        let mut store = Store::default();
        for (rel, entry) in self.store.gist_entries_for_root(&root) {
            store.insert(&root, rel, entry);
        }

        self.spawn_tracked(async move {
            let tx2 = tx.clone();
            let result = crate::hydrate::hydrate(
                &client,
                &mut store,
                &root,
                &files,
                since,
                move |progress| {
                    let _ = tx2.send(AsyncEvent::HydrationUpdate(progress));
                },
            )
            .await;
            let payload = result.map(|outcome| crate::event::HydrationDoneData {
                matched: outcome.matched,
                ambiguous: outcome.ambiguous,
                store: Box::new(store),
                root,
                new_cursor,
            });
            let _ = tx.send(AsyncEvent::HydrationDone(
                payload.map_err(|e| e.to_string()),
            ));
        });
    }

    /// Apply a user pick from the ambiguous resolver: write the chosen gist mapping
    /// into the store for the current root.
    pub(crate) fn apply_ambiguous_pick(&mut self, item: usize, candidate: usize) {
        let Some(root) = self.active_root_path() else {
            return;
        };
        let Some(am) = self.pending_ambiguous.get(item) else {
            return;
        };
        let Some(cand) = am.candidates.get(candidate) else {
            return;
        };
        let entry = FileEntry {
            backend: crate::store::GIST_BACKEND.into(),
            remote_id: cand.remote_id.clone(),
            url: cand.url.clone(),
            local_sha256: am.local_hash.clone(),
            remote_sha256: am.local_hash.clone(),
            last_synced: chrono::Utc::now(),
            remote_updated_at: None,
        };
        let rel = am.local_path.clone();
        // Take an owned copy of the URL so the `am`/`cand` borrow is released
        // before `store_mut()` (which borrows all of `self`).
        let cand_url = cand.url.clone();
        self.store_mut().insert(&root, rel.clone(), entry.clone());
        if let Err(e) = self.store.save() {
            self.status_message = format!("Resolved, but saving local state failed: {e}");
        } else {
            self.status_message = format!("Mapped {rel} → {cand_url}");
        }
        // The candidates here failed the content match, so the remote almost
        // certainly differs from local - fetch its real hash in the
        // background so the tree shows the divergence instead of a
        // fabricated "Synced".
        if let Some(client) = self.gist_client.clone() {
            let filename = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let local_content = std::fs::read_to_string(self.abs_path(&rel)).unwrap_or_default();
            let started = chrono::Utc::now();
            let tx = self.async_tx.clone();
            let rel_clone = rel.clone();
            self.spawn_tracked(async move {
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

    /// `L`: begin manually linking the selected file to an existing gist.
    /// Opens an input prompt for a gist URL or bare ID.
    pub(crate) fn start_manual_link(&mut self) {
        let Some(rel_path) = self.selected_file() else {
            self.status_message = "Select a file to link first.".into();
            return;
        };
        self.input_editor = LineEditor::new();
        self.mode = Mode::LinkGist { rel_path };
    }

    /// Fetch `raw` (a gist URL or ID), reconcile it against the local file at
    /// `rel_path`, and record the mapping. The fetch happens off-thread; the
    /// store write lands in the `LinkDone` handler.
    pub(crate) fn link_gist_to(&mut self, rel_path: String, raw: &str) {
        let Some(client) = self.gist_client.clone() else {
            self.status_message = crate::app::NO_TOKEN_HINT.into();
            return;
        };
        let Some(root) = self.active_root_path() else {
            self.status_message = "No active root.".into();
            return;
        };
        let Some(remote_id) = parse_gist_id(raw) else {
            self.status_message = format!("Couldn't read a gist ID from '{raw}'.");
            return;
        };
        // Read local content now (main thread) so the task only does network.
        let local_content = std::fs::read_to_string(self.abs_path(&rel_path)).unwrap_or_default();
        let filename = rel_path.rsplit('/').next().unwrap_or(&rel_path).to_string();
        let tx = self.async_tx.clone();
        self.status_message = format!("Linking {rel_path} → gist {remote_id}...");
        self.spawn_tracked(async move {
            let result = build_link_entry(&client, &remote_id, &filename, &local_content).await;
            let _ = tx.send(AsyncEvent::LinkDone {
                root,
                rel_path,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }
}

/// Reconcile a gist (fetched by ID) against a local file: pick the matching
/// file within the gist, fetch its content, and build a store entry carrying
/// both the local and the *observed* remote hash so the resulting status is
/// honest (Synced only when they truly match).
async fn build_link_entry(
    client: &penknife_gist::GistClient,
    remote_id: &str,
    local_filename: &str,
    local_content: &str,
) -> color_eyre::Result<FileEntry> {
    let gist = client.get(remote_id).await?;
    // Prefer the gist file whose name matches the local basename; fall back to
    // the sole file in a single-file gist. A multi-file gist with no name
    // match is genuinely ambiguous - refuse rather than guess.
    let chosen = if gist.files.contains_key(local_filename) {
        local_filename.to_string()
    } else if gist.files.len() == 1 {
        gist.files.keys().next().cloned().expect("len==1 has a key")
    } else if gist.files.is_empty() {
        color_eyre::eyre::bail!("Gist {remote_id} has no files.");
    } else {
        let names: Vec<&str> = gist.files.keys().map(|s| s.as_str()).collect();
        color_eyre::eyre::bail!(
            "Gist {remote_id} has multiple files ({}); none named '{local_filename}'. \
             Rename the local file to match one, or link via a single-file gist.",
            names.join(", ")
        );
    };
    let remote_content = client
        .file_content(&gist, &chosen)
        .await?
        .unwrap_or_default();
    Ok(FileEntry {
        backend: crate::store::GIST_BACKEND.into(),
        remote_id: gist.id.clone(),
        url: gist.html_url.clone(),
        local_sha256: sync::sha256_hex(local_content),
        remote_sha256: sync::sha256_hex(&remote_content),
        last_synced: chrono::Utc::now(),
        remote_updated_at: Some(gist.updated_at),
    })
}

/// Extract a gist ID from a URL or bare ID. Accepts the GitHub web URL form
/// (`https://gist.github.com/user/<id>` or `.../<id>`), an optional `.git`
/// suffix, trailing slashes, and a `#file-…` fragment. A bare token with no
/// slash is taken as the ID itself. Returns `None` if nothing ID-shaped
/// (alphanumeric) remains.
/// If a gist API error looks like a missing-scope 403, suggest the fix. A
/// token can be present and valid yet lack the `gist` scope (common when
/// `gh auth login` ran before you cared about gists), which surfaces as a
/// bare 403 on the first push. Point the user straight at the one command
/// that fixes it instead of leaving them to guess.
fn scope_hint(err: &str) -> &'static str {
    if err.contains("(403)") {
        " (your token may lack the 'gist' scope; run: gh auth refresh -s gist)"
    } else {
        ""
    }
}

fn parse_gist_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    // Drop any URL fragment / query, then trailing slashes.
    let no_frag = trimmed.split(['#', '?']).next().unwrap_or(trimmed);
    let no_slash = no_frag.trim_end_matches('/');
    // The ID is the last path segment, minus an optional `.git`.
    let last = no_slash.rsplit('/').next().unwrap_or(no_slash);
    let id = last.strip_suffix(".git").unwrap_or(last);
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(id.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod handler_tests {
    use super::super::App;
    use super::scope_hint;
    use crate::app::test_support::{guard, new_for_test, select, write_file};
    use crate::app::{ConfirmAction, DeleteChoice, Mode};
    use crate::event::{AsyncEvent, GistImportData, HydrationDoneData, async_channel};
    use crate::hydrate::{AmbiguousMatch, GistCandidate};
    use crate::remote::RemoteCheckOutcome;
    use crate::store::{FileEntry, GIST_BACKEND, Store};
    use crate::sync::{self, FullStatus, SyncStatus};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn app3() -> (TempDir, App, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.md", "alpha");
        write_file(dir.path(), "b.md", "beta");
        write_file(dir.path(), "sub/c.md", "gamma");
        let (tx, rx) = async_channel();
        std::mem::forget(rx);
        let app = new_for_test(dir.path(), tx);
        let root = app.active_root_path().unwrap();
        (dir, app, root)
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

    // ── PushDone ────────────────────────────────────────────────────────

    #[test]
    fn push_done_ok_inserts_entry_and_reports() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let h = sync::sha256_hex("alpha");
        app.handle_async_event(AsyncEvent::PushDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            result: Ok(entry("g1", &h, &h)),
        });
        assert_eq!(app.store.get(&root, "a.md").unwrap().remote_id, "g1");
        assert!(app.status_message.starts_with("Pushed a.md"));
        assert_eq!(app.cached_status("a.md"), SyncStatus::Synced);
    }

    #[test]
    fn push_done_err_reports_and_clears_pending_copy() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.pending_copy = Some("a.md".into());
        app.handle_async_event(AsyncEvent::PushDone {
            root,
            rel_path: "a.md".into(),
            result: Err("boom".into()),
        });
        assert!(app.status_message.starts_with("Push failed: boom"));
        assert!(app.pending_copy.is_none());
    }

    #[test]
    fn push_done_ok_with_pending_copy_clears_it() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let h = sync::sha256_hex("alpha");
        app.pending_copy = Some("a.md".into());
        app.handle_async_event(AsyncEvent::PushDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            result: Ok(entry("g1", &h, &h)),
        });
        // The queued copy fires (clipboard may be unavailable, but the
        // request is consumed either way).
        assert!(app.pending_copy.is_none());
        assert!(app.store.get(&root, "a.md").is_some());
    }

    #[test]
    fn push_done_clears_pending_push_and_skips_pushed_when_copying() {
        // Belt-and-suspenders (fix #2): when a PushDone both lands the push
        // and fulfils a queued copy, the intermediate "Pushed" line is never
        // shown - so a later, redundant push's "Pushed" echo can't clobber
        // "Copied". Also confirms the in-flight guard is cleared.
        let _g = guard();
        let (_d, mut app, root) = app3();
        let h = sync::sha256_hex("alpha");
        app.pending_copy = Some("a.md".into());
        app.pending_pushes.insert("a.md".into());
        app.handle_async_event(AsyncEvent::PushDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            result: Ok(entry("g1", &h, &h)),
        });
        assert!(app.pending_pushes.is_empty());
        assert!(app.pending_copy.is_none());
        // Either "Copied: …" or a clipboard error (headless CI), but never the
        // intermediate "Pushed …" that the second push would race against.
        assert!(!app.status_message.starts_with("Pushed"));
    }

    #[test]
    fn copy_url_queues_onto_inflight_push_without_respawning() {
        // Fix #1: pressing `c` while a push for the same fresh file is already
        // in flight must not spawn a second push - it just queues the copy.
        let _g = guard();
        let (_d, mut app, _root) = app3();
        select(&mut app, "a.md");
        // Simulate a push already running for a.md (as `u` would have set).
        app.pending_pushes.insert("a.md".into());
        app.do_copy_url();
        assert_eq!(app.pending_copy.as_deref(), Some("a.md"));
        assert!(app.status_message.contains("Push in progress"));
        // No second push was spawned (do_sync_up was skipped).
        assert!(app.tasks.is_empty());
    }

    #[test]
    fn do_sync_up_for_debounces_second_push_but_not_force() {
        // A non-force push for a file already in flight is dropped (guards the
        // double-`u` duplicate-gist race); a force push always proceeds.
        let _g = guard();
        let (_d, mut app, _root) = app3();
        app.pending_pushes.insert("a.md".into());
        app.do_sync_up_for("a.md".into(), false);
        assert!(app.status_message.contains("Already pushing"));
        // No task spawned - the guard returned before spawn_tracked.
        assert!(app.tasks.is_empty());
        // Force pushes bypass the guard; without a token it stops at the token
        // check rather than the debounce, proving it got past the guard.
        app.do_sync_up_for("a.md".into(), true);
        assert!(!app.status_message.contains("Already pushing"));
    }

    // ── PullDone ────────────────────────────────────────────────────────

    #[test]
    fn pull_done_applies_when_local_unchanged() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let expected = sync::sha256_hex("alpha");
        app.handle_async_event(AsyncEvent::PullDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            expected_local_sha256: expected,
            result: Ok(("remote body".into(), entry("g1", "x", "x"))),
        });
        assert_eq!(app.status_message, "Pulled a.md");
        let on_disk = std::fs::read_to_string(root.join("a.md")).unwrap();
        assert_eq!(on_disk, "remote body");
    }

    #[test]
    fn pull_done_refuses_on_drift() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.handle_async_event(AsyncEvent::PullDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            // Stale hash: does not match the "alpha" on disk.
            expected_local_sha256: sync::sha256_hex("something else"),
            result: Ok(("remote body".into(), entry("g1", "x", "x"))),
        });
        assert!(app.status_message.contains("changed on disk"));
        // Disk content preserved.
        assert_eq!(std::fs::read_to_string(root.join("a.md")).unwrap(), "alpha");
    }

    #[test]
    fn pull_done_err_reports() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.handle_async_event(AsyncEvent::PullDone {
            root,
            rel_path: "a.md".into(),
            expected_local_sha256: "x".into(),
            result: Err("net down".into()),
        });
        assert!(app.status_message.starts_with("Pull failed: net down"));
    }

    // ── PushBlocked ─────────────────────────────────────────────────────

    #[test]
    fn push_blocked_records_divergence_and_prompts_force() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        // Seed a mapping so record_divergence has something to update.
        let h = sync::sha256_hex("alpha");
        app.store_mut()
            .insert(&root, "a.md".into(), entry("g1", &h, &h));
        app.refresh_status_cache();
        // A blocked push must also release the in-flight guard.
        app.pending_pushes.insert("a.md".into());
        app.handle_async_event(AsyncEvent::PushBlocked {
            root: root.clone(),
            rel_path: "a.md".into(),
            remote_sha256: "different".into(),
            remote_updated_at: chrono::Utc::now(),
        });
        assert!(app.pending_pushes.is_empty());
        assert!(matches!(
            app.mode,
            Mode::Confirm {
                action: ConfirmAction::ForcePush { .. },
                ..
            }
        ));
        assert!(app.status_message.contains("Push blocked"));
        // Divergence recorded => now RemoteNewer.
        assert_eq!(app.cached_status("a.md"), SyncStatus::RemoteNewer);
    }

    // ── RemoteCheckDone ─────────────────────────────────────────────────

    #[test]
    fn remote_check_ok_applies_updates_and_resets_failures() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let started = chrono::Utc::now();
        // Existing mapping older than the check.
        let mut cur = entry("g1", "l", "r");
        cur.last_synced = started - chrono::Duration::seconds(60);
        app.store_mut().insert(&root, "a.md".into(), cur);
        app.remote_poll_failures = 3;
        app.remote_check_inflight = true;

        let refreshed = entry("g1", "l", "new-remote");
        app.handle_async_event(AsyncEvent::RemoteCheckDone {
            root: root.clone(),
            started,
            result: Ok(RemoteCheckOutcome {
                updated: vec![("a.md".into(), refreshed)],
                divergent: 1,
                missing: vec![],
                checked: 1,
            }),
        });
        assert!(!app.remote_check_inflight);
        assert_eq!(app.remote_poll_failures, 0);
        assert_eq!(
            app.store.get(&root, "a.md").unwrap().remote_sha256,
            "new-remote"
        );
        assert!(app.status_message.contains("1 of 1 changed"));
    }

    #[test]
    fn remote_check_err_increments_failures() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.remote_check_inflight = true;
        app.handle_async_event(AsyncEvent::RemoteCheckDone {
            root,
            started: chrono::Utc::now(),
            result: Err("offline".into()),
        });
        assert_eq!(app.remote_poll_failures, 1);
        assert!(app.status_message.contains("Remote check failed"));
        // A second failure backs off silently (no status change).
        let (_d2, mut app2, root2) = app3();
        app2.remote_poll_failures = 1;
        app2.status_message = "unchanged".into();
        app2.handle_async_event(AsyncEvent::RemoteCheckDone {
            root: root2,
            started: chrono::Utc::now(),
            result: Err("still offline".into()),
        });
        assert_eq!(app2.remote_poll_failures, 2);
        assert_eq!(app2.status_message, "unchanged");
    }

    // ── HydrationDone ───────────────────────────────────────────────────

    #[test]
    fn hydration_done_ok_merges_and_sets_cursor() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let mut discovered = Store::default();
        let h = sync::sha256_hex("alpha");
        discovered.insert(&root, "a.md".into(), entry("g1", &h, &h));
        let cursor = chrono::Utc::now();
        app.handle_async_event(AsyncEvent::HydrationDone(Ok(HydrationDoneData {
            matched: 1,
            ambiguous: vec![],
            store: Box::new(discovered),
            root: root.clone(),
            new_cursor: cursor,
        })));
        assert!(app.store.get(&root, "a.md").is_some());
        assert_eq!(app.store.hydrated_cursor(&root), Some(cursor));
        assert!(app.status_message.contains("Hydrated 1 file"));
    }

    #[test]
    fn hydration_done_with_ambiguous_sets_pending() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let am = AmbiguousMatch {
            local_path: "a.md".into(),
            local_hash: "h".into(),
            candidates: vec![GistCandidate {
                remote_id: "g1".into(),
                url: "u".into(),
                description: None,
                size: 1,
            }],
        };
        app.handle_async_event(AsyncEvent::HydrationDone(Ok(HydrationDoneData {
            matched: 0,
            ambiguous: vec![am],
            store: Box::new(Store::default()),
            root,
            new_cursor: chrono::Utc::now(),
        })));
        assert_eq!(app.pending_ambiguous.len(), 1);
        assert!(app.status_message.contains("press M to resolve"));
    }

    #[test]
    fn hydration_done_err_reports() {
        let _g = guard();
        let (_d, mut app, _root) = app3();
        app.handle_async_event(AsyncEvent::HydrationDone(Err("bad token".into())));
        assert!(app.status_message.starts_with("Hydration error: bad token"));
    }

    // ── DeleteDone ──────────────────────────────────────────────────────

    #[test]
    fn delete_done_ok_removes_mapping() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.store_mut()
            .insert(&root, "a.md".into(), entry("g1", "x", "x"));
        app.handle_async_event(AsyncEvent::DeleteDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            result: Ok(()),
        });
        // The mapping is gone; the transient "Deleted" message is immediately
        // replaced by the refreshed status dashboard (update_status runs last).
        assert!(app.store.get(&root, "a.md").is_none());
    }

    #[test]
    fn delete_done_err_reports() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.handle_async_event(AsyncEvent::DeleteDone {
            root,
            rel_path: "a.md".into(),
            result: Err("403".into()),
        });
        assert!(app.status_message.starts_with("Delete failed: 403"));
    }

    // ── LinkDone ────────────────────────────────────────────────────────

    #[test]
    fn link_done_ok_in_sync_reports() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        let h = sync::sha256_hex("alpha");
        app.handle_async_event(AsyncEvent::LinkDone {
            root: root.clone(),
            rel_path: "a.md".into(),
            result: Ok(entry("g1", &h, &h)),
        });
        assert!(app.store.get(&root, "a.md").is_some());
        assert!(app.status_message.contains("in sync"));
    }

    #[test]
    fn link_done_ok_differs_reports_diff_hint() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.handle_async_event(AsyncEvent::LinkDone {
            root,
            rel_path: "a.md".into(),
            result: Ok(entry("g1", "local", "remote")),
        });
        assert!(app.status_message.contains("differs"));
    }

    #[test]
    fn link_done_err_reports() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.handle_async_event(AsyncEvent::LinkDone {
            root,
            rel_path: "a.md".into(),
            result: Err("not found".into()),
        });
        assert!(app.status_message.starts_with("Link failed: not found"));
    }

    // ── RenameRemoteDone ────────────────────────────────────────────────

    #[test]
    fn rename_remote_done_ok_and_err() {
        let (_d, mut app, _root) = app3();
        app.handle_async_event(AsyncEvent::RenameRemoteDone {
            rel_path: "new.md".into(),
            result: Ok(()),
        });
        assert!(app.status_message.contains("Renamed (local + remote)"));
        app.handle_async_event(AsyncEvent::RenameRemoteDone {
            rel_path: "new.md".into(),
            result: Err("nope".into()),
        });
        assert!(app.status_message.contains("remote rename failed"));
    }

    // ── StatusCheck ─────────────────────────────────────────────────────

    #[test]
    fn status_check_ok_updates_diff_remote() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.mode = Mode::Diff {
            local: "alpha".into(),
            remote: "(fetching...)".into(),
        };
        app.handle_async_event(AsyncEvent::StatusCheck {
            root,
            rel_path: "a.md".into(),
            started: chrono::Utc::now(),
            result: Ok(FullStatus {
                status: SyncStatus::Synced,
                remote_content: "remote text".into(),
                remote_sha256: sync::sha256_hex("remote text"),
                remote_updated_at: chrono::Utc::now(),
            }),
        });
        if let Mode::Diff { remote, .. } = &app.mode {
            assert_eq!(remote, "remote text");
        } else {
            panic!("expected Diff mode");
        }
        assert!(app.status_message.contains("a.md"));
    }

    #[test]
    fn status_check_err_reports() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.handle_async_event(AsyncEvent::StatusCheck {
            root,
            rel_path: "a.md".into(),
            started: chrono::Utc::now(),
            result: Err("timeout".into()),
        });
        assert!(
            app.status_message
                .starts_with("Status check failed: timeout")
        );
    }

    // ── GdocFetched / GistImportFetched ─────────────────────────────────

    #[test]
    fn gdoc_fetched_ok_opens_filename_prompt() {
        let (_d, mut app, _root) = app3();
        app.handle_async_event(AsyncEvent::GdocFetched(Ok("# Doc".into())));
        assert!(matches!(app.mode, Mode::GdocFilename));
        assert_eq!(app.gdoc_content.as_deref(), Some("# Doc"));
    }

    #[test]
    fn gdoc_fetched_err_shows_message() {
        let (_d, mut app, _root) = app3();
        app.handle_async_event(AsyncEvent::GdocFetched(Err("404".into())));
        assert!(matches!(app.mode, Mode::Message(_)));
    }

    #[test]
    fn gist_import_fetched_prefills_filename() {
        let (_d, mut app, _root) = app3();
        app.handle_async_event(AsyncEvent::GistImportFetched(Ok(GistImportData {
            content: "body".into(),
            filename: "notes.md".into(),
            entry: entry("g1", "x", "x"),
        })));
        assert!(matches!(app.mode, Mode::GdocFilename));
        assert_eq!(app.gdoc_content.as_deref(), Some("body"));
        assert!(app.pending_import_entry.is_some());
        // The .md suffix is stripped from the prefilled name.
        assert_eq!(app.input_editor.content, "notes");
    }

    #[test]
    fn gist_import_fetched_err_shows_message() {
        let (_d, mut app, _root) = app3();
        app.handle_async_event(AsyncEvent::GistImportFetched(Err("multi-file".into())));
        assert!(matches!(app.mode, Mode::Message(_)));
    }

    // ── delete_options / apply_ambiguous_pick / scope_hint ──────────────

    #[test]
    fn delete_options_local_only_without_gist() {
        let (_d, mut app, _root) = app3();
        select(&mut app, "a.md");
        assert_eq!(app.delete_options(), vec![DeleteChoice::Local]);
    }

    #[test]
    fn delete_options_all_when_gisted() {
        let (_d, mut app, root) = app3();
        app.store_mut()
            .insert(&root, "a.md".into(), entry("g1", "x", "x"));
        select(&mut app, "a.md");
        assert_eq!(
            app.delete_options(),
            vec![
                DeleteChoice::Remote,
                DeleteChoice::Local,
                DeleteChoice::Both
            ]
        );
    }

    #[test]
    fn delete_options_empty_without_selection() {
        let (_d, app, _root) = app3();
        assert!(app.delete_options().is_empty());
    }

    #[test]
    fn apply_ambiguous_pick_records_mapping() {
        let _g = guard();
        let (_d, mut app, root) = app3();
        app.pending_ambiguous.push(AmbiguousMatch {
            local_path: "a.md".into(),
            local_hash: sync::sha256_hex("alpha"),
            candidates: vec![GistCandidate {
                remote_id: "g1".into(),
                url: "https://gist.github.com/u/g1".into(),
                description: None,
                size: 1,
            }],
        });
        app.apply_ambiguous_pick(0, 0);
        let mapped = app.store.get(&root, "a.md").unwrap();
        assert_eq!(mapped.remote_id, "g1");
        assert!(app.status_message.contains("Mapped a.md"));
    }

    #[test]
    fn scope_hint_flags_403() {
        assert!(scope_hint("boom (403)").contains("scope"));
        assert_eq!(scope_hint("some other error"), "");
    }

    #[test]
    fn scan_progress_updates_count_and_refresh_done_clears_scanning() {
        let (_d, mut app, root) = app3();
        app.scanning = true;
        let generation = app.refresh_generation;

        // Progress from the in-flight scan bumps the counter.
        app.handle_async_event(AsyncEvent::ScanProgress {
            generation,
            count: 512,
        });
        assert_eq!(app.scan_count, 512);

        // Progress from a superseded scan is ignored.
        app.handle_async_event(AsyncEvent::ScanProgress {
            generation: generation + 9,
            count: 999,
        });
        assert_eq!(app.scan_count, 512);

        // The refresh landing leaves the scanning state.
        app.handle_async_event(AsyncEvent::RefreshDone {
            generation,
            root,
            files: Vec::new(),
            status_cache: std::collections::HashMap::new(),
            git_repo_root: None,
            git_statuses: std::collections::HashMap::new(),
            select: None,
            only_if_changed: false,
            done_message: None,
        });
        assert!(!app.scanning);
        assert_eq!(app.scan_count, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_gist_id;

    #[test]
    fn parses_full_web_url() {
        assert_eq!(
            parse_gist_id("https://gist.github.com/jhheider/0828a8e1bbdb66e7e082b054c9e975e3"),
            Some("0828a8e1bbdb66e7e082b054c9e975e3".to_string())
        );
    }

    #[test]
    fn parses_bare_id() {
        assert_eq!(
            parse_gist_id("0828a8e1bbdb66e7e082b054c9e975e3"),
            Some("0828a8e1bbdb66e7e082b054c9e975e3".to_string())
        );
    }

    #[test]
    fn strips_git_suffix_trailing_slash_and_fragment() {
        assert_eq!(
            parse_gist_id("https://gist.github.com/u/abc123.git"),
            Some("abc123".to_string())
        );
        assert_eq!(
            parse_gist_id("https://gist.github.com/u/abc123/"),
            Some("abc123".to_string())
        );
        assert_eq!(
            parse_gist_id("https://gist.github.com/u/abc123#file-foo-md"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn rejects_empty_or_nonalphanumeric() {
        // Nothing ID-shaped left → None. A merely-wrong-but-alphanumeric
        // segment is accepted and left to 404 on the actual fetch.
        assert_eq!(parse_gist_id(""), None);
        assert_eq!(parse_gist_id("   "), None);
        assert_eq!(parse_gist_id("not a/url!"), None);
    }
}
