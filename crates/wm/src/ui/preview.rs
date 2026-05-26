use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Render a markdown file preview with basic syntax highlighting.
pub fn render_preview(
    f: &mut Frame,
    area: Rect,
    content: &str,
    title: Line<'static>,
    scroll: u16,
    focused: bool,
) {
    let border = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border));

    let lines = highlight(content);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    f.render_widget(paragraph, area);
}

/// Highlight a full markdown document. Tracks code-block state across lines
/// and groups runs of pipe-prefixed lines into rendered tables.
fn highlight(content: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code_block = false;
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            // Toggle code block; render fence in green.
            in_code_block = !in_code_block;
            out.push(Line::styled(
                line.to_string(),
                Style::default().fg(Color::Green),
            ));
            i += 1;
            continue;
        }
        if in_code_block {
            out.push(Line::styled(
                line.to_string(),
                Style::default().fg(Color::Green),
            ));
            i += 1;
            continue;
        }
        if let Some(consumed) = try_render_table(&lines, i, &mut out) {
            i += consumed;
            continue;
        }
        // Inline <br> support: split the source line into visual sub-lines.
        // The first piece keeps the original leading whitespace; subsequent
        // pieces start fresh (a <br> doesn't preserve indentation).
        let pieces = split_br(line);
        for (n, sub) in pieces.iter().enumerate() {
            let s = if n == 0 {
                sub.as_str()
            } else {
                sub.trim_start()
            };
            let sub_trimmed = s.trim_start();
            out.push(highlight_line(s, sub_trimmed));
        }
        i += 1;
    }
    out
}

/// If a markdown table starts at `lines[start]`, render it into `out` and
/// return the number of source lines consumed. Otherwise return None.
///
/// Recognizes the standard GFM shape: a header row, a separator row whose
/// cells are all dashes (optionally with `:` for alignment), then zero or
/// more body rows — all starting with `|` after trim.
fn try_render_table(lines: &[&str], start: usize, out: &mut Vec<Line<'static>>) -> Option<usize> {
    if start + 1 >= lines.len() {
        return None;
    }
    let header_raw = lines[start].trim();
    let sep_raw = lines[start + 1].trim();
    if !is_table_row(header_raw) || !is_table_separator(sep_raw) {
        return None;
    }

    // Each cell becomes a Vec<String> of sub-lines (split on inline <br>).
    // A row is Vec<cell-sub-lines>; the table is header + body of such rows.
    let header_cells: Vec<Vec<String>> = split_cells(header_raw)
        .into_iter()
        .map(|c| split_br(&c))
        .collect();
    let col_count = header_cells.len();
    if col_count == 0 {
        return None;
    }

    let mut body: Vec<Vec<Vec<String>>> = Vec::new();
    let mut idx = start + 2;
    while idx < lines.len() {
        let row = lines[idx].trim();
        if !is_table_row(row) {
            break;
        }
        let mut cells = split_cells(row);
        cells.resize(col_count, String::new());
        let split: Vec<Vec<String>> = cells.iter().map(|c| split_br(c)).collect();
        body.push(split);
        idx += 1;
    }

    // Column widths: max visible width across all sub-lines of every cell in
    // that column (header + body).
    let mut widths: Vec<usize> = vec![1; col_count];
    let measure = |sub_lines: &Vec<Vec<String>>, widths: &mut Vec<usize>| {
        for (j, cell_subs) in sub_lines.iter().enumerate() {
            for s in cell_subs {
                widths[j] = widths[j].max(visible_width(s));
            }
        }
    };
    measure(&header_cells, &mut widths);
    for row in &body {
        measure(row, &mut widths);
    }
    for w in widths.iter_mut() {
        *w = (*w).max(1);
    }

    let border_style = Style::default().fg(Color::DarkGray);
    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let sep = Span::styled(" │ ", border_style);

    out.push(Line::styled(
        border_row("┌", "┬", "┐", &widths),
        border_style,
    ));
    render_logical_row(&header_cells, &widths, &sep, Some(header_style), out);
    out.push(Line::styled(
        border_row("├", "┼", "┤", &widths),
        border_style,
    ));
    for row in &body {
        render_logical_row(row, &widths, &sep, None, out);
    }
    out.push(Line::styled(
        border_row("└", "┴", "┘", &widths),
        border_style,
    ));

    Some(idx - start)
}

