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
                    match sync_apply::apply_pull(
                        &mut self.store,
                        &root,
                        &rel_path,
                        &expected_local_sha256,
                        &content,
                        entry,
                    ) {
                        Ok(PullApply::DriftRefused) => {
                            self.status_message = format!(
                                "{rel_path} changed on disk during pull — not overwriting. Pull again to retry."
                            );
                        }
                        Ok(PullApply::Applied) => {
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
                // Record the observed divergence so the tree shows the real
                // state (RemoteNewer/Conflict) even if the user declines.
                if sync_apply::record_divergence(
                    &mut self.store,
                    &root,
                    &rel_path,
                    remote_sha256,
                    remote_updated_at,
                ) {
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
                    let applied = sync_apply::apply_remote_updates(
                        &mut self.store,
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
                    // Advance this root's incremental cursor so the next
                    // hydrate only fetches gists changed after this walk began.
                    self.store.set_hydrated_cursor(&data.root, data.new_cursor);
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
                    if sync_apply::record_observation(
                        &mut self.store,
                        &root,
                        &rel_path,
                        started,
                        full.remote_sha256.clone(),
                        full.remote_updated_at,
                    ) {
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
                    self.store.remove(&root, &rel_path);
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
            AsyncEvent::LinkDone {
                root,
                rel_path,
                result,
            } => match result {
                Ok(entry) => {
                    let url = entry.url.clone();
                    let synced = entry.local_sha256 == entry.remote_sha256;
                    self.store.insert(&root, rel_path.clone(), entry);
                    if let Err(e) = self.store.save() {
                        self.status_message = format!("Linked but store save failed: {e}");
                        return;
                    }
                    self.refresh_status_for(&rel_path);
                    self.rebuild_tree();
                    self.update_status();
                    self.status_message = if synced {
                        format!("Linked {rel_path} → {url} (in sync)")
                    } else {
                        format!("Linked {rel_path} → {url} (differs — D to diff, u/d to reconcile)")
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
        }
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
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
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
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
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
                gist_id: entry.gist_id,
            },
        };
    }

    pub(crate) fn do_delete_remote(&mut self, rel_path: String, root: PathBuf, gist_id: String) {
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

    pub(crate) fn do_copy_url(&mut self) {
        let Some(rel) = self.selected_file() else {
            return;
        };
        let entry = self
            .active_root_path()
            .and_then(|r| self.store.get(&r, &rel).cloned());
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

    pub(crate) fn do_diff(&mut self) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
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
    /// remote-newer/conflict in the tree without pulling anything.
    pub(crate) fn start_remote_check(&mut self) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.active_root_path() else {
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

    pub(crate) fn start_hydration(&mut self) {
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.active_root_path() else {
            self.status_message = "No active root.".into();
            return;
        };

        self.mode = Mode::Hydrating {
            progress: None,
            done: false,
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
        if let Some(map) = self.store.files_for_root(&root) {
            for (rel, entry) in map {
                store.insert(&root, rel.clone(), entry.clone());
            }
        }

        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
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
        let Some(token) = self.token.clone() else {
            self.status_message = "No GitHub token available.".into();
            return;
        };
        let Some(root) = self.active_root_path() else {
            self.status_message = "No active root.".into();
            return;
        };
        let Some(gist_id) = parse_gist_id(raw) else {
            self.status_message = format!("Couldn't read a gist ID from '{raw}'.");
            return;
        };
        // Read local content now (main thread) so the task only does network.
        let local_content = std::fs::read_to_string(self.abs_path(&rel_path)).unwrap_or_default();
        let filename = rel_path.rsplit('/').next().unwrap_or(&rel_path).to_string();
        let tx = self.async_tx.clone();
        self.status_message = format!("Linking {rel_path} → gist {gist_id}...");
        self.spawn_tracked(async move {
            let client = gist_rs::GistClient::new(token);
            let result = build_link_entry(&client, &gist_id, &filename, &local_content).await;
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
    client: &gist_rs::GistClient,
    gist_id: &str,
    local_filename: &str,
    local_content: &str,
) -> crate::error::Result<FileEntry> {
    use crate::error::WmError;
    let gist = client.get(gist_id).await?;
    // Prefer the gist file whose name matches the local basename; fall back to
    // the sole file in a single-file gist. A multi-file gist with no name
    // match is genuinely ambiguous — refuse rather than guess.
    let chosen = if gist.files.contains_key(local_filename) {
        local_filename.to_string()
    } else if gist.files.len() == 1 {
        gist.files.keys().next().cloned().expect("len==1 has a key")
    } else if gist.files.is_empty() {
        return Err(WmError::Other(format!("Gist {gist_id} has no files.")));
    } else {
        let names: Vec<&str> = gist.files.keys().map(|s| s.as_str()).collect();
        return Err(WmError::Other(format!(
            "Gist {gist_id} has multiple files ({}); none named '{local_filename}'. \
             Rename the local file to match one, or link via a single-file gist.",
            names.join(", ")
        )));
    };
    let remote_content = client
        .file_content(&gist, &chosen)
        .await?
        .unwrap_or_default();
    Ok(FileEntry {
        gist_id: gist.id.clone(),
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
