//! Multiline text editor for the prompt box (cursor is a byte index on a char boundary).
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub struct Editor {
    pub text: String,
    cursor: usize,
}

impl Editor {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }

    /// Ctrl+U: delete from the start of the line to the cursor.
    pub fn kill_line(&mut self) {
        let s = self.line_start();
        self.text.drain(s..self.cursor);
        self.cursor = s;
    }

    /// Ctrl+W: delete the previous word (never crossing a newline).
    pub fn kill_word(&mut self) {
        let s = self.line_start();
        let line = &self.text[s..self.cursor];
        let trimmed = line.trim_end();
        let start = s + trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Hard-wraps the text to `width` columns: rows plus the cursor's (row, col).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = vec![String::new()];
        let (mut col, mut cur) = (0, (0, 0));
        for (i, c) in self.text.char_indices() {
            let w = c.width().unwrap_or(0);
            if c != '\n' && col + w > width {
                rows.push(String::new());
                col = 0;
            }
            if i == self.cursor {
                cur = (rows.len() - 1, col);
            }
            if c == '\n' {
                rows.push(String::new());
                col = 0;
                continue;
            }
            if let Some(r) = rows.last_mut() {
                r.push(c);
            }
            col += w;
        }
        if self.cursor == self.text.len() {
            if col >= width {
                rows.push(String::new());
                col = 0;
            }
            cur = (rows.len() - 1, col);
        }
        (rows, cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(s: &str) -> Editor {
        let mut e = Editor::default();
        s.chars().for_each(|c| e.insert(c));
        e
    }

    #[test]
    fn edits_around_cursor_with_multibyte() {
        let mut e = ed("aéb");
        e.left();
        e.backspace();
        assert_eq!(e.text, "ab");
        e.delete();
        assert_eq!(e.text, "a");
        e.home();
        e.insert('x');
        e.end();
        e.insert('y');
        assert_eq!(e.text, "xay");
    }

    #[test]
    fn kill_word_and_line_stay_on_the_current_line() {
        let mut e = ed("one two\nthree four");
        e.kill_word();
        assert_eq!(e.text, "one two\nthree ");
        e.kill_line();
        assert_eq!(e.text, "one two\n");
        e.kill_word(); // nothing before the cursor on this line
        assert_eq!(e.text, "one two\n");
        e.left();
        assert_eq!(e.layout(20).1, (0, 7));
    }

    #[test]
    fn layout_wraps_and_places_cursor() {
        let (rows, cur) = ed("abcdef\ng").layout(4);
        assert_eq!(rows, ["abcd", "ef", "g"]);
        assert_eq!(cur, (2, 1));
        let (rows, cur) = ed("abcd").layout(4);
        assert_eq!(rows, ["abcd", ""]);
        assert_eq!(cur, (1, 0));
    }

    #[test]
    fn take_resets() {
        let mut e = ed("hi");
        assert_eq!(e.take(), "hi");
        assert!(e.is_empty());
        e.insert('a');
        assert_eq!(e.text, "a");
    }
}
