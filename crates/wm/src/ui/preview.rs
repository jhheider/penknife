use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Render a markdown file preview with basic syntax highlighting.
pub fn render_preview(f: &mut Frame, area: Rect, content: &str, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(Style::default().fg(Color::DarkGray))
        .title_style(Style::default().fg(Color::Cyan));

    let lines: Vec<Line> = content.lines().map(highlight_md_line).collect();

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn highlight_md_line(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        Line::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if trimmed.starts_with('>') {
        Line::styled(
            line.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )
    } else if trimmed.starts_with("---") || trimmed.starts_with("***") || trimmed.starts_with("___")
    {
        Line::styled(
            line.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        )
    } else if trimmed.starts_with("```") {
        Line::styled(line.to_string(), Style::default().fg(Color::Green))
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        Line::from(vec![
            Span::styled(trimmed[..2].to_string(), Style::default().fg(Color::Yellow)),
            Span::raw(trimmed[2..].to_string()),
        ])
    } else {
        Line::raw(line.to_string())
    }
}