/// Render one logical row (which may have cells of differing height because
/// of inline `<br>`) by emitting `max_height` visual lines. Cells with fewer
/// sub-lines pad blank on the trailing visual rows.
fn render_logical_row(
    cells: &[Vec<String>],
    widths: &[usize],
    sep: &Span<'static>,
    force_style: Option<Style>,
    out: &mut Vec<Line<'static>>,
) {
    let height = cells.iter().map(|c| c.len().max(1)).max().unwrap_or(1);
    for vl in 0..height {
        let row: Vec<String> = cells
            .iter()
            .map(|c| c.get(vl).cloned().unwrap_or_default())
            .collect();
        out.push(render_row(&row, widths, sep, force_style));
    }
}

fn is_table_row(line: &str) -> bool {
    line.starts_with('|') && line.len() > 1
}

/// A GFM table separator: cells are dashes only, optionally bracketed by `:`
/// for alignment. We don't honor alignment yet, just detect.
fn is_table_separator(line: &str) -> bool {
    if !line.starts_with('|') {
        return false;
    }
    let cells = split_cells(line);
    if cells.is_empty() {
        return false;
    }
    cells.iter().all(|c| {
        let trimmed = c.trim().trim_start_matches(':').trim_end_matches(':');
        !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '-')
    })
}

fn split_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim().trim_start_matches('|').trim_end_matches('|');
    trimmed.split('|').map(|c| c.trim().to_string()).collect()
}

