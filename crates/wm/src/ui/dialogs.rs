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
pub fn render_help(f: &mut Frame, area: Rect, app: &App) {
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
                ("m", "Rename / move the selected file"),
                ("=", "Toggle JSON between compact and pretty in place"),
                ("X", "Delete remote gist (keeps local file)"),
                ("_", "Move local file to system trash (with confirm)"),
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
            "Git (when root is in a repo)",
            &[
                ("g", "Show `git status` in suspended terminal"),
                (
                    "G",
                    "Show `git log -p <file>` (or repo-wide if no selection)",
                ),
                ("(", "git pull --rebase (with confirm)"),
                (")", "git push (with confirm)"),
            ],
        ),
        (
            "Files & roots",
            &[
                ("/", "Fuzzy file picker (fzf-style)"),
                ("O", "Pick sort order for the tree"),
                ("B", "Bulk ops menu (push/pull dirty, format JSON, prune)"),
                ("s", "Find & replace (recursive within current scope)"),
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
    // Configured aliases, if any.
    if !app.config.aliases.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("Aliases (from config.toml)", header_style));
        for (k, cmd) in &app.config.aliases {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{k:<12}"), key_style),
                Span::raw(" "),
                Span::styled(cmd.clone(), desc_style),
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
        lines.push(Line::styled(
            format!("{marker}{}", root.path.display()),
            style,
        ));
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

/// Render the find-and-replace review dialog. Top line summarizes the
/// substitution and scope; below, a scrollable checklist where each row is
/// one match (rel_path:line + the line text with the matched substring
/// highlighted). Space toggles, a/z select all/none, Enter applies, Esc
/// aborts.
pub fn render_replace_review(f: &mut Frame, area: Rect, app: &App, selected: usize) {
    let modal = modal_area(area, 85, 80);
    f.render_widget(Clear, modal);

    let g = crate::glyphs::glyphs();
    let total = app.replace_matches.len();
    let checked = app.replace_checked.iter().filter(|c| **c).count();

    let title_line = Line::from(vec![
        Span::styled(format!("{} ", g.search), Style::default().fg(Color::Yellow)),
        Span::styled(
            "Replace",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("(", Style::default().fg(Color::DarkGray)),
        Span::styled(
            checked.to_string(),
            Style::default()
                .fg(if checked == 0 {
                    Color::DarkGray
                } else {
                    Color::Green
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("/", Style::default().fg(Color::DarkGray)),
        Span::styled(total.to_string(), Style::default().fg(Color::White)),
        Span::styled(" checked)", Style::default().fg(Color::DarkGray)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_line)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    let chunks = Layout::vertical([
        Constraint::Length(1), // summary line
        Constraint::Length(1), // spacer
        Constraint::Min(1),    // results
        Constraint::Length(1), // hints
    ])
    .split(inner);

    // Summary line: 'foo' → 'bar' in scope/path
    let dim = Style::default().fg(Color::DarkGray);
    let summary = Line::from(vec![
        Span::styled("'", dim),
        Span::styled(
            app.replace_query.clone(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::styled("' → '", dim),
        Span::styled(
            if app.replace_target.is_empty() {
                "(empty — delete matches)".to_string()
            } else {
                app.replace_target.clone()
            },
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("' in ", dim),
        Span::styled(app.replace_scope_label(), Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(summary), chunks[0]);

    // Scrolling viewport for the list.
    let view_h = chunks[2].height as usize;
    let start = if view_h == 0 {
        0
    } else if selected >= view_h {
        selected + 1 - view_h
    } else {
        0
    };
    let end = (start + view_h).min(total);

    let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
    for row_idx in start..end {
        let m = &app.replace_matches[row_idx];
        let is_checked = app.replace_checked.get(row_idx).copied().unwrap_or(false);
        let is_selected = row_idx == selected;
        let row_bg = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        let mark = if is_checked { "✓" } else { " " };
        let mark_color = if is_checked {
            Color::Green
        } else {
            Color::DarkGray
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        // Selection caret + checkbox.
        spans.push(Span::styled(
            if is_selected { " ▶ " } else { "   " },
            row_bg,
        ));
        spans.push(Span::styled(
            format!("[{mark}] "),
            if is_selected {
                row_bg.add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(mark_color).add_modifier(Modifier::BOLD)
            },
        ));
        // Path:line — magenta path, cyan line number.
        spans.push(Span::styled(
            format!("{}:{}", m.rel_path, m.line),
            if is_selected {
                row_bg
            } else {
                Style::default().fg(Color::Magenta)
            },
        ));
        spans.push(Span::raw("  "));
        // Line context with the match highlighted.
        let line = &m.line_text;
        let end_byte = m.col_byte + app.replace_query.len();
        let before = line.get(..m.col_byte).unwrap_or("");
        let hit = line.get(m.col_byte..end_byte).unwrap_or("");
        let after = line.get(end_byte..).unwrap_or("");
        // Trim long lines to fit. Keep ~30 chars on each side of the match.
        let (before, after) = trim_context(before, after, 30);
        spans.push(Span::styled(before, row_bg));
        spans.push(Span::styled(
            hit.to_string(),
            if is_selected {
                row_bg.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            },
        ));
        spans.push(Span::styled(after, row_bg));
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::styled("  (no matches)", dim));
    }
    f.render_widget(Paragraph::new(lines), chunks[2]);

    let hints = Line::from(vec![
        Span::styled("Space ", Style::default().fg(Color::Yellow)),
        Span::styled("toggle  ", dim),
        Span::styled("a ", Style::default().fg(Color::Yellow)),
        Span::styled("all  ", dim),
        Span::styled("z ", Style::default().fg(Color::Yellow)),
        Span::styled("none  ", dim),
        Span::styled("↑/↓ ", Style::default().fg(Color::Yellow)),
        Span::styled("move  ", dim),
        Span::styled("Enter ", Style::default().fg(Color::Green)),
        Span::styled("apply  ", dim),
        Span::styled("Esc ", Style::default().fg(Color::Red)),
        Span::styled("cancel", dim),
    ]);
    f.render_widget(Paragraph::new(hints), chunks[3]);
}

/// Truncate `before`/`after` context strings around a match so each fits in
/// roughly `pad` chars. Front-ellipsizes the "before" side and back-ellipsizes
/// the "after" side so the matched substring stays visible.
fn trim_context(before: &str, after: &str, pad: usize) -> (String, String) {
    let b_chars: Vec<char> = before.chars().collect();
    let a_chars: Vec<char> = after.chars().collect();
    let b_out = if b_chars.len() > pad {
        let tail: String = b_chars[b_chars.len() - pad..].iter().collect();
        format!("…{tail}")
    } else {
        before.to_string()
    };
    let a_out = if a_chars.len() > pad {
        let head: String = a_chars[..pad].iter().collect();
        format!("{head}…")
    } else {
        after.to_string()
    };
    (b_out, a_out)
}

/// Render the bulk-operations picker. Shows the four ops, each with its
/// precomputed file count colored by emptiness (dim if 0, yellow/bold if >0).
pub fn render_bulk_menu(f: &mut Frame, area: Rect, app: &App, selected: usize) {
    let modal = modal_area(area, 60, 40);
    f.render_widget(Clear, modal);

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Bulk operations", g.file_pane))
        .border_style(Style::default().fg(Color::Yellow))
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let opts = app.bulk_options();
    let mut lines: Vec<Line> = Vec::new();
    for (i, opt) in opts.iter().enumerate() {
        let is_selected = i == selected;
        let count = opt.count();
        let marker = if is_selected { " ▶ " } else { "   " };
        let row_style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };
        let count_style = if is_selected {
            row_style.add_modifier(Modifier::BOLD)
        } else if count == 0 {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), row_style),
            Span::styled(format!("{:<26}", opt.label()), row_style),
            Span::styled(format!("({count})"), count_style),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  ↑/↓ navigate · Enter run (with confirm) · Esc cancel",
        Style::default().fg(Color::DarkGray),
    ));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, modal);
}

/// Render the sort-mode picker. Lists the five sort modes with the active
/// one marked, and the cursor on `selected`.
pub fn render_sort_menu(f: &mut Frame, area: Rect, app: &App, selected: usize) {
    let modal = modal_area(area, 50, 30);
    f.render_widget(Clear, modal);

    let g = crate::glyphs::glyphs();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} Sort by", g.file_pane))
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let active = app.config.sort.mode;
    let mut lines: Vec<Line> = Vec::new();
    for (i, mode) in crate::config::SortMode::all().iter().enumerate() {
        let is_selected = i == selected;
        let is_active = *mode == active;
        let marker = if is_active { " ● " } else { "   " };
        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if is_active {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::styled(format!("{marker}{}", mode.label()), style));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  ↑/↓ navigate · Enter select · Esc cancel",
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
