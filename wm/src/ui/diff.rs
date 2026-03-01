use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use similar::{ChangeTag, TextDiff};

/// Render a unified diff between local and remote content.
pub fn render_diff(f: &mut Frame, area: Rect, local: &str, remote: &str, title: &str) {
    let diff = TextDiff::from_lines(remote, local);
    let mut lines: Vec<Line> = Vec::new();

    for change in diff.iter_all_changes() {
        let (sign, style) = match change.tag() {
            ChangeTag::Delete => ("-", Style::default().fg(Color::Red)),
            ChangeTag::Insert => ("+", Style::default().fg(Color::Green)),
            ChangeTag::Equal => (" ", Style::default().fg(Color::DarkGray)),
        };
        let text = format!("{sign}{}", change.value().trim_end_matches('\n'));
        lines.push(Line::styled(text, style));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Diff: {title}"));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}
