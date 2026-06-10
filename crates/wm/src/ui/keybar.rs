use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode, PaneFocus};

/// One keybar entry: the key chord and a short verb.
type Hint = (&'static str, &'static str);

/// Context-sensitive key hints for the bottom bar. Returns the bindings that
/// actually do something in the current mode (and, in Normal mode, the
/// focused pane), so the user never has to open Help just to remember the
/// next keystroke.
pub fn hints(mode: &Mode, focused_pane: PaneFocus) -> Vec<Hint> {
    match mode {
        Mode::Normal => match focused_pane {
            PaneFocus::Tree => vec![
                ("Tab", "focus"),
                ("u", "push"),
                ("d", "pull"),
                ("D", "diff"),
                ("f", "fetch"),
                ("e", "edit"),
                ("/", "find"),
                ("B", "bulk"),
                ("g", "git"),
                ("?", "help"),
                ("q", "quit"),
            ],
            PaneFocus::Right => vec![
                ("Tab", "focus"),
                ("j/k", "scroll"),
                ("PgUp/PgDn", "page"),
                ("e", "edit"),
                ("?", "help"),
                ("q", "quit"),
            ],
        },
        Mode::Help | Mode::Message(_) => vec![("any key", "close")],
        Mode::Diff { .. } => vec![("j/k", "scroll"), ("PgUp/PgDn", "page"), ("Esc", "close")],
        Mode::Confirm { .. } => vec![("y/Enter", "yes"), ("n/Esc", "no")],
        Mode::FilePicker { .. } => vec![("↑/↓", "select"), ("Enter", "open"), ("Esc", "cancel")],
        Mode::GdocUrl
        | Mode::GdocFilename
        | Mode::Rename { .. }
        | Mode::LinkGist { .. }
        | Mode::AddRoot => {
            vec![("Enter", "confirm"), ("Esc", "cancel")]
        }
        Mode::SetupRoot => vec![("Enter", "confirm"), ("Ctrl+Q", "quit")],
        Mode::ReplaceQuery | Mode::ReplaceTarget => {
            vec![("Enter", "next"), ("Esc", "cancel")]
        }
        Mode::ReplaceReview { .. } => vec![
            ("Space", "toggle"),
            ("a", "all"),
            ("z", "none"),
            ("Enter", "apply"),
            ("Esc", "cancel"),
        ],
        Mode::Hydrating { done, .. } => {
            if *done {
                vec![("any key", "continue")]
            } else {
                vec![("", "hydrating…")]
            }
        }
        Mode::RootSwitcher { .. } => vec![
            ("Enter", "switch"),
            ("a", "add"),
            ("d", "delete"),
            ("Esc", "close"),
        ],
        Mode::ResolveAmbiguous { .. } => vec![
            ("j/k", "navigate"),
            ("Enter", "pick"),
            ("s", "skip"),
            ("Esc", "abort"),
        ],
        Mode::SortMenu { .. } | Mode::BulkMenu { .. } => {
            vec![("↑/↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")]
        }
    }
}

/// Render the keybar into `area` (expected height: 1 row). Keys are yellow
/// and bold, verbs dim; entries past the right edge are clipped, which is
/// fine — hints are ordered most-useful-first.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(Color::DarkGray);

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, label)) in hints(&app.mode, app.focused_pane).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", label_style));
        } else {
            spans.push(Span::raw(" "));
        }
        if !key.is_empty() {
            spans.push(Span::styled(key, key_style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(label, label_style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_tree_focus_shows_gist_verbs() {
        let h = hints(&Mode::Normal, PaneFocus::Tree);
        assert!(h.contains(&("u", "push")));
        assert!(h.contains(&("d", "pull")));
        assert!(h.contains(&("q", "quit")));
    }

    #[test]
    fn right_focus_swaps_to_scroll_hints() {
        let h = hints(&Mode::Normal, PaneFocus::Right);
        assert!(h.contains(&("j/k", "scroll")));
        assert!(!h.contains(&("u", "push")));
    }

    #[test]
    fn every_mode_has_at_least_one_hint() {
        let modes = vec![
            Mode::Normal,
            Mode::Help,
            Mode::Message("hi".into()),
            Mode::Diff {
                local: String::new(),
                remote: String::new(),
            },
            Mode::FilePicker { selected: 0 },
            Mode::GdocUrl,
            Mode::GdocFilename,
            Mode::Hydrating {
                progress: None,
                done: false,
            },
            Mode::Hydrating {
                progress: None,
                done: true,
            },
            Mode::RootSwitcher { selected: 0 },
            Mode::AddRoot,
            Mode::SetupRoot,
            Mode::ResolveAmbiguous {
                item: 0,
                selected: 0,
            },
            Mode::ReplaceQuery,
            Mode::ReplaceTarget,
            Mode::ReplaceReview { selected: 0 },
            Mode::Rename {
                old_rel: "a.md".into(),
            },
            Mode::SortMenu { selected: 0 },
            Mode::BulkMenu { selected: 0 },
        ];
        for mode in modes {
            assert!(!hints(&mode, PaneFocus::Tree).is_empty(), "{mode:?}");
        }
    }
}
