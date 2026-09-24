//! Multi-line prompt editor with history. Deliberately small: no external
//! text-area crate, so the composer cannot drift out of step with ratatui.

use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const MAX_ROWS: u16 = 6;
pub const HISTORY_LIMIT: usize = 100;

#[derive(Debug, Default, Clone)]
pub struct Composer {
    lines: Vec<String>,
    /// (line index, char index within the line)
    cursor: (usize, usize),
    history: Vec<String>,
    history_pos: Option<usize>,
    draft: Option<Vec<String>>,
}

impl Composer {
    pub fn new(history: Vec<String>) -> Self {
        Self {
            lines: vec![String::new()],
            cursor: (0, 0),
            history,
            history_pos: None,
            draft: None,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    /// The cursor's character column on its current line — used by the
    /// Agents-screen Left-key rule, which only fires at column 0 (otherwise
    /// Left just moves the cursor, as usual).
    pub fn cursor_col(&self) -> usize {
        self.cursor.1
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let last = self.lines.len() - 1;
        self.cursor = (last, self.lines[last].chars().count());
    }

    pub fn current_token(&self) -> String {
        let (row, col) = self.cursor;
        let before: String = self.lines[row].chars().take(col).collect();
        before
            .rsplit_once(char::is_whitespace)
            .map(|(_, token)| token)
            .unwrap_or(&before)
            .to_string()
    }

    pub fn replace_current_token(&mut self, replacement: &str) {
        let (row, col) = self.cursor;
        let before: String = self.lines[row].chars().take(col).collect();
        let after: String = self.lines[row].chars().skip(col).collect();
        let after_token = after
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map(|(index, _)| &after[index..])
            .unwrap_or("");
        let start = before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(idx, c)| idx + c.len_utf8())
            .unwrap_or(0);
        let prefix = &before[..start];
        self.lines[row] = format!("{prefix}{replacement}{after_token}");
        self.cursor.1 = prefix.chars().count() + replacement.chars().count();
    }

    /// Take the text, push it to history, and reset.
    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > HISTORY_LIMIT {
                let excess = self.history.len() - HISTORY_LIMIT;
                self.history.drain(0..excess);
            }
        }
        self.clear();
        text
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor = (0, 0);
        self.history_pos = None;
        self.draft = None;
    }

    fn line_len(&self, idx: usize) -> usize {
        self.lines[idx].chars().count()
    }

    fn byte_index(line: &str, char_idx: usize) -> usize {
        line.char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.newline();
            return;
        }
        let (row, col) = self.cursor;
        let at = Self::byte_index(&self.lines[row], col);
        self.lines[row].insert(at, c);
        self.cursor.1 += 1;
        self.history_pos = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\r' {
                continue;
            }
            self.insert_char(c);
        }
    }

    pub fn newline(&mut self) {
        let (row, col) = self.cursor;
        let at = Self::byte_index(&self.lines[row], col);
        let rest = self.lines[row].split_off(at);
        self.lines.insert(row + 1, rest);
        self.cursor = (row + 1, 0);
    }

    pub fn backspace(&mut self) {
        let (row, col) = self.cursor;
        if col > 0 {
            let at = Self::byte_index(&self.lines[row], col - 1);
            self.lines[row].remove(at);
            self.cursor.1 -= 1;
        } else if row > 0 {
            let removed = self.lines.remove(row);
            let prev_len = self.line_len(row - 1);
            self.lines[row - 1].push_str(&removed);
            self.cursor = (row - 1, prev_len);
        }
    }

    pub fn delete(&mut self) {
        let (row, col) = self.cursor;
        if col < self.line_len(row) {
            let at = Self::byte_index(&self.lines[row], col);
            self.lines[row].remove(at);
        } else if row + 1 < self.lines.len() {
            let next = self.lines.remove(row + 1);
            self.lines[row].push_str(&next);
        }
    }

    pub fn left(&mut self) {
        let (row, col) = self.cursor;
        if col > 0 {
            self.cursor.1 -= 1;
        } else if row > 0 {
            self.cursor = (row - 1, self.line_len(row - 1));
        }
    }

    pub fn right(&mut self) {
        let (row, col) = self.cursor;
        if col < self.line_len(row) {
            self.cursor.1 += 1;
        } else if row + 1 < self.lines.len() {
            self.cursor = (row + 1, 0);
        }
    }

    pub fn home(&mut self) {
        self.cursor.1 = 0;
    }

    pub fn end(&mut self) {
        self.cursor.1 = self.line_len(self.cursor.0);
    }

    /// Up: move within the text, or step back through history from the
    /// first line.
    pub fn up(&mut self) {
        let (row, col) = self.cursor;
        if row > 0 {
            self.cursor = (row - 1, col.min(self.line_len(row - 1)));
            return;
        }
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            None => {
                self.draft = Some(self.lines.clone());
                self.history.len() - 1
            }
            Some(0) => return,
            Some(p) => p - 1,
        };
        self.history_pos = Some(next);
        let text = self.history[next].clone();
        self.set_text(&text);
    }

    pub fn down(&mut self) {
        let (row, col) = self.cursor;
        if row + 1 < self.lines.len() {
            self.cursor = (row + 1, col.min(self.line_len(row + 1)));
            return;
        }
        let Some(pos) = self.history_pos else {
            return;
        };
        if pos + 1 < self.history.len() {
            self.history_pos = Some(pos + 1);
            let text = self.history[pos + 1].clone();
            self.set_text(&text);
        } else {
            self.history_pos = None;
            let draft = self.draft.take().unwrap_or_else(|| vec![String::new()]);
            self.lines = draft;
            let last = self.lines.len() - 1;
            self.cursor = (last, self.line_len(last));
        }
    }

    /// Visual rows the composer needs at `width`, clamped to 1..=MAX_ROWS.
    pub fn height(&self, width: u16) -> u16 {
        let rows = self.visual_rows(width).len();
        (rows.max(1) as u16).min(MAX_ROWS)
    }

    /// Word-agnostic wrap by display width; returns (logical line, start
    /// char, text) per visual row.
    fn visual_rows(&self, width: u16) -> Vec<(usize, usize, String)> {
        let width = width.max(1) as usize;
        let mut rows = Vec::new();
        for (li, line) in self.lines.iter().enumerate() {
            let mut current = String::new();
            let mut current_w = 0;
            let mut start = 0;
            for (count, c) in line.chars().enumerate() {
                let cw = c.width().unwrap_or(1);
                if current_w + cw > width {
                    rows.push((li, start, std::mem::take(&mut current)));
                    start = count;
                    current_w = 0;
                }
                current.push(c);
                current_w += cw;
            }
            rows.push((li, start, current));
        }
        rows
    }

    /// Render into the inner area of the composer dock and place the cursor.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        prompt: &'static str,
        continuation: &'static str,
        placeholder: &'static str,
        prompt_style: Style,
        text_style: Style,
        placeholder_style: Style,
        ghost: Option<&str>,
    ) {
        let prompt_width = UnicodeWidthStr::width(prompt).max(1) as u16;
        let text_width = area.width.saturating_sub(prompt_width);
        let rows = self.visual_rows(text_width);
        let (crow, ccol) = self.cursor;
        // Visual row containing the cursor.
        let mut cursor_visual = (0usize, 0usize);
        for (vi, (li, start, text)) in rows.iter().enumerate() {
            if *li == crow {
                let len = text.chars().count();
                if ccol >= *start && ccol <= start + len {
                    cursor_visual = (vi, ccol - start);
                    if ccol < start + len {
                        break;
                    }
                }
            }
        }
        let visible = area.height as usize;
        let first = cursor_visual.0.saturating_sub(visible.saturating_sub(1));
        let lines: Vec<Line> = rows
            .iter()
            .skip(first)
            .take(visible)
            .enumerate()
            .map(|(i, (_, _, text))| {
                let glyph = if i == 0 && first == 0 {
                    prompt
                } else {
                    continuation
                };
                let visual_index = first + i;
                let content = if self.is_empty() && i == 0 {
                    vec![Span::styled(placeholder, placeholder_style)]
                } else {
                    let mut spans = vec![Span::styled(text.clone(), text_style)];
                    if visual_index == cursor_visual.0
                        && ccol == self.line_len(crow)
                        && ghost.is_some()
                    {
                        spans.push(Span::styled(ghost.unwrap_or_default(), placeholder_style));
                    }
                    spans
                };
                let mut spans = vec![Span::styled(glyph, prompt_style)];
                spans.extend(content);
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
        let cursor_x = rows
            .get(cursor_visual.0)
            .map(|(_, _, text)| {
                text.chars()
                    .take(cursor_visual.1)
                    .map(|c| c.width().unwrap_or(1))
                    .sum::<usize>()
            })
            .unwrap_or(0) as u16;
        let cursor_y = (cursor_visual.0 - first) as u16;
        frame.set_cursor_position(Position::new(
            area.x + prompt_width + cursor_x.min(text_width),
            area.y + cursor_y,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_multiline() {
        let mut c = Composer::new(vec![]);
        c.insert_str("héllo");
        c.left();
        c.left();
        c.insert_char('X');
        assert_eq!(c.text(), "hélXlo");
        c.newline();
        c.insert_str("second");
        assert_eq!(c.text(), "hélX\nsecondlo");
        c.home();
        c.backspace();
        assert_eq!(c.text(), "hélXsecondlo");
        c.end();
        c.delete();
        assert_eq!(c.text(), "hélXsecondlo");
        assert_eq!(c.height(4), 3, "12 chars wrap to 3 rows of 4");
        assert_eq!(c.height(80), 1);
    }

    #[test]
    fn take_records_history_and_up_down_walk_it() {
        let mut c = Composer::new(vec!["first".into()]);
        c.insert_str("second");
        assert_eq!(c.take(), "second");
        assert!(c.is_empty());
        assert_eq!(c.history(), &["first".to_string(), "second".to_string()]);
        c.insert_str("draft");
        c.up();
        assert_eq!(c.text(), "second");
        c.up();
        assert_eq!(c.text(), "first");
        c.up();
        assert_eq!(c.text(), "first", "stops at the oldest entry");
        c.down();
        c.down();
        assert_eq!(
            c.text(),
            "draft",
            "returns to the draft after the newest entry"
        );
    }

    #[test]
    fn replaces_the_token_at_the_cursor() {
        let mut c = Composer::new(vec![]);
        c.insert_str("look @src/ma then");
        for _ in 0..5 {
            c.left();
        }
        assert_eq!(c.current_token(), "@src/ma");
        c.replace_current_token("@src/main.rs");
        assert_eq!(c.text(), "look @src/main.rs then");
    }

    #[test]
    fn replacing_a_token_consumes_its_suffix_after_the_cursor() {
        let mut composer = Composer::new(vec![]);
        composer.set_text("open @src/main.rs please");
        for _ in 0..8 {
            composer.left();
        }
        composer.replace_current_token("@src/lib.rs");
        assert_eq!(composer.text(), "open @src/lib.rs please");
    }

    #[test]
    fn take_deduplicates_consecutive_entries_and_caps_history() {
        let mut c = Composer::new((0..HISTORY_LIMIT).map(|i| format!("h{i}")).collect());
        c.insert_str("new");
        c.take();
        assert_eq!(c.history().len(), HISTORY_LIMIT);
        assert_eq!(c.history().last().map(String::as_str), Some("new"));
        c.insert_str("new");
        c.take();
        assert_eq!(
            c.history().len(),
            HISTORY_LIMIT,
            "identical consecutive entry not duplicated"
        );
    }
}
