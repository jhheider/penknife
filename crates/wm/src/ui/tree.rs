use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use ratatui::prelude::*;
use tui_tree_widget::TreeItem;

use crate::git::GitStatus;
use crate::scanner::ScannedFile;
use crate::store::Store;
use crate::sync::{self, SyncStatus};

/// Result of building the tree: the widget items, the flat list of all
/// identifiers (files + directories), and the subset that are file leaves.
pub struct BuiltTree<'a> {
    pub items: Vec<TreeItem<'a, String>>,
    pub identifiers: Vec<String>,
    pub file_ids: HashSet<String>,
}

#[derive(Default)]
struct Node<'a> {
    /// Subdirectories keyed by component name, sorted alphabetically (BTreeMap).
    children: BTreeMap<String, Node<'a>>,
    /// Files directly under this directory, kept in insertion order so the
    /// caller's sort (mtime desc, from scanner) survives the rebuild.
    files: Vec<&'a ScannedFile>,
}

/// Build a hierarchical tree of items from scanned files. Supports arbitrary
/// directory depth — components are walked recursively rather than truncated.
/// `git_statuses` is optional per-file git state; empty map = no git column.
pub fn build_tree<'a>(
    files: &[ScannedFile],
    store: &Store,
    root: Option<&Path>,
    git_statuses: &HashMap<String, GitStatus>,
) -> BuiltTree<'a> {
    let mut root_node: Node = Node::default();

    for file in files {
        let mut parts = file.rel_path.split('/').collect::<Vec<_>>();
        // The final component is the file itself; the rest are directories.
        let _name = parts.pop();
        let mut cur = &mut root_node;
        for dir in parts {
            cur = cur.children.entry(dir.to_string()).or_default();
        }
        cur.files.push(file);
    }

    let mut identifiers = Vec::new();
    let mut file_ids: HashSet<String> = HashSet::new();
    let items = render_node(
        &root_node,
        "",
        store,
        root,
        git_statuses,
        &mut identifiers,
        &mut file_ids,
    );

    BuiltTree {
        items,
        identifiers,
        file_ids,
    }
}

fn render_node<'a>(
    node: &Node,
    prefix: &str,
    store: &Store,
    root: Option<&Path>,
    git_statuses: &HashMap<String, GitStatus>,
    identifiers: &mut Vec<String>,
    file_ids: &mut HashSet<String>,
) -> Vec<TreeItem<'a, String>> {
    let mut items = Vec::new();

    // Directories first (sorted alphabetically thanks to BTreeMap).
    for (name, child) in &node.children {
        let dir_id = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let children = render_node(
            child,
            &dir_id,
            store,
            root,
            git_statuses,
            identifiers,
            file_ids,
        );
        identifiers.push(dir_id.clone());
        items.push(
            TreeItem::new(dir_id, format_directory(name), children).expect("unique tree item id"),
        );
    }

    // Then files (preserve scanner-supplied order = mtime desc).
    for file in &node.files {
        let status = file_status(file, store, root);
        let git = git_statuses.get(&file.rel_path).copied();
        let label = format_leaf(&file.rel_path, status, git);
        let id = file.rel_path.clone();
        identifiers.push(id.clone());
        file_ids.insert(id.clone());
        items.push(TreeItem::new_leaf(id, label));
    }

    items
}

fn file_status(file: &ScannedFile, store: &Store, root: Option<&Path>) -> SyncStatus {
    let entry = root.and_then(|r| store.get(r, &file.rel_path));
    if entry.is_none() {
        return SyncStatus::NotGisted;
    }
    let content = std::fs::read_to_string(&file.abs_path).unwrap_or_default();
    sync::local_status(&content, entry)
}

fn format_leaf(rel_path: &str, status: SyncStatus, git: Option<GitStatus>) -> Line<'static> {
    let name = rel_path
        .rsplit('/')
        .next()
        .unwrap_or(rel_path)
        .trim_end_matches(".md");
    let g = crate::glyphs::glyphs();
    let icon = format!("{} ", status.icon());

    let (git_glyph, git_color) = match git {
        Some(s) if s.staged && s.modified => {
            // Staged AND further unstaged changes — show modified (the
            // more-actionable signal), but in a brighter color.
            (g.git_modified, Color::Yellow)
        }
        Some(s) if s.staged => (g.git_staged, Color::Green),
        Some(s) if s.modified => (g.git_modified, Color::Yellow),
        Some(s) if s.untracked => (g.git_untracked, Color::DarkGray),
        _ => (g.git_clean, Color::DarkGray),
    };

    Line::from(vec![
        Span::raw(icon),
        Span::styled(format!("{git_glyph} "), Style::default().fg(git_color)),
        Span::styled(name.to_string(), Style::default().fg(status.color())),
    ])
}

fn format_directory(name: &str) -> Line<'static> {
    let g = crate::glyphs::glyphs();
    Line::from(vec![
        Span::raw(format!("{} ", g.dir)),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn sf(rel: &str) -> ScannedFile {
        ScannedFile {
            rel_path: rel.to_string(),
            abs_path: PathBuf::from(rel),
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn deep_paths_nest_recursively() {
        let files = vec![sf("a/b/c/d.md"), sf("a/b/c/e.md"), sf("a/f.md")];
        let store = Store::default();
        let built = build_tree(&files, &store, None, &HashMap::new());
        // Top-level should have exactly one directory entry ("a"); identifiers
        // should include each directory level ("a", "a/b", "a/b/c").
        assert_eq!(built.items.len(), 1);
        assert!(built.identifiers.contains(&"a".to_string()));
        assert!(built.identifiers.contains(&"a/b".to_string()));
        assert!(built.identifiers.contains(&"a/b/c".to_string()));
        assert!(built.file_ids.contains("a/b/c/d.md"));
        assert!(built.file_ids.contains("a/f.md"));
    }

    #[test]
    fn root_level_files_appear_at_top() {
        let files = vec![sf("README.md"), sf("notes/today.md")];
        let store = Store::default();
        let built = build_tree(&files, &store, None, &HashMap::new());
        assert!(built.file_ids.contains("README.md"));
        assert!(built.file_ids.contains("notes/today.md"));
    }
}
