//! Google Docs publish flow: the `p` menu, the device-flow sign-in, and the
//! store bookkeeping for a file's gdoc copy.
//!
//! Publish is push-only (the backend is `BackendKind::Publish`): create makes
//! a real Doc from the markdown, update replaces the Doc wholesale, and
//! unpublish deletes it. There is deliberately no pull; a Doc edited on the
//! Google side is a "diverged, view in browser" situation, never a silent
//! overwrite of the local file.

use penknife_backend::Backend;

use super::{App, ConfirmAction, Mode, PublishChoice};
use crate::event::{AsyncEvent, GdocPublishData};
use crate::store::GDOC_BACKEND;
use crate::sync;

impl App {
    /// Open the publish menu for the selected file, or explain what's
    /// missing (a file, or OAuth credentials).
    pub(crate) fn open_publish_menu(&mut self) {
        if self.selected_file().is_none() {
            self.status_message = "No file selected.".into();
            return;
        }
        if self.gdoc_client.is_none() {
            self.status_message = "Publishing needs a Google OAuth client: set [gdoc] client_id \
                                   and client_secret in config.toml (penknife --config)."
                .into();
            return;
        }
        self.mode = Mode::PublishMenu { selected: 0 };
    }

    /// The publish choices valid for the selected file, in display order.
    pub(crate) fn publish_options(&self) -> Vec<PublishChoice> {
        let Some(rel) = self.selected_file() else {
            return Vec::new();
        };
        let Some(root) = self.active_root_path() else {
            return Vec::new();
        };
        if self.store.get_backend(&root, &rel, GDOC_BACKEND).is_some() {
            vec![
                PublishChoice::Update,
                PublishChoice::Open,
                PublishChoice::CopyUrl,
                PublishChoice::Unpublish,
            ]
        } else {
            vec![PublishChoice::Create]
        }
    }

    pub(crate) fn run_publish_choice(&mut self, choice: PublishChoice) {
        self.mode = Mode::Normal;
        let Some(rel) = self.selected_file() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
            return;
        };
        match choice {
            PublishChoice::Create | PublishChoice::Update => self.start_gdoc_publish(rel),
            PublishChoice::Open => {
                if let Some(copy) = self.store.get_backend(&root, &rel, GDOC_BACKEND) {
                    let url = copy.url.clone();
                    if let Err(e) = open::that(&url) {
                        self.status_message = format!("Failed to open browser: {e}");
                    } else {
                        self.status_message = format!("Opened {url}");
                    }
                }
            }
            PublishChoice::CopyUrl => {
                if let Some(copy) = self.store.get_backend(&root, &rel, GDOC_BACKEND) {
                    let url = copy.url.clone();
                    self.copy_to_clipboard(&url);
                }
            }
            PublishChoice::Unpublish => {
                if let Some(copy) = self.store.get_backend(&root, &rel, GDOC_BACKEND) {
                    self.mode = Mode::Confirm {
                        message: format!(
                            "Delete the Google Doc for {rel}? The local file is untouched."
                        ),
                        action: ConfirmAction::GdocUnpublish {
                            rel_path: rel,
                            root,
                            remote_id: copy.remote_id.clone(),
                        },
                    };
                }
            }
        }
    }

    /// Publish `rel` to Google Docs: create on first publish, replace after.
    /// If the token cache is missing or dead, this detours through the
    /// device flow first and resumes via the `GdocAuthDone` handler.
    pub(crate) fn start_gdoc_publish(&mut self, rel: String) {
        let Some(client) = self.gdoc_client.clone() else {
            return;
        };
        let Some(root) = self.active_root_path() else {
            return;
        };
        let abs = root.join(&rel);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Read failed: {e}");
                return;
            }
        };
        let filename = std::path::Path::new(&rel)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| rel.clone());
        let existing = self.store.get_backend(&root, &rel, GDOC_BACKEND).cloned();
        let tx = self.async_tx.clone();

        self.pending_publish = Some(rel.clone());
        self.status_message = format!("Publishing {rel} to Google Docs...");

        let handle = tokio::spawn(async move {
            // Auth first. NotAuthenticated detours through the device flow;
            // the publish itself re-fires from the GdocAuthDone handler.
            match client.auth().access_token().await {
                Ok(_) => {}
                Err(penknife_gdoc::GdocError::NotAuthenticated) => {
                    match client.auth().start_device_flow().await {
                        Ok(da) => {
                            let _ = tx.send(AsyncEvent::GdocAuthPrompt {
                                user_code: da.user_code.clone(),
                                verification_url: da.verification_url.clone(),
                            });
                            let result = client
                                .auth()
                                .poll_device_flow(&da)
                                .await
                                .map(|_| ())
                                .map_err(|e| e.to_string());
                            let _ = tx.send(AsyncEvent::GdocAuthDone(result));
                        }
                        Err(e) => {
                            let _ = tx.send(AsyncEvent::GdocAuthDone(Err(e.to_string())));
                        }
                    }
                    return;
                }
                Err(e) => {
                    let _ = tx.send(AsyncEvent::GdocPublishDone {
                        root,
                        rel_path: rel,
                        result: Err(e.to_string()),
                    });
                    return;
                }
            }

            let content_sha = sync::sha256_hex(&content);
            let result = match &existing {
                Some(copy) => Backend::update(&*client, &copy.remote_id, &filename, &content)
                    .await
                    .map(|r| (r, true)),
                None => Backend::create(&*client, &filename, &content, "")
                    .await
                    .map(|r| (r, false)),
            };
            let payload = result
                .map(|(r, updated)| GdocPublishData {
                    remote_id: r.remote_id,
                    // Media updates may omit webViewLink; keep the known URL.
                    url: if r.url.is_empty() {
                        existing.as_ref().map(|c| c.url.clone()).unwrap_or_default()
                    } else {
                        r.url
                    },
                    revision: r.revision,
                    content_sha,
                    updated,
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(AsyncEvent::GdocPublishDone {
                root,
                rel_path: rel,
                result: payload,
            });
        });
        self.gdoc_auth_abort = Some(handle.abort_handle());
        self.tasks.retain(|h| !h.is_finished());
        self.tasks.push(handle);
    }

    /// Delete the Google Doc for `rel` (confirmed already). Local file and
    /// other backends' copies are untouched.
    pub(crate) fn do_gdoc_unpublish(
        &mut self,
        rel_path: String,
        root: std::path::PathBuf,
        remote_id: String,
    ) {
        let Some(client) = self.gdoc_client.clone() else {
            return;
        };
        let tx = self.async_tx.clone();
        self.status_message = format!("Deleting the Google Doc for {rel_path}...");
        self.spawn_tracked(async move {
            let result = Backend::delete(&*client, &remote_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AsyncEvent::GdocUnpublishDone {
                root,
                rel_path,
                result,
            });
        });
    }
}
