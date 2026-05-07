pub mod dialogs;
pub mod diff;
pub mod input;
pub mod preview;
pub mod tree;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_tree_widget::Tree;

use crate::app::{App, Mode};

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

    // Tree pane
    let g = crate::glyphs::glyphs();
    let tree_block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Files", g.file_pane))
        .border_style(Style::default().fg(Color::DarkGray))
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

    // Right pane (preview or diff)
    match &app.mode {
        Mode::Diff { local, remote } => {
            let title = app.selected_file().unwrap_or_default();
            diff::render_diff(f, panes[1], local, remote, &title);
        }
        _ => {
            let title = app.selected_file().unwrap_or_default();
            preview::render_preview(f, panes[1], &app.preview_content, &title);
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
