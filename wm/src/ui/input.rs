use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Simple line editor for text prompts (search, URL input, filename).
#[derive(Debug, Default)]
pub struct LineEditor {
    pub content: String,
    pub cursor: usize,
}

impl LineEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_content(content: &str) -> Self {
        Self {
            cursor: content.len(),
            content: content.to_string(),
        }
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
                    self.cursor += 1;
                }
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
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
                self.cursor = self.cursor.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                if self.cursor < self.content.len() {
                    self.cursor += 1;
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
}
