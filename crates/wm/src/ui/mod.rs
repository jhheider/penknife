pub mod dialogs;
pub mod diff;
pub mod input;
pub mod preview;
pub mod tree;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_tree_widget::Tree;

use crate::app::{App, Mode, PaneFocus};

/// Truncate a path for use as a pane title. Keeps the rightmost (most
/// specific) part visible by ellipsizing the front when needed.
fn truncate_path_for_title(path: &str, max: usize) -> String {
    if max <= 1 || path.is_empty() {
        return String::new();
    }
    if path.chars().count() <= max {
        return path.to_string();
    }
    // Reserve room for the leading ellipsis.
    let want = max.saturating_sub(1);
    let tail: String = path.chars().rev().take(want).collect::<String>();
    let tail: String = tail.chars().rev().collect();
    format!("…{tail}")
}

/// Render the full UI.
pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(f.area());

    let main_area = chunks[0];
    let status_area = chunks[1];

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
    let tree_block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Files", g.file_pane))
        .border_style(Style::default().fg(tree_border))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
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
            let title = if trimmed.is_empty() {
                "Diff".to_string()
            } else {
                format!("Diff · {trimmed}")
            };
            diff::render_diff(
                f,
                panes[1],
                local,
                remote,
                &title,
                app.diff_scroll,
                right_focused,
            );
        }
        _ => {
            let title = if trimmed.is_empty() {
                "Preview".to_string()
            } else {
                format!("Preview · {trimmed}")
            };
            preview::render_preview(
                f,
                panes[1],
                &app.preview_content,
                &title,
                app.preview_scroll,
                right_focused,
            );
        }
    }

    // Status bar
    let status_text = &app.status_message;
    let status = Paragraph::new(status_text.clone())
        .style(Style::default().bg(Color::DarkGray).fg(app.status_color));
    f.render_widget(status, status_area);

    // Modal overlays
    match &app.mode {
        Mode::Help => dialogs::render_help(f, f.area()),
        Mode::Search => dialogs::render_search(f, f.area(), &app.search_editor),
        Mode::Confirm { message, .. } => {
            dialogs::render_confirm(f, f.area(), message);
        }
        Mode::GdocUrl => {
            dialogs::render_input_dialog(
                f,
                f.area(),
                "Import Google Doc",
                "Enter Google Doc URL:",
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
        Mode::Hydrating {
            progress: Some(p), ..
        } => {
            dialogs::render_hydration_progress(f, f.area(), p);
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
}
