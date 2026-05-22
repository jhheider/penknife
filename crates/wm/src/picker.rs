use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::scanner::ScannedFile;

/// One result from the picker — full rel_path, fuzzy score, and the
/// character indices (into rel_path) that matched the query.
#[derive(Debug, Clone)]
pub struct PickerMatch {
    pub rel_path: String,
    pub score: u32,
    pub indices: Vec<u32>,
}

/// Stateful wrapper so we can reuse the Matcher's internal buffers across
/// keystrokes (avoids per-keystroke allocation churn).
pub struct Picker {
    matcher: Matcher,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }
}

impl Picker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rank `files` against `query`. With an empty query, returns all files
    /// in their original order (so the picker shows something useful on open).
    /// Otherwise returns only matching files, sorted by descending score.
    /// Caller may limit how many results to render.
    pub fn rank(&mut self, files: &[ScannedFile], query: &str) -> Vec<PickerMatch> {
        if query.is_empty() {
            return files
                .iter()
                .map(|f| PickerMatch {
                    rel_path: f.rel_path.clone(),
                    score: 0,
                    indices: Vec::new(),
                })
                .collect();
        }

        // `Pattern::parse` interprets the query as one or more whitespace-
        // separated atoms with smartcase + path-bonus matching, which gives
        // us fzf-like ergonomics (e.g. `"foo bar"` matches both substrings).
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

        let mut results: Vec<PickerMatch> = Vec::new();
        let mut hay_buf = Vec::new();
        let mut indices = Vec::new();

        for file in files {
            hay_buf.clear();
            indices.clear();
            let hay = Utf32Str::new(&file.rel_path, &mut hay_buf);
            if let Some(score) = pattern.indices(hay, &mut self.matcher, &mut indices) {
                indices.sort_unstable();
                indices.dedup();
                results.push(PickerMatch {
                    rel_path: file.rel_path.clone(),
                    score,
                    indices: indices.clone(),
                });
            }
        }

        // Sort by descending score, then alphabetical for stable tie-breaks.
        results.sort_by(|a, b| b.score.cmp(&a.score).then(a.rel_path.cmp(&b.rel_path)));
        results
    }
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
    fn empty_query_returns_all_files_in_order() {
        let files = vec![sf("z.md"), sf("a.md"), sf("m.md")];
        let mut p = Picker::new();
        let out = p.rank(&files, "");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].rel_path, "z.md");
        assert_eq!(out[2].rel_path, "m.md");
    }

    #[test]
    fn fuzzy_query_matches_subsequence() {
        let files = vec![
            sf("drafts/post.md"),
            sf("publish/post.md"),
            sf("unrelated/file.md"),
        ];
        let mut p = Picker::new();
        let out = p.rank(&files, "pubpost");
        assert!(out.iter().any(|m| m.rel_path == "publish/post.md"));
        assert!(!out.iter().any(|m| m.rel_path == "unrelated/file.md"));
    }

    #[test]
    fn returns_match_indices_for_highlight() {
        let files = vec![sf("hello.md")];
        let mut p = Picker::new();
        let out = p.rank(&files, "hlo");
        assert_eq!(out.len(), 1);
        assert!(!out[0].indices.is_empty());
        // All indices must be within the rel_path's char length.
        let len = files[0].rel_path.chars().count() as u32;
        assert!(out[0].indices.iter().all(|&i| i < len));
    }

    #[test]
    fn results_exclude_non_matches_and_sort_consistently() {
        let files = vec![sf("aaa/zzz/file.md"), sf("file.md"), sf("other.md")];
        let mut p = Picker::new();
        let out = p.rank(&files, "file");
        let names: Vec<&str> = out.iter().map(|m| m.rel_path.as_str()).collect();
        assert!(names.contains(&"aaa/zzz/file.md"));
        assert!(names.contains(&"file.md"));
        assert!(!names.contains(&"other.md"));
        // Scores are monotonically non-increasing.
        for w in out.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
    }
}
