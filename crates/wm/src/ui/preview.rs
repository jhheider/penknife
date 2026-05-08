use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Render a markdown file preview with basic syntax highlighting.
pub fn render_preview(f: &mut Frame, area: Rect, content: &str, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(Style::default().fg(Color::DarkGray))
        .title_style(Style::default().fg(Color::Cyan));

    let lines = highlight(content);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

/// Highlight a full markdown document. Tracks code-block state across lines.
fn highlight(content: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code_block = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            // Toggle code block; render fence in green.
            in_code_block = !in_code_block;
            out.push(Line::styled(
                line.to_string(),
                Style::default().fg(Color::Green),
            ));
            continue;
        }
        if in_code_block {
            out.push(Line::styled(
                line.to_string(),
                Style::default().fg(Color::Green),
            ));
            continue;
        }
        out.push(highlight_line(line, trimmed));
    }
    out
}

fn highlight_line(line: &str, trimmed: &str) -> Line<'static> {
    if trimmed.starts_with('#') {
        return Line::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    }
    if trimmed.starts_with('>') {
        return Line::styled(
            line.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        );
    }
    if (trimmed.starts_with("---") || trimmed.starts_with("***") || trimmed.starts_with("___"))
        && trimmed.chars().all(|c| matches!(c, '-' | '*' | '_'))
    {
        return Line::styled(
            line.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        );
    }
    // List marker
    let (prefix_len, marker_color) = if let Some(rest) = trimmed.strip_prefix("- ") {
        (line.len() - rest.len(), Some(Color::Yellow))
    } else if let Some(rest) = trimmed.strip_prefix("* ") {
        (line.len() - rest.len(), Some(Color::Yellow))
    } else {
        (0, None)
    };

    let leading_ws = &line[..line.len() - trimmed.len()];
    let mut spans: Vec<Span<'static>> = Vec::new();
    if prefix_len > 0 {
        spans.push(Span::raw(leading_ws.to_string()));
        let marker = &line[leading_ws.len()..prefix_len];
        spans.push(Span::styled(
            marker.to_string(),
            Style::default()
                .fg(marker_color.unwrap_or(Color::Yellow))
                .add_modifier(Modifier::BOLD),
        ));
        spans.extend(parse_inline(&line[prefix_len..]));
    } else {
        spans.extend(parse_inline(line));
    }
    Line::from(spans)
}

/// Tokenize inline markdown into styled spans. Handles `code`, **bold**, *italic*.
/// Does not nest. Unclosed delimiters fall through as raw text.
fn parse_inline(text: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::raw(std::mem::take(buf)));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Inline code: `...`
        if c == '`'
            && let Some(end) = chars[i + 1..].iter().position(|&c| c == '`')
        {
            flush(&mut buf, &mut spans);
            let code: String = chars[i + 1..i + 1 + end].iter().collect();
            spans.push(Span::styled(code, Style::default().fg(Color::Green)));
            i += end + 2;
            continue;
        }
        // Bold: **...**
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            // Find closing **
            let search_start = i + 2;
            let mut j = search_start;
            while j + 1 < chars.len() {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    break;
                }
                j += 1;
            }
            if j + 1 < chars.len() && chars[j] == '*' && chars[j + 1] == '*' {
                flush(&mut buf, &mut spans);
                let inner: String = chars[search_start..j].iter().collect();
                spans.push(Span::styled(
                    inner,
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                i = j + 2;
                continue;
            }
        }
        // Italic: *...* (single asterisk, not part of **)
        if c == '*' && chars.get(i + 1) != Some(&'*') {
            // Find closing single *
            let search_start = i + 1;
            let mut j = search_start;
            while j < chars.len() && chars[j] != '*' {
                j += 1;
            }
            // Require non-empty content and closing *
            if j < chars.len() && j > search_start {
                flush(&mut buf, &mut spans);
                let inner: String = chars[search_start..j].iter().collect();
                spans.push(Span::styled(
                    inner,
                    Style::default().add_modifier(Modifier::ITALIC),
                ));
                i = j + 1;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut spans);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span_texts(spans: &[Span<'static>]) -> Vec<String> {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn parse_inline_plain_text_one_span() {
        let s = parse_inline("just text");
        assert_eq!(span_texts(&s), vec!["just text"]);
    }

    #[test]
    fn parse_inline_extracts_inline_code() {
        let s = parse_inline("run `cargo build` then go");
        assert_eq!(span_texts(&s), vec!["run ", "cargo build", " then go"]);
    }

    #[test]
    fn parse_inline_extracts_bold() {
        let s = parse_inline("hello **world** here");
        assert_eq!(span_texts(&s), vec!["hello ", "world", " here"]);
    }

    #[test]
    fn parse_inline_extracts_italic() {
        let s = parse_inline("a *cool* thing");
        assert_eq!(span_texts(&s), vec!["a ", "cool", " thing"]);
    }

    #[test]
    fn parse_inline_unclosed_delimiter_falls_through() {
        // Backtick with no close → treat as raw text.
        let s = parse_inline("a `b c");
        assert_eq!(span_texts(&s), vec!["a `b c"]);
    }

    #[test]
    fn parse_inline_does_not_treat_double_star_as_italic() {
        // `**bold**` should be one bold span, not nested italic spans.
        let s = parse_inline("**x**");
        assert_eq!(span_texts(&s), vec!["x"]);
    }

    #[test]
    fn highlight_tracks_code_block_state() {
        let md = "regular\n```\nin block\n```\nafter";
        let lines = highlight(md);
        assert_eq!(lines.len(), 5);
        // The "in block" line is inside the fence, so its single span should
        // be styled green (whole-line code block style).
        let in_block = &lines[2];
        // It's a single styled line — we only verify it has at least one span.
        assert!(!in_block.spans.is_empty());
    }
}
