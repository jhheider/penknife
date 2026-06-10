use std::collections::{BTreeMap, HashMap, HashSet};

use ratatui::prelude::*;
use tui_tree_widget::TreeItem;

use crate::git::GitStatus;
use crate::scanner::ScannedFile;
use crate::sync::SyncStatus;

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
/// `status_cache` carries the precomputed per-file sync status (so the tree
/// build doesn't re-read every file). `git_statuses` is optional per-file
/// git state; empty map = no git column.
pub fn build_tree<'a>(
    files: &[ScannedFile],
    status_cache: &HashMap<String, SyncStatus>,
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

    // Pre-pass: recursively tally each directory's subtree by sync status, so
    // directory rows can trail a rolled-up summary like `[9 3 1 12]`. Computed
    // once here off the cached statuses — no IO.
    let mut dir_counts: HashMap<String, [usize; 5]> = HashMap::new();
    tally_dirs(&root_node, "", status_cache, &mut dir_counts);

    let mut identifiers = Vec::new();
    let mut file_ids: HashSet<String> = HashSet::new();
    let items = render_node(
        &root_node,
        "",
        status_cache,
        git_statuses,
        &dir_counts,
        &mut identifiers,
        &mut file_ids,
    );

    BuiltTree {
        items,
        identifiers,
        file_ids,
    }
}

/// Canonical order of statuses in the directory summary gutter, and the index
/// each maps to in the `[usize; 5]` count arrays.
const STATUS_ORDER: [SyncStatus; 5] = [
    SyncStatus::Synced,
    SyncStatus::LocalNewer,
    SyncStatus::RemoteNewer,
    SyncStatus::Conflict,
    SyncStatus::NotGisted,
];

fn status_index(s: SyncStatus) -> usize {
    match s {
        SyncStatus::Synced => 0,
        SyncStatus::LocalNewer => 1,
        SyncStatus::RemoteNewer => 2,
        SyncStatus::Conflict => 3,
        SyncStatus::NotGisted => 4,
    }
}

/// Recursively accumulate per-directory subtree counts (by sync status) into
/// `out`, keyed by directory id. Returns this node's own subtree totals so the
/// parent can fold them in.
fn tally_dirs(
    node: &Node,
    prefix: &str,
    cache: &HashMap<String, SyncStatus>,
    out: &mut HashMap<String, [usize; 5]>,
) -> [usize; 5] {
    let mut total = [0usize; 5];
    for (name, child) in &node.children {
        let dir_id = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let sub = tally_dirs(child, &dir_id, cache, out);
        for i in 0..5 {
            total[i] += sub[i];
        }
        out.insert(dir_id, sub);
    }
    for f in &node.files {
        let s = cache
            .get(&f.rel_path)
            .copied()
            .unwrap_or(SyncStatus::NotGisted);
        total[status_index(s)] += 1;
    }
    total
}

fn render_node<'a>(
    node: &Node,
    prefix: &str,
    status_cache: &HashMap<String, SyncStatus>,
    git_statuses: &HashMap<String, GitStatus>,
    dir_counts: &HashMap<String, [usize; 5]>,
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
            status_cache,
            git_statuses,
            dir_counts,
            identifiers,
            file_ids,
        );
        let counts = dir_counts.get(&dir_id).copied().unwrap_or([0; 5]);
        let label = format_directory(name, &counts);
        identifiers.push(dir_id.clone());
        items.push(TreeItem::new(dir_id, label, children).expect("unique tree item id"));
    }

    // Then files (preserve scanner-supplied order = mtime desc).
    for file in &node.files {
        let status = status_cache
            .get(&file.rel_path)
            .copied()
            .unwrap_or(SyncStatus::NotGisted);
        let git = git_statuses.get(&file.rel_path).copied();
        let label = format_leaf(&file.rel_path, status, git);
        let id = file.rel_path.clone();
        identifiers.push(id.clone());
        file_ids.insert(id.clone());
        items.push(TreeItem::new_leaf(id, label));
    }

    items
}

fn format_leaf(rel_path: &str, status: SyncStatus, git: Option<GitStatus>) -> Line<'static> {
    let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    // Strip the last extension uniformly (.md, .json, …). The tree icon
    // already signals "this is a writing"; the extension is noise.
    let name = match basename.rfind('.') {
        Some(idx) if idx > 0 => &basename[..idx],
        _ => basename,
    };
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

fn format_directory(name: &str, counts: &[usize; 5]) -> Line<'static> {
    let g = crate::glyphs::glyphs();
    let mut spans = vec![
        Span::raw(format!("{} ", g.dir)),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // Trailing summary: the subtree's non-zero status counts as color-only
    // numbers in a bracketed block, e.g. `[9 3 1 12]`. The color *is* the
    // legend — green synced, yellow local-newer, blue remote-newer, red
    // conflict, grey not-gisted (matching the status bar and file icons).
    // Brackets are dimmed so they frame without competing. A fully-empty
    // subtree (no files anywhere) shows nothing.
    if counts.iter().any(|&n| n > 0) {
        let dim = Style::default().fg(Color::DarkGray);
        spans.push(Span::styled("  [", dim));
        let mut first = true;
        for st in STATUS_ORDER {
            let n = counts[status_index(st)];
            if n == 0 {
                continue;
            }
            if !first {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                n.to_string(),
                Style::default().fg(st.color()).add_modifier(Modifier::BOLD),
            ));
            first = false;
        }
        spans.push(Span::styled("]", dim));
    }

    Line::from(spans)
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
        let built = build_tree(&files, &HashMap::new(), &HashMap::new());
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
    fn dir_counts_roll_up_recursively_and_omit_zeros() {
        // a/b/c/d.md (synced), a/b/c/e.md (local-newer), a/f.md (not-gisted).
        let files = vec![sf("a/b/c/d.md"), sf("a/b/c/e.md"), sf("a/f.md")];
        let mut cache = HashMap::new();
        cache.insert("a/b/c/d.md".to_string(), SyncStatus::Synced);
        cache.insert("a/b/c/e.md".to_string(), SyncStatus::LocalNewer);
        cache.insert("a/f.md".to_string(), SyncStatus::NotGisted);

        let mut root = Node::default();
        for f in &files {
            let mut parts: Vec<&str> = f.rel_path.split('/').collect();
            parts.pop();
            let mut cur = &mut root;
            for d in parts {
                cur = cur.children.entry(d.to_string()).or_default();
            }
            cur.files.push(f);
        }
        let mut out = HashMap::new();
        let total = tally_dirs(&root, "", &cache, &mut out);
        // Whole tree: 1 synced, 1 local-newer, 1 not-gisted.
        assert_eq!(total, [1, 1, 0, 0, 1]);
        // "a" subtree == everything; "a/b/c" holds only the two gisted files.
        assert_eq!(out["a"], [1, 1, 0, 0, 1]);
        assert_eq!(out["a/b/c"], [1, 1, 0, 0, 0]);
    }

    #[test]
    fn root_level_files_appear_at_top() {
        let files = vec![sf("README.md"), sf("notes/today.md")];
        let built = build_tree(&files, &HashMap::new(), &HashMap::new());
        assert!(built.file_ids.contains("README.md"));
        assert!(built.file_ids.contains("notes/today.md"));
    }
}
