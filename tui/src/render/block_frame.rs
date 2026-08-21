//! Reusable framing for transcript blocks.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy)]
pub struct BlockFrame<'a> {
    surface: Style,
    left: u16,
    right: u16,
    accent: Option<(&'a str, Style)>,
}

impl<'a> BlockFrame<'a> {
    pub fn new(surface: Style) -> Self {
        Self {
            surface,
            left: 0,
            right: 0,
            accent: None,
        }
    }

    pub fn padding(mut self, left: u16, right: u16) -> Self {
        self.left = left;
        self.right = right;
        self
    }

    #[allow(dead_code)]
    pub fn accent(mut self, glyph: &'a str, style: Style) -> Self {
        self.accent = Some((glyph, style));
        self
    }

    pub fn render(self, lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
        let accent_width = self
            .accent
            .map(|(glyph, _)| glyph.width() as u16)
            .unwrap_or(0);
        let content_width = width
            .saturating_sub(self.left)
            .saturating_sub(self.right)
            .saturating_sub(accent_width) as usize;
        lines
            .into_iter()
            .map(|line| {
                let line = clip_line(line, content_width);
                let used = line.width();
                let mut spans = Vec::with_capacity(line.spans.len() + 4);
                if let Some((glyph, style)) = self.accent {
                    spans.push(Span::styled(glyph.to_string(), style.patch(self.surface)));
                }
                spans.push(Span::styled(" ".repeat(self.left as usize), self.surface));
                spans.extend(line.spans.into_iter().map(|span| {
                    Span::styled(span.content.into_owned(), self.surface.patch(span.style))
                }));
                spans.push(Span::styled(
                    " ".repeat(content_width.saturating_sub(used) + self.right as usize),
                    self.surface,
                ));
                Line::from(spans).style(self.surface)
            })
            .collect()
    }
}

fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut remaining = width;
    let mut clipped = Vec::new();
    for span in line.spans {
        if remaining == 0 {
            break;
        }
        let mut text = String::new();
        for c in span.content.chars() {
            let char_width = c.width().unwrap_or(0);
            if char_width > remaining {
                break;
            }
            text.push(c);
            remaining = remaining.saturating_sub(char_width);
        }
        if !text.is_empty() {
            clipped.push(Span::styled(text, span.style));
        }
    }
    Line::from(clipped).style(line.style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn padding_and_surface_cover_the_full_width() {
        let surface = Style::default().bg(Color::Blue);
        let lines = BlockFrame::new(surface)
            .padding(2, 2)
            .render(vec![Line::from("hello")], 12);
        assert_eq!(lines[0].width(), 12);
        assert_eq!(lines[0].spans.first().unwrap().content, "  ");
        assert!(lines[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(Color::Blue)));
    }

    #[test]
    fn accent_consumes_width_without_overflowing() {
        let lines = BlockFrame::new(Style::default())
            .padding(1, 1)
            .accent("┃", Style::default().fg(Color::Red))
            .render(vec![Line::from("content")], 10);
        assert_eq!(lines[0].width(), 10);
    }

    #[test]
    fn overlong_content_is_clipped_to_the_frame() {
        let lines = BlockFrame::new(Style::default())
            .padding(1, 1)
            .render(vec![Line::from("content that is too long")], 10);
        assert_eq!(lines[0].width(), 10);
        let text = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, " content  ");
    }
}
