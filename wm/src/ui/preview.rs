use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Render a markdown file preview in the given area.
pub fn render_preview(f: &mut Frame, area: Rect, content: &str, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string());

    let paragraph = Paragraph::new(content.to_string())
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}
