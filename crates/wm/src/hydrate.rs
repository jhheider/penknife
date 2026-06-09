use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use gist_rs::{Gist, GistClient};

use crate::error::Result;
use crate::scanner::ScannedFile;
use crate::store::{FileEntry, Store};
use crate::sync::sha256_hex;

#[derive(Debug)]
pub struct HydrationProgress {
    pub phase: String,
    pub matched: usize,
    pub total_gists: usize,
    pub total_files: usize,
    pub current_file: usize,
    pub ambiguous: Vec<AmbiguousMatch>,
}

#[derive(Debug, Clone)]
pub struct AmbiguousMatch {
    pub local_path: String,
    pub local_hash: String,
    pub candidates: Vec<GistCandidate>,
}

#[derive(Debug, Clone)]
pub struct GistCandidate {
    pub gist_id: String,
    pub url: String,
    pub description: Option<String>,
    pub size: u64,
}

pub struct HydrationOutcome {
    pub matched: usize,
    pub ambiguous: Vec<AmbiguousMatch>,
}

/// Run the full hydration algorithm. Calls `progress_cb` with updates.
/// Mutations are scoped to `root` in the store. The caller is responsible
/// for persisting `store` after merging results.
pub async fn hydrate(
    client: &GistClient,
    store: &mut Store,
    root: &Path,
    files: &[ScannedFile],
    mut progress_cb: impl FnMut(HydrationProgress),
) -> Result<HydrationOutcome> {
    let total_files = files.len();

    // Phase 1: Fetch all gists page by page
    progress_cb(HydrationProgress {
        phase: "Fetching gists... page 1".into(),
        matched: 0,
        total_gists: 0,
        total_files,
        current_file: 0,
        ambiguous: vec![],
    });

    let mut all_gists: Vec<Gist> = Vec::new();
    let mut page = 1u32;
    loop {
        let result = client.list_page(page).await?;
        all_gists.extend(result.gists);
        if !result.has_next {
            break;
        }
        page += 1;
        progress_cb(HydrationProgress {
            phase: format!(
                "Fetching gists... page {} ({} so far)",
                page,
                all_gists.len()
            ),
            matched: 0,
            total_gists: all_gists.len(),
            total_files,
            current_file: 0,
            ambiguous: vec![],
        });
    }

    let total_gists = all_gists.len();

    progress_cb(HydrationProgress {
        phase: format!("Fetched {total_gists} gists. Building index..."),
        matched: 0,
        total_gists,
        total_files,
        current_file: 0,
        ambiguous: vec![],
    });

    // Phase 2: Build reverse index — filename → [Gist]
    let mut by_filename: HashMap<String, Vec<&Gist>> = HashMap::new();
    for gist in &all_gists {
        for filename in gist.files.keys() {
            by_filename.entry(filename.clone()).or_default().push(gist);
        }
    }

    // Build set of local filenames to rel_paths
    let mut local_by_filename: HashMap<String, Vec<&ScannedFile>> = HashMap::new();
    for file in files {
        let filename = file
            .abs_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        local_by_filename.entry(filename).or_default().push(file);
    }

    let mut matched = 0usize;
    let mut ambiguous = Vec::new();

    // Phase 3: Unique matches
    for (i, file) in files.iter().enumerate() {
        // Skip already-mapped files
        if store.get(root, &file.rel_path).is_some() {
            matched += 1;
            // Send progress every 10 files
            if i % 10 == 0 {
                progress_cb(HydrationProgress {
                    phase: format!("Matching files... ({}/{})", i + 1, total_files),
                    matched,
                    total_gists,
                    total_files,
                    current_file: i + 1,
                    ambiguous: vec![],
                });
            }
            continue;
        }

        let filename = file
            .abs_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let Some(gists) = by_filename.get(&filename) else {
            if i % 10 == 0 {
                progress_cb(HydrationProgress {
                    phase: format!("Matching files... ({}/{})", i + 1, total_files),
                    matched,
                    total_gists,
                    total_files,
                    current_file: i + 1,
                    ambiguous: vec![],
                });
            }
            continue;
        };

        let locals = local_by_filename
            .get(&filename)
            .map(|v| v.len())
            .unwrap_or(0);

        if gists.len() == 1 && locals == 1 {
            // Unique filename match. Fetch the gist's actual content so the
            // stored remote hash is real — recording the local hash here
            // would fabricate a "Synced" state even when the remote differs,
            // and the next push would silently overwrite it.
            let gist = gists[0];
            let local_content = match std::fs::read_to_string(&file.abs_path) {
                Ok(c) => c,
                Err(_) => {
                    // File disappeared or unreadable — skip rather than fabricate empty match.
                    continue;
                }
            };
            let Some(entry) = verified_entry(client, &gist.id, &filename, &local_content).await
            else {
                continue;
            };
            store.insert(root, file.rel_path.clone(), entry);
            matched += 1;
        }

        if i % 10 == 0 {
            progress_cb(HydrationProgress {
                phase: format!("Matching files... ({}/{})", i + 1, total_files),
                matched,
                total_gists,
                total_files,
                current_file: i + 1,
                ambiguous: vec![],
            });
        }
    }

    progress_cb(HydrationProgress {
        phase: format!("Unique matches: {matched}. Disambiguating..."),
        matched,
        total_gists,
        total_files,
        current_file: total_files,
        ambiguous: vec![],
    });

    // Phase 4: Disambiguate by content (SHA-256 compare)
    for (i, file) in files.iter().enumerate() {
        if store.get(root, &file.rel_path).is_some() {
            continue;
        }

        let filename = file
            .abs_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let Some(gists) = by_filename.get(&filename) else {
            continue;
        };
        if gists.len() <= 1 {
            continue;
        }

        // Per-file progress during disambiguation
        progress_cb(HydrationProgress {
            phase: format!("Disambiguating: {} ({}/{})", filename, i + 1, total_files),
            matched,
            total_gists,
            total_files,
            current_file: i + 1,
            ambiguous: ambiguous.clone(),
        });

        let local_content = match std::fs::read_to_string(&file.abs_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let local_hash = sha256_hex(&local_content);
        let local_size = local_content.len() as u64;

        // Pre-filter by size, then fetch content to compare
        let mut size_matches: Vec<&Gist> = gists
            .iter()
            .filter(|g| {
                g.files
                    .get(&filename)
                    .is_some_and(|f| f.size.abs_diff(local_size) < 100)
            })
            .copied()
            .collect();

        if size_matches.len() == 1 {
            // Size narrowed it to one candidate, but size-similar ≠
            // identical — fetch the content so the remote hash is real.
            let gist = size_matches[0];
            if let Some(entry) = verified_entry(client, &gist.id, &filename, &local_content).await {
                store.insert(root, file.rel_path.clone(), entry);
                matched += 1;
                continue;
            }
        }

        // If size filter didn't help enough, try fetching content
        if size_matches.is_empty() {
            size_matches = gists.to_vec();
        }

        let mut content_match = None;
        for gist in &size_matches {
            if let Ok(full_gist) = client.get(&gist.id).await
                && let Ok(Some(content)) = client.file_content(&full_gist, &filename).await
                && sha256_hex(&content) == local_hash
            {
                content_match = Some(full_gist);
                break;
            }
        }

        if let Some(gist) = content_match {
            store.insert(
                root,
                file.rel_path.clone(),
                FileEntry {
                    gist_id: gist.id.clone(),
                    url: gist.html_url.clone(),
                    local_sha256: local_hash.clone(),
                    remote_sha256: local_hash,
                    last_synced: Utc::now(),
                    remote_updated_at: Some(gist.updated_at),
                },
            );
            matched += 1;
        } else {
            // Phase 5: Mark as ambiguous for manual resolution
            ambiguous.push(AmbiguousMatch {
                local_path: file.rel_path.clone(),
                local_hash: local_hash.clone(),
                candidates: size_matches
                    .iter()
                    .map(|g| GistCandidate {
                        gist_id: g.id.clone(),
                        url: g.html_url.clone(),
                        description: g.description.clone(),
                        size: g.files.get(&filename).map(|f| f.size).unwrap_or(0),
                    })
                    .collect(),
            });
        }
    }

    progress_cb(HydrationProgress {
        phase: format!(
            "Done. Matched {matched} files, {} ambiguous.",
            ambiguous.len()
        ),
        matched,
        total_gists,
        total_files,
        current_file: total_files,
        ambiguous: ambiguous.clone(),
    });

    Ok(HydrationOutcome { matched, ambiguous })
}

/// Build a store entry for a filename-matched gist with the *observed*
/// remote hash, fetched from the gist itself. Returns None if the fetch
/// fails (better to leave the file unmapped than to record a guess).
async fn verified_entry(
    client: &GistClient,
    gist_id: &str,
    filename: &str,
    local_content: &str,
) -> Option<FileEntry> {
    let full = client.get(gist_id).await.ok()?;
    let remote_content = client.file_content(&full, filename).await.ok()??;
    Some(FileEntry {
        gist_id: full.id.clone(),
        url: full.html_url.clone(),
        local_sha256: sha256_hex(local_content),
        remote_sha256: sha256_hex(&remote_content),
        last_synced: Utc::now(),
        remote_updated_at: Some(full.updated_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn listed_gist(id: &str, filename: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "html_url": format!("https://gist.github.com/u/{id}"),
            "description": null,
            "public": false,
            "files": {
                filename: { "filename": filename, "size": 10, "raw_url": null }
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
        })
    }

    fn full_gist(id: &str, filename: &str, content: &str) -> serde_json::Value {
        let mut v = listed_gist(id, filename);
        v["files"][filename]["content"] = content.into();
        v["files"][filename]["truncated"] = false.into();
        v
    }

    /// A unique filename match must record the gist's *actual* content hash,
    /// not assume remote == local — otherwise divergent remotes hydrate
    /// straight into a fabricated "Synced" state.
    #[tokio::test]
    async fn unique_match_records_real_remote_hash() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![listed_gist("g1", "a.md")]))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/gists/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(full_gist(
                "g1",
                "a.md",
                "remote content",
            )))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join("a.md");
        std::fs::write(&abs, "local content").unwrap();
        let files = vec![ScannedFile {
            rel_path: "a.md".into(),
            abs_path: abs,
            modified: std::time::SystemTime::UNIX_EPOCH,
        }];

        let client = GistClient::with_base_url("t".into(), server.uri());
        let mut store = Store::default();
        let outcome = hydrate(&client, &mut store, dir.path(), &files, |_| {})
            .await
            .unwrap();

        assert_eq!(outcome.matched, 1);
        let entry = store.get(dir.path(), "a.md").expect("mapped");
        assert_eq!(entry.local_sha256, sha256_hex("local content"));
        assert_eq!(entry.remote_sha256, sha256_hex("remote content"));
        assert_ne!(entry.local_sha256, entry.remote_sha256);
        assert!(entry.remote_updated_at.is_some());
    }
}
