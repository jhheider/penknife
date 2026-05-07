use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap};

use crate::app::App;
use crate::hydrate::HydrationProgress;
use crate::ui::input::LineEditor;

/// Render a centered modal overlay.
fn modal_area(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - height_pct) / 2),
        Constraint::Percentage(height_pct),
        Constraint::Percentage((100 - height_pct) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - width_pct) / 2),
        Constraint::Percentage(width_pct),
        Constraint::Percentage((100 - width_pct) / 2),
    ])
    .split(vertical[1])[1]
}

/// Render the help overlay.
pub fn render_help(f: &mut Frame, area: Rect) {
    let modal = modal_area(area, 60, 70);
    f.render_widget(Clear, modal);

    let help_text = "\
Keybindings:

  j/k/↑/↓     Navigate tree
  Enter/l/→    Expand / select
  h/←/Bksp    Collapse
  u            Sync up (push to gist)
  d            Sync down (pull from gist)
  c            Copy URL to clipboard
  D            Diff view (local vs remote)
  H            Hydrate (match gists to files)
  /            Search / filter
  I            Google Doc import
  Tab          Switch root directory
  r            Refresh file tree
  ?            This help
  q            Quit

Press any key to close.";

    let block = Block::default()
        .borders(Borders::ALL)
        .title("❓ Help")
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render the search input dialog.
pub fn render_search(f: &mut Frame, area: Rect, editor: &LineEditor) {
    let modal = modal_area(area, 50, 10);
    f.render_widget(Clear, modal);

    let text = format!("/{}", editor.content);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("🔍 Search")
        .border_style(Style::default().fg(Color::Yellow))
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(text).block(block);
    f.render_widget(para, modal);
}

/// Render a text input dialog with a prompt.
pub fn render_input_dialog(
    f: &mut Frame,
    area: Rect,
    title: &str,
    prompt: &str,
    editor: &LineEditor,
) {
    let modal = modal_area(area, 60, 15);
    f.render_widget(Clear, modal);

    let text = format!("{prompt}\n\n> {}", editor.content);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render hydration progress with a gauge bar.
pub fn render_hydration_progress(f: &mut Frame, area: Rect, progress: &HydrationProgress) {
    let modal = modal_area(area, 60, 25);
    f.render_widget(Clear, modal);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("🔄 Hydration")
        .border_style(Style::default().fg(Color::Yellow))
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    // Split modal into text area and gauge
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    let chunks = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1), // spacer
        Constraint::Length(1), // gauge
    ])
    .split(inner);

    let text = format!(
        "{}\n\nMatched: {}  |  Total gists: {}  |  Ambiguous: {}",
        progress.phase,
        progress.matched,
        progress.total_gists,
        progress.ambiguous.len()
    );
    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    f.render_widget(para, chunks[0]);

    // Gauge: use current_file / total_files if available
    let ratio = if progress.total_files > 0 {
        (progress.current_file as f64 / progress.total_files as f64).min(1.0)
    } else if progress.total_gists > 0 {
        (progress.matched as f64 / progress.total_gists as f64).min(1.0)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Yellow).bg(Color::DarkGray))
        .ratio(ratio);
    f.render_widget(gauge, chunks[2]);
}

/// Render a confirmation dialog.
pub fn render_confirm(f: &mut Frame, area: Rect, message: &str) {
    let modal = modal_area(area, 50, 15);
    f.render_widget(Clear, modal);

    let text = format!("{message}\n\n[y] Yes  [n] No");
    let block = Block::default()
        .borders(Borders::ALL)
        .title("⚠️  Confirm")
        .border_style(Style::default().fg(Color::Red))
        .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render a status message overlay.
pub fn render_message(f: &mut Frame, area: Rect, message: &str) {
    let modal = modal_area(area, 50, 10);
    f.render_widget(Clear, modal);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("💬 Info")
        .border_style(Style::default().fg(Color::Green))
        .title_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(message.to_string())
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render root switcher dialog.
pub fn render_root_switcher(f: &mut Frame, area: Rect, app: &App) {
    let modal = modal_area(area, 60, 50);
    f.render_widget(Clear, modal);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("📂 Root Directories")
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let selected = if let crate::app::Mode::RootSwitcher { selected } = &app.mode {
        *selected
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    for (i, root) in app.config.roots.iter().enumerate() {
        let marker = if i == app.active_root { " ▶ " } else { "   " };
        let style = if i == selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if i == app.active_root {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::styled(format!("{marker}{}", root.display()), style));
    }

    if app.config.roots.is_empty() {
        lines.push(Line::styled(
            "  (no roots configured)".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Enter=switch  a=add  d=delete  Esc=close".to_string(),
        Style::default().fg(Color::DarkGray),
    ));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render the ambiguous-match resolver dialog. Shows the current item with
/// the candidate gists and footer keybindings.
pub fn render_resolve_ambiguous(
    f: &mut Frame,
    area: Rect,
    app: &App,
    item: usize,
    selected: usize,
) {
    let modal = modal_area(area, 70, 60);
    f.render_widget(Clear, modal);

    let total = app.pending_ambiguous.len();
    let title = format!("❓ Resolve ambiguous match ({} of {})", item + 1, total);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Yellow))
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut lines: Vec<Line> = Vec::new();
    if let Some(am) = app.pending_ambiguous.get(item) {
        lines.push(Line::styled(
            format!("Local file: {}", am.local_path),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Candidate gists:",
            Style::default().fg(Color::White),
        ));
        for (i, c) in am.candidates.iter().enumerate() {
            let marker = if i == selected { " ▶ " } else { "   " };
            let style = if i == selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            let desc = c.description.as_deref().unwrap_or("(no description)");
            lines.push(Line::styled(
                format!("{marker}{:.10}  {} bytes  {}", c.gist_id, c.size, desc),
                style,
            ));
            lines.push(Line::styled(
                format!("    {}", c.url),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "j/k=navigate  Enter=pick  s=skip  Esc=abort",
        Style::default().fg(Color::DarkGray),
    ));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render setup/add root dialog.
pub fn render_setup_root(f: &mut Frame, area: Rect, app: &App) {
    let modal = modal_area(area, 60, 20);
    f.render_widget(Clear, modal);

    let is_setup = matches!(app.mode, crate::app::Mode::SetupRoot);
    let title = if is_setup {
        "👋 Welcome to Writings Manager"
    } else {
        "📂 Add Root Directory"
    };
    let prompt = if is_setup {
        "Enter path to your writings folder:"
    } else {
        "Enter path to add:"
    };
    let hint = if is_setup {
        "\n\n(Enter to confirm · Ctrl+Q to quit)"
    } else {
        "\n\n(Enter to confirm · Esc to cancel)"
    };

    let text = format!("{prompt}\n\n> {}{hint}", app.input_editor.content);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}
