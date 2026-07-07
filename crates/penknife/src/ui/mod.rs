pub mod dialogs;
pub mod diff;
pub mod input;
pub mod keybar;
pub mod preview;
pub mod tree;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_tree_widget::Tree;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Mode, PaneFocus};

/// Build a colored pane title: a bold-colored mode label, optionally
/// followed by " · " and the (already-truncated) path in white.
fn right_pane_title(label: &str, label_color: Color, path: &str) -> Line<'static> {
    let mut spans = vec![Span::styled(
        label.to_string(),
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];
    if !path.is_empty() {
        spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            path.to_string(),
            Style::default().fg(Color::White),
        ));
    }
    Line::from(spans)
}

/// Truncate a path for use as a pane title. Keeps the rightmost (most
/// specific) part visible by ellipsizing the front when needed. `max` is a
/// display-column budget, not a char count, so wide (CJK) filenames don't
/// overflow the border.
fn truncate_path_for_title(path: &str, max: usize) -> String {
    if max <= 1 || path.is_empty() {
        return String::new();
    }
    if path.width() <= max {
        return path.to_string();
    }
    // Reserve one column for the leading ellipsis, then take chars from the
    // end while their summed display width still fits.
    let budget = max.saturating_sub(1);
    let mut used = 0;
    let mut tail_rev: Vec<char> = Vec::new();
    for ch in path.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        tail_rev.push(ch);
    }
    let tail: String = tail_rev.into_iter().rev().collect();
    format!("…{tail}")
}