/// Split `text` on inline `<br>` tags (case-insensitive, with optional self-
/// closing slash and surrounding whitespace: `<br>`, `<br/>`, `<br />`, `<BR>`).
/// Returns at least one element — the original text if no tags were found.
/// Used for both body content (line continuations) and table cells (multi-
/// line cells).
fn split_br(text: &str) -> Vec<String> {
    if !text.contains('<') && !text.contains('＜') {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            // Try to match "<br" then optional whitespace, optional "/", more
            // whitespace, then ">".
            let next1 = chars.get(i + 1).copied();
            let next2 = chars.get(i + 2).copied();
            if next1.is_some_and(|c| c.eq_ignore_ascii_case(&'b'))
                && next2.is_some_and(|c| c.eq_ignore_ascii_case(&'r'))
            {
                let mut j = i + 3;
                while j < chars.len() && chars[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == '/' {
                    j += 1;
                    while j < chars.len() && chars[j].is_ascii_whitespace() {
                        j += 1;
                    }
                }
                if j < chars.len() && chars[j] == '>' {
                    out.push(std::mem::take(&mut buf));
                    i = j + 1;
                    continue;
                }
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    out.push(buf);
    out
}

/// Visible width of a cell after inline-markdown parsing strips delimiters.
fn visible_width(cell: &str) -> usize {
    parse_inline(cell)
        .iter()
        .map(|s| s.content.chars().count())
        .sum()
}

fn border_row(left: &str, mid: &str, right: &str, widths: &[usize]) -> String {
    let mut s = String::new();
    s.push_str(left);
    for (j, w) in widths.iter().enumerate() {
        for _ in 0..(w + 2) {
            s.push('─');
        }
        if j + 1 < widths.len() {
            s.push_str(mid);
        }
    }
    s.push_str(right);
    s
}

/// Render a content row: leading │, cells padded to column width, separators
/// between them. If `force_style` is Some, every body span is restyled with it
/// (used for the header row to make all text bold/cyan even if cell content
/// contains inline markdown like `code`).
fn render_row(
    cells: &[String],
    widths: &[usize],
    sep: &Span<'static>,
    force_style: Option<Style>,
) -> Line<'static> {
    let border_style = Style::default().fg(Color::DarkGray);
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled("│ ", border_style));
    for (j, cell) in cells.iter().enumerate() {
        let mut cell_spans = parse_inline(cell);
        if let Some(s) = force_style {
            cell_spans = cell_spans
                .into_iter()
                .map(|span| {
                    let content = span.content.into_owned();
                    Span::styled(content, s)
                })
                .collect();
        }
        let cell_w: usize = cell_spans.iter().map(|s| s.content.chars().count()).sum();
        spans.extend(cell_spans);
        let pad = widths.get(j).copied().unwrap_or(0).saturating_sub(cell_w);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        if j + 1 < cells.len() {
            spans.push(sep.clone());
        }
    }
    spans.push(Span::styled(" │", border_style));
    Line::from(spans)
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

    #[test]
    fn detects_simple_table() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let lines = highlight(md);
        // top border, header, mid, body, bottom = 5 lines.
        assert_eq!(lines.len(), 5);
        let top: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let bot: String = lines[4].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(top.starts_with('┌') && top.ends_with('┐'));
        assert!(bot.starts_with('└') && bot.ends_with('┘'));
    }

    #[test]
    fn table_pads_columns_to_widest_cell() {
        let md = "| a | longvalue |\n|---|---|\n| longerleft | x |\n";
        let lines = highlight(md);
        // 5 rendered lines, each must have the same visible width.
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>()
            })
            .collect();
        // All five rows align (top/mid/bottom borders and header/body content).
        let first = widths[0];
        assert!(widths.iter().all(|&w| w == first), "widths: {widths:?}");
    }

    #[test]
    fn non_table_pipes_pass_through_unchanged() {
        let md = "| this is just a sentence with a pipe |\nno separator below";
        let lines = highlight(md);
        // Should NOT be rendered as a table: the line after the alleged header
        // isn't a dash-separator row.
        assert_eq!(lines.len(), 2);
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first.contains('|'));
    }

    #[test]
    fn split_br_handles_common_variants() {
        assert_eq!(split_br("a<br>b"), vec!["a", "b"]);
        assert_eq!(split_br("a<br/>b"), vec!["a", "b"]);
        assert_eq!(split_br("a<br />b"), vec!["a", "b"]);
        assert_eq!(split_br("a<BR>b"), vec!["a", "b"]);
        assert_eq!(split_br("no breaks"), vec!["no breaks"]);
        // Trailing <br> produces an empty trailing element (intentional).
        assert_eq!(split_br("end<br>"), vec!["end", ""]);
        // Non-br tag stays as literal text.
        assert_eq!(split_br("<b>not a br</b>"), vec!["<b>not a br</b>"]);
    }

    #[test]
    fn body_line_with_br_emits_multiple_visual_lines() {
        let md = "first<br>second<br>third";
        let lines = highlight(md);
        assert_eq!(lines.len(), 3);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(text, vec!["first", "second", "third"]);
    }

    #[test]
    fn table_cell_br_expands_row_height() {
        // Right cell has 3 sub-lines, left has 1 — row should render as 3 visual
        // body lines, with the left cell padded blank on rows 2 and 3.
        let md = "| Name | Stats |\n|---|---|\n| Goblin | HP: 7<br>AC: 15<br>STR: 8 |\n";
        let lines = highlight(md);
        // top + header + mid + 3 body + bottom = 7
        assert_eq!(lines.len(), 7);
        let body_widths: Vec<usize> = lines[3..6]
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>()
            })
            .collect();
        // All three body visual rows align to the same total width.
        let first = body_widths[0];
        assert!(
            body_widths.iter().all(|&w| w == first),
            "body widths: {body_widths:?}"
        );
        // The second and third visual body rows should contain "AC:" and "STR:"
        // somewhere in their content (the right cell continues).
        let row2_text: String = lines[4].spans.iter().map(|s| s.content.as_ref()).collect();
        let row3_text: String = lines[5].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(row2_text.contains("AC: 15"));
        assert!(row3_text.contains("STR: 8"));
    }

    #[test]
    fn table_inside_code_block_is_not_parsed() {
        let md = "```\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n";
        let lines = highlight(md);
        // 5 lines, all inside the fence — none should be rendered as box-drawing.
        for l in &lines {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                !text.contains('┌') && !text.contains('└'),
                "unexpected box-drawing in {text:?}"
            );
        }
    }
}
