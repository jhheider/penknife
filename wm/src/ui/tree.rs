use std::collections::BTreeMap;

use tui_tree_widget::TreeItem;

use crate::scanner::ScannedFile;
use crate::store::Store;
use crate::sync::{self, SyncStatus};

/// Build a hierarchical tree of items from scanned files.
/// Returns (tree_items, flat_identifiers) where identifiers map tree positions to rel_paths.
pub fn build_tree<'a>(
    files: &[ScannedFile],
    store: &Store,
    filter: &str,
) -> (Vec<TreeItem<'a, String>>, Vec<String>) {
    // Group files by directory hierarchy
    // e.g. "Game/rp-posts/file.md" → ["Game", "rp-posts", "file.md"]
    let mut tree: BTreeMap<String, BTreeMap<String, Vec<&ScannedFile>>> = BTreeMap::new();

    for file in files {
        if !filter.is_empty() && !file.rel_path.to_lowercase().contains(&filter.to_lowercase()) {
            continue;
        }

        let parts: Vec<&str> = file.rel_path.splitn(3, '/').collect();
        match parts.len() {
            1 => {
                tree.entry("".to_string())
                    .or_default()
                    .entry("".to_string())
                    .or_default()
                    .push(file);
            }
            2 => {
                tree.entry(parts[0].to_string())
                    .or_default()
                    .entry("".to_string())
                    .or_default()
                    .push(file);
            }
            3.. => {
                tree.entry(parts[0].to_string())
                    .or_default()
                    .entry(parts[1].to_string())
                    .or_default()
                    .push(file);
            }
            _ => {}
        }
    }

    let mut items = Vec::new();
    let mut identifiers = Vec::new();

    for (game, subdirs) in &tree {
        if game.is_empty() {
            // Root-level files
            for file in subdirs.values().flatten() {
                let status = file_status(file, store);
                let label = format_leaf(&file.rel_path, status);
                let id = file.rel_path.clone();
                identifiers.push(id.clone());
                items.push(TreeItem::new_leaf(id, label));
            }
            continue;
        }

        let mut game_children = Vec::new();
        let game_id = game.clone();

        for (subdir, sub_files) in subdirs {
            if subdir.is_empty() {
                // Files directly under game
                for file in sub_files {
                    let status = file_status(file, store);
                    let label = format_leaf(&file.rel_path, status);
                    let id = file.rel_path.clone();
                    identifiers.push(id.clone());
                    game_children.push(TreeItem::new_leaf(id, label));
                }
            } else {
                let sub_id = format!("{game}/{subdir}");
                let mut sub_children = Vec::new();
                for file in sub_files {
                    let status = file_status(file, store);
                    let label = format_leaf(&file.rel_path, status);
                    let id = file.rel_path.clone();
                    identifiers.push(id.clone());
                    sub_children.push(TreeItem::new_leaf(id, label));
                }
                identifiers.push(sub_id.clone());
                game_children.push(
                    TreeItem::new(sub_id, subdir.clone(), sub_children)
                        .expect("tree item"),
                );
            }
        }

        identifiers.push(game_id.clone());
        items.push(
            TreeItem::new(game_id, game.clone(), game_children)
                .expect("tree item"),
        );
    }

    (items, identifiers)
}

fn file_status(file: &ScannedFile, store: &Store) -> SyncStatus {
    let entry = store.get(&file.rel_path);
    if entry.is_none() {
        return SyncStatus::NotGisted;
    }
    let content = std::fs::read_to_string(&file.abs_path).unwrap_or_default();
    sync::local_status(&content, entry)
}

fn format_leaf(rel_path: &str, status: SyncStatus) -> String {
    let name = rel_path
        .rsplit('/')
        .next()
        .unwrap_or(rel_path)
        .trim_end_matches(".md");
    format!("{} {name}", status.icon())
}