/// Render the full UI.
pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1), // status bar
        Constraint::Length(1), // keybar
    ])
    .split(f.area());

    let main_area = chunks[0];
    let status_area = chunks[1];
    let keybar_area = chunks[2];

    // Main area: tree pane sized adaptively (clamped 28..=48 cols) so paths
    // are readable on narrow terminals without dominating wide ones.
    let tree_width = (main_area.width / 3).clamp(28, 48);
    let panes =
        Layout::horizontal([Constraint::Length(tree_width), Constraint::Min(0)]).split(main_area);

    // Record pane rects so handle_mouse can route wheel events.
    app.tree_pane_rect = panes[0];
    app.right_pane_rect = panes[1];

    // Tree pane
    let g = crate::glyphs::glyphs();
    let tree_focused = app.focused_pane == PaneFocus::Tree;
    let right_focused = app.focused_pane == PaneFocus::Right;
    let tree_border = if tree_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let cyan_bold = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let title_line = Line::from(vec![
        Span::styled(
            format!("{} ", g.file_pane),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled("Files", cyan_bold),
        Span::raw(" "),
        Span::styled("(", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.files.len().to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(")", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(" · {}", app.config.sort.mode.short()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let tree_block = Block::default()
        .borders(Borders::ALL)
        .title(title_line)
        .border_style(Style::default().fg(tree_border));
    let tree_widget = Tree::new(&app.tree_items)
        .expect("tree widget")
        .block(tree_block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));

    f.render_stateful_widget(tree_widget, panes[0], &mut app.tree_state);

    // Right pane (preview or diff). Titles are mode-prefixed and front-truncated
    // so long rel_paths don't blow out the border.
    let rel = app.selected_file().unwrap_or_default();
    let max_title_width = panes[1].width.saturating_sub(12) as usize;
    let trimmed = truncate_path_for_title(&rel, max_title_width);
    match &app.mode {
        Mode::Diff { local, remote } => {
            let title = right_pane_title("Diff", Color::Yellow, &trimmed);
            diff::render_diff(
                f,
                panes[1],
                local,
                remote,
                title,
                app.diff_scroll,
                right_focused,
            );
        }
        _ => {
            let title = right_pane_title("Preview", Color::Cyan, &trimmed);
            let ext = std::path::Path::new(&rel)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            preview::render_preview(
                f,
                panes[1],
                &app.preview_content,
                ext,
                title,
                app.preview_scroll,
                right_focused,
            );
        }
    }

    // Status bar. If status_spans is fresh (its concatenated text matches
    // status_message), render the multi-color version; otherwise the message
    // was set by a transient action and we fall back to a flat color.
    let spans_text: String = app
        .status_spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let base = Style::default().bg(Color::DarkGray);
    if !app.status_spans.is_empty() && spans_text == app.status_message {
        let line = Line::from(app.status_spans.clone());
        f.render_widget(Paragraph::new(line).style(base), status_area);
    } else {
        let status = Paragraph::new(app.status_message.clone()).style(base.fg(app.status_color));
        f.render_widget(status, status_area);
    }

    // Context-sensitive key hints under the status bar.
    keybar::render(f, keybar_area, app);

    // Modal overlays
    match &app.mode {
        Mode::Help => dialogs::render_help(f, f.area(), app),
        Mode::FilePicker { selected } => {
            dialogs::render_file_picker(f, f.area(), app, *selected);
        }
        Mode::Confirm { message, .. } => {
            dialogs::render_confirm(f, f.area(), message);
        }
        Mode::GdocUrl => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                "Import from URL",
                "Google Doc or gist URL (or bare gist ID):",
                &app.input_editor,
            );
        }
        Mode::GdocFilename => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                "Save As",
                "Enter filename (without .md):",
                &app.input_editor,
            );
        }
        Mode::Message(msg) => {
            dialogs::render_message(f, f.area(), msg);
        }
        Mode::RootSwitcher { .. } => {
            dialogs::render_root_switcher(f, f.area(), app);
        }
        Mode::SetupRoot | Mode::AddRoot => {
            dialogs::render_setup_root(f, f.area(), app);
        }
        Mode::ResolveAmbiguous { item, selected } => {
            dialogs::render_resolve_ambiguous(f, f.area(), app, *item, *selected);
        }
        Mode::SearchQuery => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                &format!("Find in {}", app.replace_scope_label()),
                "Search for (exact match):",
                &app.input_editor,
            );
        }
        Mode::SearchResults { selected } => {
            dialogs::render_search_results(f, f.area(), app, *selected);
        }
        Mode::ReplaceQuery => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                &format!("Replace in {}", app.replace_scope_label()),
                "Search for (exact match):",
                &app.input_editor,
            );
        }
        Mode::ReplaceTarget => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                &format!(
                    "Replace '{}' in {}",
                    app.replace_query,
                    app.replace_scope_label()
                ),
                "Replace with:",
                &app.input_editor,
            );
        }
        Mode::ReplaceReview { selected } => {
            dialogs::render_replace_review(f, f.area(), app, *selected);
        }
        Mode::Rename { old_rel } => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                "Rename / move",
                &format!("New path for {old_rel} (relative to root):"),
                &app.input_editor,
            );
        }
        Mode::LinkGist { rel_path } => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                "Link to gist",
                &format!("Gist URL or ID to link to {rel_path}:"),
                &app.input_editor,
            );
        }
        Mode::SortMenu { selected } => {
            dialogs::render_sort_menu(f, f.area(), app, *selected);
        }
        Mode::BulkMenu { selected } => {
            dialogs::render_bulk_menu(f, f.area(), app, *selected);
        }
        Mode::DeleteMenu { selected } => {
            dialogs::render_delete_menu(f, f.area(), app, *selected);
        }
        Mode::GitMenu { selected } => {
            dialogs::render_git_menu(f, f.area(), *selected);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_path_for_title;

    #[test]
    fn short_paths_pass_through_unchanged() {
        assert_eq!(truncate_path_for_title("a/b.md", 20), "a/b.md");
    }

    mod render {
        use super::super::draw;
        use crate::app::test_support::{new_for_test, select, write_file};
        use crate::app::{App, Mode};
        use crate::event::async_channel;
        use crate::hydrate::{AmbiguousMatch, GistCandidate};
        use crate::replace::ReplaceMatch;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use tempfile::TempDir;

        fn make_app() -> (TempDir, App) {
            let dir = tempfile::tempdir().unwrap();
            write_file(dir.path(), "a.md", "# alpha\n\nbody");
            write_file(dir.path(), "sub/c.md", "gamma");
            let (tx, rx) = async_channel();
            std::mem::forget(rx);
            let mut app = new_for_test(dir.path(), tx);
            select(&mut app, "a.md");
            (dir, app)
        }

        /// Render `app` at `w`x`h` and return the rendered text, asserting the
        /// draw did not panic.
        fn render_at(app: &mut App, w: u16, h: u16) -> String {
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            buf.content().iter().map(|c| c.symbol()).collect::<String>()
        }

        fn render(app: &mut App) -> String {
            render_at(app, 100, 30)
        }

        fn ambiguous(app: &mut App) {
            app.pending_ambiguous.push(AmbiguousMatch {
                local_path: "a.md".into(),
                local_hash: "h".into(),
                candidates: vec![GistCandidate {
                    remote_id: "g1".into(),
                    url: "https://gist.github.com/u/g1".into(),
                    description: Some("desc".into()),
                    size: 12,
                }],
            });
        }

        fn matches(app: &mut App) -> Vec<ReplaceMatch> {
            vec![
                ReplaceMatch {
                    abs_path: app.abs_path("a.md"),
                    rel_path: "a.md".into(),
                    line: 1,
                    col_byte: 2,
                    line_text: "# alpha".into(),
                },
                ReplaceMatch {
                    abs_path: app.abs_path("sub/c.md"),
                    rel_path: "sub/c.md".into(),
                    line: 1,
                    col_byte: 0,
                    line_text: "gamma".into(),
                },
            ]
        }

        #[test]
        fn draws_normal_with_preview() {
            let (_d, mut app) = make_app();
            let out = render(&mut app);
            assert!(out.contains("Files"));
            assert!(out.contains("Preview"));
        }

        #[test]
        fn draws_help() {
            let (_d, mut app) = make_app();
            app.mode = Mode::Help;
            let out = render(&mut app);
            assert!(out.contains("Help") || out.contains("Keybindings") || out.contains("keys"));
        }

        #[test]
        fn draws_file_picker() {
            let (_d, mut app) = make_app();
            app.handle_key(crate::app::test_support::ch('/'));
            let out = render(&mut app);
            assert!(!out.trim().is_empty());
        }

        #[test]
        fn draws_confirm() {
            let (_d, mut app) = make_app();
            app.mode = Mode::Confirm {
                message: "Really do the thing?".into(),
                action: crate::app::ConfirmAction::SyncDown,
            };
            let out = render(&mut app);
            assert!(out.contains("Really do the thing"));
        }

        #[test]
        fn draws_message() {
            let (_d, mut app) = make_app();
            app.mode = Mode::Message("something happened".into());
            let out = render(&mut app);
            assert!(out.contains("something happened"));
        }

        #[test]
        fn draws_input_dialogs() {
            let (_d, mut app) = make_app();
            for mode in [
                Mode::GdocUrl,
                Mode::GdocFilename,
                Mode::SearchQuery,
                Mode::ReplaceQuery,
                Mode::ReplaceTarget,
                Mode::Rename {
                    old_rel: "a.md".into(),
                },
                Mode::LinkGist {
                    rel_path: "a.md".into(),
                },
            ] {
                app.mode = mode;
                let out = render(&mut app);
                assert!(!out.trim().is_empty());
            }
        }

        #[test]
        fn draws_root_switcher_and_setup() {
            let (_d, mut app) = make_app();
            app.mode = Mode::RootSwitcher { selected: 0 };
            assert!(!render(&mut app).trim().is_empty());
            app.mode = Mode::AddRoot;
            assert!(!render(&mut app).trim().is_empty());
            app.mode = Mode::SetupRoot;
            assert!(!render(&mut app).trim().is_empty());
        }

        #[test]
        fn draws_resolve_ambiguous() {
            let (_d, mut app) = make_app();
            ambiguous(&mut app);
            app.mode = Mode::ResolveAmbiguous {
                item: 0,
                selected: 0,
            };
            let out = render(&mut app);
            assert!(!out.trim().is_empty());
        }

        #[test]
        fn draws_search_results() {
            let (_d, mut app) = make_app();
            app.search_matches = matches(&mut app);
            app.mode = Mode::SearchResults { selected: 0 };
            let out = render(&mut app);
            assert!(!out.trim().is_empty());
        }

        #[test]
        fn draws_replace_review() {
            let (_d, mut app) = make_app();
            app.replace_matches = matches(&mut app);
            app.replace_checked = vec![true, false];
            app.replace_query = "a".into();
            app.replace_target = "b".into();
            app.mode = Mode::ReplaceReview { selected: 0 };
            let out = render(&mut app);
            assert!(!out.trim().is_empty());
        }

        #[test]
        fn draws_menus() {
            let (_d, mut app) = make_app();
            for mode in [
                Mode::SortMenu { selected: 0 },
                Mode::BulkMenu { selected: 0 },
                Mode::DeleteMenu { selected: 0 },
                Mode::GitMenu { selected: 0 },
            ] {
                app.mode = mode;
                assert!(!render(&mut app).trim().is_empty());
            }
        }

        #[test]
        fn draws_diff() {
            let (_d, mut app) = make_app();
            app.mode = Mode::Diff {
                local: "line one\nline two".into(),
                remote: "line one\nline three".into(),
            };
            let out = render(&mut app);
            assert!(out.contains("Diff"));
        }

        #[test]
        fn draws_in_tiny_area_without_panic() {
            let (_d, mut app) = make_app();
            // Absurdly small terminals must not panic the renderer.
            for (w, h) in [(4u16, 3u16), (1, 1), (10, 4)] {
                let _ = render_at(&mut app, w, h);
                app.mode = Mode::Help;
                let _ = render_at(&mut app, w, h);
                app.mode = Mode::Confirm {
                    message: "x".into(),
                    action: crate::app::ConfirmAction::SyncDown,
                };
                let _ = render_at(&mut app, w, h);
                app.mode = Mode::Normal;
            }
        }
    }

    #[test]
    fn long_paths_get_front_ellipsized() {
        let out = truncate_path_for_title("very/long/path/to/file.md", 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("file.md"));
    }

    #[test]
    fn empty_or_too_small_max_returns_empty() {
        assert_eq!(truncate_path_for_title("", 10), "");
        assert_eq!(truncate_path_for_title("anything", 0), "");
        assert_eq!(truncate_path_for_title("anything", 1), "");
    }

    #[test]
    fn wide_chars_count_by_display_width() {
        use unicode_width::UnicodeWidthStr;
        // Each CJK char is 2 columns; the result must fit the column budget,
        // not just the char count.
        let out = truncate_path_for_title("notes/日本語のメモ.md", 10);
        assert!(out.width() <= 10, "{out:?} is {} cols", out.width());
        assert!(out.starts_with('…'));
        assert!(out.ends_with(".md"));
    }
}
