pub const MAX_INPUT_CHARS: usize = 512;
pub const MAX_HISTORY: usize = 200;

#[derive(Debug, Default)]
pub struct InputBuffer {
    buffer: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_cursor: usize,
}

impl InputBuffer {
    pub fn new(history: impl IntoIterator<Item = String>) -> InputBuffer {
        let mut history: Vec<String> = history
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if history.len() > MAX_HISTORY {
            history.drain(..history.len() - MAX_HISTORY);
        }
        let history_cursor = history.len();
        InputBuffer {
            buffer: Vec::new(),
            cursor: 0,
            history,
            history_cursor,
        }
    }

    pub fn text(&self) -> String {
        self.buffer.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn at_end(&self) -> bool {
        self.cursor >= self.buffer.len()
    }

    pub fn append(&mut self, ch: char) {
        if self.buffer.len() < MAX_INPUT_CHARS {
            self.buffer.insert(self.cursor, ch);
            self.cursor += 1;
        }
        self.history_cursor = self.history.len();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.buffer.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.history_cursor = self.history.len();
    }

    pub fn take(&mut self) -> String {
        let value = self.text();
        self.clear();
        value
    }

    pub fn set(&mut self, value: &str) {
        let chars: Vec<char> = value.chars().collect();
        let start = chars.len().saturating_sub(MAX_INPUT_CHARS);
        self.buffer = chars[start..].to_vec();
        self.cursor = self.buffer.len();
        self.history_cursor = self.history.len();
    }

    pub fn remember(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        if self.history.last().is_none_or(|last| last != command) {
            self.history.push(command.to_owned());
            if self.history.len() > MAX_HISTORY {
                self.history.drain(..self.history.len() - MAX_HISTORY);
            }
        }
        self.history_cursor = self.history.len();
    }

    pub fn previous_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        self.history_cursor = self.history_cursor.saturating_sub(1);
        self.load_history();
    }

    pub fn next_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        self.history_cursor = (self.history_cursor + 1).min(self.history.len());
        if self.history_cursor == self.history.len() {
            self.buffer.clear();
        } else {
            self.load_history();
        }
        self.cursor = self.buffer.len();
    }

    fn load_history(&mut self) {
        self.buffer = self.history[self.history_cursor].chars().collect();
        self.cursor = self.buffer.len();
    }
}
