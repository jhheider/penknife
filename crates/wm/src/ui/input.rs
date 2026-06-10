use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// Simple line editor for text prompts (search, URL input, filename).
///
/// `cursor` is a byte offset into `content`, always kept on a char boundary
/// so multi-byte input (accents, CJK, emoji pasted from the clipboard) can't
/// panic `String::insert`/`remove`.
#[derive(Debug, Default)]
pub struct LineEditor {
    pub content: String,
    pub cursor: usize,
}

impl LineEditor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Byte offset of the char boundary immediately before the cursor.
    fn prev_boundary(&self) -> usize {
        self.content[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Byte offset of the char boundary immediately after the cursor.
    fn next_boundary(&self) -> usize {
        self.content[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
            .unwrap_or(self.cursor)
    }

    /// Handle a key event. Returns true if the event was consumed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match c {
                        'a' => self.cursor = 0,
                        'e' => self.cursor = self.content.len(),
                        'u' => {
                            self.content.drain(..self.cursor);
                            self.cursor = 0;
                        }
                        'k' => {
                            self.content.truncate(self.cursor);
                        }
                        _ => return false,
                    }
                } else {
                    self.content.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                }
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor = self.prev_boundary();
                    self.content.remove(self.cursor);
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.content.len() {
                    self.content.remove(self.cursor);
                }
                true
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = self.prev_boundary();
                }
                true
            }
            KeyCode::Right => {
                if self.cursor < self.content.len() {
                    self.cursor = self.next_boundary();
                }
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.cursor = self.content.len();
                true
            }
            _ => false,
        }
    }

    /// Render the content with a visible cursor: the char under the cursor
    /// (or a trailing space, when the cursor sits at the end) is drawn
    /// reversed. The terminal's own cursor is hidden in raw mode, so without
    /// this the user can't tell where Left/Right have taken them.
    pub fn spans(&self, style: Style) -> Vec<Span<'static>> {
        let before = &self.content[..self.cursor];
        let after = &self.content[self.cursor..];
        let (at, rest) = match after.chars().next() {
            Some(c) => (c.to_string(), &after[c.len_utf8()..]),
            None => (" ".to_string(), after),
        };
        let mut spans = Vec::with_capacity(3);
        if !before.is_empty() {
            spans.push(Span::styled(before.to_string(), style));
        }
        spans.push(Span::styled(at, style.add_modifier(Modifier::REVERSED)));
        if !rest.is_empty() {
            spans.push(Span::styled(rest.to_string(), style));
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(ed: &mut LineEditor, s: &str) {
        for c in s.chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn multibyte_typing_and_backspace() {
        let mut ed = LineEditor::new();
        type_str(&mut ed, "héllo·wörld");
        assert_eq!(ed.content, "héllo·wörld");
        ed.handle_key(key(KeyCode::Backspace));
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.content, "héllo·wör");
    }

    #[test]
    fn arrows_move_by_char_not_byte() {
        let mut ed = LineEditor::new();
        type_str(&mut ed, "aé");
        ed.handle_key(key(KeyCode::Left)); // before é
        ed.handle_key(key(KeyCode::Left)); // before a
        assert_eq!(ed.cursor, 0);
        ed.handle_key(key(KeyCode::Right));
        ed.handle_key(key(KeyCode::Char('x')));
        assert_eq!(ed.content, "axé");
    }

    #[test]
    fn insert_mid_string_after_multibyte() {
        let mut ed = LineEditor::new();
        type_str(&mut ed, "日本語");
        ed.handle_key(key(KeyCode::Left));
        ed.handle_key(key(KeyCode::Char('x')));
        assert_eq!(ed.content, "日本x語");
        ed.handle_key(key(KeyCode::Delete));
        assert_eq!(ed.content, "日本x");
    }

    #[test]
    fn cursor_span_reverses_char_under_cursor() {
        let mut ed = LineEditor::new();
        type_str(&mut ed, "abc");
        ed.handle_key(key(KeyCode::Left));
        let spans = ed.spans(Style::default());
        let texts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(texts, vec!["ab", "c"]);
        assert!(spans[1].style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn cursor_span_at_end_is_reversed_space() {
        let ed = LineEditor::new();
        let spans = ed.spans(Style::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), " ");
        assert!(spans[0].style.add_modifier.contains(Modifier::REVERSED));
    }
}
