use std::collections::HashMap;

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
    pub ambiguous: Vec<AmbiguousMatch>,
}

#[derive(Debug, Clone)]
pub struct AmbiguousMatch {
    pub local_path: String,
    pub candidates: Vec<GistCandidate>,
}

#[derive(Debug, Clone)]
pub struct GistCandidate {
    pub gist_id: String,
    pub url: String,
    pub description: Option<String>,
    pub size: u64,
}

/// Run the full hydration algorithm. Calls `progress_cb` with updates.
pub async fn hydrate(
    client: &GistClient,
    store: &mut Store,
    files: &[ScannedFile],
    mut progress_cb: impl FnMut(HydrationProgress),
) -> Result<usize> {
    // Phase 1: Fetch all gists
    progress_cb(HydrationProgress {
        phase: "Fetching all gists...".into(),
        matched: 0,
        total_gists: 0,
        ambiguous: vec![],
    });

    let all_gists = client.list_all().await?;
    let total_gists = all_gists.len();

    progress_cb(HydrationProgress {
        phase: format!("Fetched {total_gists} gists. Building index..."),
        matched: 0,
        total_gists,
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
        local_by_filename
            .entry(filename)
            .or_default()
            .push(file);
    }

    let mut matched = 0usize;
    let mut ambiguous = Vec::new();

    // Phase 3: Unique matches
    for file in files {
        // Skip already-mapped files
        if store.get(&file.rel_path).is_some() {
            matched += 1;
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

        let locals = local_by_filename.get(&filename).map(|v| v.len()).unwrap_or(0);

        if gists.len() == 1 && locals == 1 {
            // Unique match
            let gist = gists[0];
            let hash = sha256_hex(&std::fs::read_to_string(&file.abs_path).unwrap_or_default());
            store.insert(
                file.rel_path.clone(),
                FileEntry {
                    gist_id: gist.id.clone(),
                    url: gist.html_url.clone(),
                    local_sha256: hash.clone(),
                    remote_sha256: hash,
                    last_synced: Utc::now(),
                },
            );
            matched += 1;
        }
    }

    progress_cb(HydrationProgress {
        phase: format!("Unique matches: {matched}. Disambiguating..."),
        matched,
        total_gists,
        ambiguous: vec![],
    });

    // Phase 4: Disambiguate by content (SHA-256 compare)
    for file in files {
        if store.get(&file.rel_path).is_some() {
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

        let local_content = std::fs::read_to_string(&file.abs_path).unwrap_or_default();
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
            let gist = size_matches[0];
            store.insert(
                file.rel_path.clone(),
                FileEntry {
                    gist_id: gist.id.clone(),
                    url: gist.html_url.clone(),
                    local_sha256: local_hash.clone(),
                    remote_sha256: local_hash,
                    last_synced: Utc::now(),
                },
            );
            matched += 1;
            continue;
        }

        // If size filter didn't help enough, try fetching content
        if size_matches.is_empty() {
            size_matches = gists.iter().copied().collect();
        }

        let mut content_match = None;
        for gist in &size_matches {
            if let Ok(full_gist) = client.get(&gist.id).await {
                if let Some(gf) = full_gist.files.get(&filename) {
                    if let Some(ref content) = gf.content {
                        if sha256_hex(content) == local_hash {
                            content_match = Some(full_gist);
                            break;
                        }
                    }
                }
            }
        }

        if let Some(gist) = content_match {
            store.insert(
                file.rel_path.clone(),
                FileEntry {
                    gist_id: gist.id.clone(),
                    url: gist.html_url.clone(),
                    local_sha256: local_hash.clone(),
                    remote_sha256: local_hash,
                    last_synced: Utc::now(),
                },
            );
            matched += 1;
        } else {
            // Phase 5: Mark as ambiguous for manual resolution
            ambiguous.push(AmbiguousMatch {
                local_path: file.rel_path.clone(),
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
        phase: format!("Done. Matched {matched} files, {} ambiguous.", ambiguous.len()),
        matched,
        total_gists,
        ambiguous,
    });

    store.save()?;
    Ok(matched)
}
