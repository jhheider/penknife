use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

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
  r            Refresh file tree
  ?            This help
  q            Quit

Press any key to close.";

    let block = Block::default().borders(Borders::ALL).title("Help");
    let para = Paragraph::new(help_text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render the search input dialog.
pub fn render_search(f: &mut Frame, area: Rect, editor: &LineEditor) {
    let modal = modal_area(area, 50, 10);
    f.render_widget(Clear, modal);

    let text = format!("/{}", editor.content);
    let block = Block::default().borders(Borders::ALL).title("Search");
    let para = Paragraph::new(text).block(block);
    f.render_widget(para, modal);
}

/// Render a text input dialog with a prompt.
pub fn render_input_dialog(f: &mut Frame, area: Rect, title: &str, prompt: &str, editor: &LineEditor) {
    let modal = modal_area(area, 60, 15);
    f.render_widget(Clear, modal);

    let text = format!("{prompt}\n\n> {}", editor.content);
    let block = Block::default().borders(Borders::ALL).title(title.to_string());
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render hydration progress.
pub fn render_hydration_progress(f: &mut Frame, area: Rect, progress: &HydrationProgress) {
    let modal = modal_area(area, 60, 20);
    f.render_widget(Clear, modal);

    let text = format!(
        "{}\n\nMatched: {}\nTotal gists: {}\nAmbiguous: {}",
        progress.phase,
        progress.matched,
        progress.total_gists,
        progress.ambiguous.len()
    );
    let block = Block::default().borders(Borders::ALL).title("Hydration");
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render a confirmation dialog.
pub fn render_confirm(f: &mut Frame, area: Rect, message: &str) {
    let modal = modal_area(area, 50, 15);
    f.render_widget(Clear, modal);

    let text = format!("{message}\n\n[y] Yes  [n] No");
    let block = Block::default().borders(Borders::ALL).title("Confirm");
    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render a status message (non-modal, just a brief overlay).
pub fn render_message(f: &mut Frame, area: Rect, message: &str) {
    let modal = modal_area(area, 50, 10);
    f.render_widget(Clear, modal);

    let block = Block::default().borders(Borders::ALL).title("Info");
    let para = Paragraph::new(message.to_string()).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}
