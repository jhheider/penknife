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

    // Section headers and key/description pairs. Each pair is rendered with
    // the key chord in yellow/bold and the description in default white —
    // makes the table easier to scan than the previous monochrome block.
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("Tab", "Toggle focus: tree pane ↔ preview/diff pane"),
                ("j/k  ↑/↓", "Navigate the focused pane"),
                ("Enter  l  →", "Expand / select (tree pane)"),
                ("h  ←  Bksp", "Collapse (tree pane)"),
                ("PgUp/PgDn", "Scroll preview/diff (any focus)"),
                ("n / N", "Jump to next / previous non-synced file"),
            ],
        ),
        (
            "Gist actions",
            &[
                ("u", "Push selected file to gist"),
                ("d", "Pull remote into selected file"),
                ("c", "Copy gist URL to clipboard"),
                ("o", "Open gist URL in browser"),
                ("e", "Edit selected file in $EDITOR"),
                ("X", "Delete remote gist (keeps local file)"),
                ("D", "Diff local vs remote"),
                ("H", "Hydrate — match existing gists to files"),
            ],
        ),
        (
            "Clipboard",
            &[
                ("C", "Copy selected file's contents to clipboard"),
                ("V", "Paste clipboard (rich HTML → markdown) as new file"),
            ],
        ),
        (
            "Files & roots",
            &[
                ("/", "Fuzzy file picker (fzf-style)"),
                ("I", "Import a Google Doc as markdown"),
                ("R", "Switch root directory"),
                ("r", "Refresh file tree"),
                ("?", "This help"),
                ("q", "Quit"),
            ],
        ),
    ];

    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::White);
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();
    for (idx, (header, pairs)) in sections.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(header.to_string(), header_style));
        for (key, desc) in *pairs {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{key:<12}"), key_style),
                Span::raw(" "),
                Span::styled(desc.to_string(), desc_style),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "In Diff view: j/k, arrows, PgUp/PgDn scroll; Esc/q exits.",
        dim,
    ));
    lines.push(Line::styled(
        "Mouse: cmd-click on URLs and native selection work by default.",
        dim,
    ));
    lines.push(Line::styled(
        "Set WM_MOUSE=1 to enable click-to-select + wheel-scroll routing.",
        dim,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Press any key to close.",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::ITALIC),
    ));

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Help", g.help))
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render the fzf-style file picker overlay. Top row is the query input;
/// the rest is a ranked list of matching paths with the matched characters
/// highlighted. Selected row is inverted.
pub fn render_file_picker(f: &mut Frame, area: Rect, app: &App, selected: usize) {
    let modal = modal_area(area, 75, 70);
    f.render_widget(Clear, modal);

    let g = crate::glyphs::glyphs();
    let total = app.files.len();
    let shown = app.picker_matches.len();
    let yellow_bold = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let title_line = Line::from(vec![
        Span::styled(format!("{} ", g.search), Style::default().fg(Color::Yellow)),
        Span::styled("Find file", yellow_bold),
        Span::raw("  "),
        Span::styled("(", Style::default().fg(Color::DarkGray)),
        Span::styled(
            shown.to_string(),
            Style::default()
                .fg(if shown == 0 {
                    Color::DarkGray
                } else {
                    Color::Cyan
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("/", Style::default().fg(Color::DarkGray)),
        Span::styled(total.to_string(), Style::default().fg(Color::White)),
        Span::styled(")", Style::default().fg(Color::DarkGray)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_line)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    let chunks = Layout::vertical([
        Constraint::Length(1), // query line
        Constraint::Length(1), // separator
        Constraint::Min(1),    // results
        Constraint::Length(1), // hints
    ])
    .split(inner);

    // Query line
    let query = Line::from(vec![
        Span::styled(
            "> ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.picker_editor.content.clone()),
    ]);
    f.render_widget(Paragraph::new(query), chunks[0]);

    // Visible window: clamp `selected` into a scrolling viewport that keeps
    // the cursor in view without bouncing.
    let view_h = chunks[2].height as usize;
    let start = if view_h == 0 {
        0
    } else if selected >= view_h {
        selected + 1 - view_h
    } else {
        0
    };
    let end = (start + view_h).min(app.picker_matches.len());

    let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
    for (i, m) in app.picker_matches[start..end].iter().enumerate() {
        let row_idx = start + i;
        let is_selected = row_idx == selected;
        let row_style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        let marker = if is_selected { "▶ " } else { "  " };
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(marker, row_style));
        // Render rel_path char-by-char, highlighting indices that nucleo
        // identified as match positions.
        let mut idx_iter = m.indices.iter().copied().peekable();
        for (pos, ch) in m.rel_path.chars().enumerate() {
            let highlighted = matches!(idx_iter.peek(), Some(&p) if p as usize == pos);
            if highlighted {
                idx_iter.next();
            }
            let style = match (is_selected, highlighted) {
                (true, true) => row_style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                (true, false) => row_style,
                (false, true) => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                (false, false) => Style::default(),
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "  (no matches)",
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(lines), chunks[2]);

    // Hint footer
    let hints = Line::styled(
        "↑/↓ or Ctrl-n/p select · Enter open · Esc cancel",
        Style::default().fg(Color::DarkGray),
    );
    f.render_widget(Paragraph::new(hints), chunks[3]);
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

    let lines = vec![
        Line::styled(
            prompt.to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "> ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(editor.content.clone(), Style::default().fg(Color::Yellow)),
        ]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_string())
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render hydration progress with a gauge bar.
pub fn render_hydration_progress(f: &mut Frame, area: Rect, progress: &HydrationProgress) {
    let modal = modal_area(area, 60, 25);
    f.render_widget(Clear, modal);

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Hydration", g.hydrating))
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

    let bold = Modifier::BOLD;
    let yellow_bold = Style::default().fg(Color::Yellow).add_modifier(bold);
    let dim = Style::default().fg(Color::DarkGray);
    let lines = vec![
        Line::styled(progress.phase.clone(), yellow_bold),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Matched: ", dim),
            Span::styled(
                progress.matched.to_string(),
                Style::default().fg(Color::Green).add_modifier(bold),
            ),
            Span::styled("   Total gists: ", dim),
            Span::styled(
                progress.total_gists.to_string(),
                Style::default().fg(Color::Cyan).add_modifier(bold),
            ),
            Span::styled("   Ambiguous: ", dim),
            Span::styled(
                progress.ambiguous.len().to_string(),
                Style::default()
                    .fg(if progress.ambiguous.is_empty() {
                        Color::DarkGray
                    } else {
                        Color::Yellow
                    })
                    .add_modifier(bold),
            ),
        ]),
    ];
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
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

    let bold = Modifier::BOLD;
    let lines = vec![
        Line::styled(message.to_string(), Style::default().fg(Color::White)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("[", Style::default().fg(Color::DarkGray)),
            Span::styled("y", Style::default().fg(Color::Green).add_modifier(bold)),
            Span::styled("] ", Style::default().fg(Color::DarkGray)),
            Span::styled("Yes", Style::default().fg(Color::Green)),
            Span::raw("   "),
            Span::styled("[", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::Red).add_modifier(bold)),
            Span::styled("] ", Style::default().fg(Color::DarkGray)),
            Span::styled("No", Style::default().fg(Color::Red)),
        ]),
    ];

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{}  Confirm", g.warn))
        .border_style(Style::default().fg(Color::Red))
        .title_style(Style::default().fg(Color::Red).add_modifier(bold));
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render a status message overlay.
pub fn render_message(f: &mut Frame, area: Rect, message: &str) {
    let modal = modal_area(area, 50, 10);
    f.render_widget(Clear, modal);

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Info", g.info))
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

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Root Directories", g.root))
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
    let g = crate::glyphs::glyphs();
    let title = format!(
        "{} Resolve ambiguous match ({} of {})",
        g.question,
        item + 1,
        total
    );
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

    let g = crate::glyphs::glyphs();
    let is_setup = matches!(app.mode, crate::app::Mode::SetupRoot);
    let title: String = if is_setup {
        format!("{} Welcome to Writings Manager", g.welcome)
    } else {
        format!("{} Add Root Directory", g.root)
    };
    let prompt = if is_setup {
        "Enter path to your writings folder:"
    } else {
        "Enter path to add:"
    };
    let hint = if is_setup {
        "(Enter to confirm · Ctrl+Q to quit)"
    } else {
        "(Enter to confirm · Esc to cancel)"
    };

    let lines = vec![
        Line::styled(
            prompt.to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "> ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.input_editor.content.clone(),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::raw(""),
        Line::styled(
            hint.trim().to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}
