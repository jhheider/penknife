use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use similar::{ChangeTag, TextDiff};

/// Render a unified diff between local and remote content.
pub fn render_diff(
    f: &mut Frame,
    area: Rect,
    local: &str,
    remote: &str,
    title: &str,
    scroll: u16,
    focused: bool,
) {
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

    let border = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Diff: {title}"))
        .border_style(Style::default().fg(border));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(paragraph, area);
}
