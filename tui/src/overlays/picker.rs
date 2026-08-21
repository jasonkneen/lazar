//! A generic fuzzy list picker rendered as a centred popup. Used for models,
//! providers, sessions, skills, and directory scopes.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::render::scrollbar::render_scrollbar;
use crate::render::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickItem {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub tag: Option<String>,
    /// Greyed out but still selectable (e.g. provider without a key).
    pub disabled: bool,
}

impl PickItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: detail.into(),
            tag: None,
            disabled: false,
        }
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    Pending,
    Cancelled,
    Chosen(String),
}

#[derive(Debug, Clone)]
pub struct Picker {
    pub title: String,
    items: Vec<PickItem>,
    query: String,
    selected: usize,
    hovered: Option<usize>,
    hit_rows: Vec<(Rect, usize)>,
    hit_area: Option<Rect>,
    /// Enter on a query that matches nothing submits the query itself
    /// (free-form model ids).
    pub allow_free_text: bool,
}

impl Picker {
    pub fn new(title: impl Into<String>, items: Vec<PickItem>) -> Self {
        Self {
            title: title.into(),
            items,
            query: String::new(),
            selected: 0,
            hovered: None,
            hit_rows: Vec::new(),
            hit_area: None,
            allow_free_text: false,
        }
    }

    pub fn with_free_text(mut self) -> Self {
        self.allow_free_text = true;
        self
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.selected = 0;
        self.hovered = None;
    }

    pub fn select_id(&mut self, id: &str) {
        if let Some(pos) = self.filtered().iter().position(|(_, item)| item.id == id) {
            self.selected = pos;
        }
    }

    /// Case-insensitive subsequence match on label, id and detail.
    pub fn filtered(&self) -> Vec<(usize, &PickItem)> {
        let q: Vec<char> = self.query.to_lowercase().chars().collect();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                q.is_empty()
                    || [&item.label, &item.id, &item.detail]
                        .iter()
                        .any(|s| subsequence(&q, s))
            })
            .collect()
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> PickerOutcome {
        match key.code {
            KeyCode::Esc => return PickerOutcome::Cancelled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return PickerOutcome::Cancelled
            }
            KeyCode::Enter => {
                let filtered = self.filtered();
                if let Some((_, item)) = filtered.get(self.selected) {
                    return PickerOutcome::Chosen(item.id.clone());
                }
                if self.allow_free_text && !self.query.trim().is_empty() {
                    return PickerOutcome::Chosen(self.query.trim().to_string());
                }
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                let n = self.filtered().len();
                if n > 0 {
                    self.selected = (self.selected + 1).min(n - 1);
                }
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => {
                let n = self.filtered().len();
                if n > 0 {
                    self.selected = (self.selected + 10).min(n - 1);
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        PickerOutcome::Pending
    }

    pub fn selected_id(&self) -> Option<String> {
        self.filtered()
            .get(self.selected)
            .map(|(_, item)| item.id.clone())
    }

    pub fn contains(&self, column: u16, row: u16) -> bool {
        self.hit_area
            .is_some_and(|area| area.contains((column, row).into()))
    }

    pub fn handle_mouse(&mut self, event: MouseEvent) -> PickerOutcome {
        let inside = self.contains(event.column, event.row);
        if !inside {
            if matches!(event.kind, MouseEventKind::Moved) {
                self.hovered = None;
            }
            return PickerOutcome::Pending;
        }
        match event.kind {
            MouseEventKind::Moved => {
                self.hovered = self
                    .hit_rows
                    .iter()
                    .find(|(area, _)| area.contains((event.column, event.row).into()))
                    .map(|(_, index)| *index);
            }
            MouseEventKind::ScrollUp => self.selected = self.selected.saturating_sub(1),
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 1).min(self.filtered().len().saturating_sub(1));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self
                    .hit_rows
                    .iter()
                    .find(|(area, _)| area.contains((event.column, event.row).into()))
                    .map(|(_, index)| *index)
                {
                    self.selected = index;
                    if let Some(id) = self.selected_id() {
                        return PickerOutcome::Chosen(id);
                    }
                }
            }
            _ => {}
        }
        PickerOutcome::Pending
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let filtered_len = self.filtered().len();
        let height = (filtered_len as u16 + 4).clamp(6, area.height.saturating_sub(2).max(6));
        let width = area.width.clamp(20, 90).min(area.width);
        let [popup] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(popup);
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.accent)
            .style(theme.canvas)
            .title(Span::styled(format!(" {} ", self.title), theme.title));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let [query_area, list_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(theme.icon("󰅂 ", "› ", "> "), theme.accent),
                Span::raw(self.query.clone()),
                Span::styled(theme.glyph("▏", "|"), theme.dim),
            ])),
            query_area,
        );
        self.render_rows(frame, list_area, theme);
        self.hit_area = Some(popup);
        let hint = if self.allow_free_text {
            theme.glyph(
                "↑↓ move · Enter choose (or type a custom id) · Esc close",
                "Up/Down move | Enter choose (or type a custom id) | Esc close",
            )
        } else {
            theme.glyph(
                "↑↓ move · Enter choose · Esc close",
                "Up/Down move | Enter choose | Esc close",
            )
        };
        frame.render_widget(Paragraph::new(Span::styled(hint, theme.dim)), hint_area);
    }

    /// Compact completion menu anchored directly above the composer dock.
    pub fn render_completion(&mut self, frame: &mut Frame, anchor: Rect, theme: &Theme) {
        let filtered_len = self.filtered().len();
        let height = (filtered_len.min(8) as u16 + 2).max(4).min(anchor.y.max(1));
        let width = anchor.width.saturating_sub(2).clamp(1, 90);
        let popup = Rect {
            x: anchor.x.saturating_add(1),
            y: anchor.y.saturating_sub(height),
            width,
            height,
        };
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.accent)
            .style(theme.canvas)
            .title(Span::styled(format!(" {} ", self.title), theme.title))
            .title_bottom(Span::styled(
                theme.glyph(
                    " ↑↓ select · Enter insert · Esc close ",
                    " Up/Down select | Enter insert | Esc close ",
                ),
                theme.dim,
            ));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        self.render_rows(frame, inner, theme);
        self.hit_area = Some(popup);
    }

    fn render_rows(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.hit_rows.clear();
        self.hit_area = Some(area);
        if area.width == 0 || area.height == 0 {
            return;
        }
        let items: Vec<PickItem> = self
            .filtered()
            .into_iter()
            .map(|(_, item)| item.clone())
            .collect();
        if items.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled("  No matches", theme.dim)),
                area,
            );
            return;
        }
        self.selected = self.selected.min(items.len() - 1);
        let viewport = area.height as usize;
        let max_start = items.len().saturating_sub(viewport);
        let start = self.selected.saturating_sub(viewport / 2).min(max_start);
        let needs_scrollbar = items.len() > viewport;
        let row_width = area.width.saturating_sub(u16::from(needs_scrollbar));
        let content_width = row_width as usize;
        let max_label = items
            .iter()
            .map(|item| tagged_label(item).width())
            .max()
            .unwrap_or(0)
            .min(content_width.saturating_mul(3) / 5)
            .min(40);
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .skip(start)
            .take(viewport)
            .map(|(index, item)| {
                let selected = index == self.selected;
                let hovered = self.hovered == Some(index) && !selected;
                let row_style = if selected {
                    theme.emphasize(theme.selection)
                } else if hovered {
                    theme.hover
                } else {
                    theme.canvas
                };
                let prefix = if selected {
                    theme.glyph("▌ ", "> ")
                } else {
                    "  "
                };
                let ellipsis = theme.glyph("…", "...");
                let label = truncate_width(&tagged_label(item), max_label, ellipsis);
                let label_width = label.width();
                let detail_budget = content_width.saturating_sub(2 + max_label + 2);
                let detail = truncate_width(&item.detail, detail_budget, ellipsis);
                let item_style = if item.disabled {
                    theme.dim.patch(row_style)
                } else {
                    row_style
                };
                self.hit_rows.push((
                    Rect::new(area.x, area.y + (index - start) as u16, row_width, 1),
                    index,
                ));
                Line::from(vec![
                    Span::styled(prefix, row_style),
                    Span::styled(label, item_style),
                    Span::styled(
                        " ".repeat(max_label.saturating_sub(label_width) + 2),
                        row_style,
                    ),
                    Span::styled(detail, theme.dim.patch(row_style)),
                ])
                .style(row_style)
            })
            .collect();
        frame.render_widget(
            Paragraph::new(lines),
            Rect {
                width: row_width,
                ..area
            },
        );
        if needs_scrollbar {
            render_scrollbar(frame, area, items.len(), viewport, start, false, theme);
        }
    }
}

fn tagged_label(item: &PickItem) -> String {
    match &item.tag {
        Some(tag) => format!("{} [{}]", item.label, tag),
        None => item.label.clone(),
    }
}

fn truncate_width(text: &str, max_width: usize, ellipsis: &str) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let marker = ellipsis
        .chars()
        .scan(0usize, |used, c| {
            let next = *used + c.width().unwrap_or(0);
            (next <= max_width).then(|| {
                *used = next;
                c
            })
        })
        .collect::<String>();
    let mut out = String::new();
    let budget = max_width.saturating_sub(marker.width());
    for c in text.chars() {
        if format!("{out}{c}").width() > budget {
            break;
        }
        out.push(c);
    }
    out.push_str(&marker);
    out
}

fn subsequence(query: &[char], haystack: &str) -> bool {
    let mut qi = 0;
    for c in haystack.to_lowercase().chars() {
        if qi < query.len() && c == query[qi] {
            qi += 1;
        }
    }
    qi == query.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> Picker {
        Picker::new(
            "Model",
            vec![
                PickItem::new("claude-sonnet-4-6", "claude-sonnet-4-6", "Anthropic"),
                PickItem::new("MiniMax-M3", "MiniMax-M3", "MiniMax"),
                PickItem::new("kimi-k3", "kimi-k3", "Kimi"),
            ],
        )
    }

    #[test]
    fn filters_by_subsequence_and_chooses_on_enter() {
        let mut p = picker();
        for c in "mm3".chars() {
            p.handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(p.filtered().len(), 1);
        assert_eq!(
            p.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            PickerOutcome::Chosen("MiniMax-M3".into())
        );
    }

    #[test]
    fn free_text_and_cancel() {
        let mut p = picker().with_free_text();
        for c in "custom/model".chars() {
            p.handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(p.filtered().is_empty());
        assert_eq!(
            p.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            PickerOutcome::Chosen("custom/model".into())
        );
        assert_eq!(
            picker().handle_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            PickerOutcome::Cancelled
        );
    }

    #[test]
    fn mouse_hover_click_and_wheel_use_rendered_hit_rows() {
        let mut p = picker();
        p.hit_area = Some(Rect::new(10, 5, 40, 3));
        p.hit_rows = vec![
            (Rect::new(10, 5, 40, 1), 0),
            (Rect::new(10, 6, 40, 1), 1),
            (Rect::new(10, 7, 40, 1), 2),
        ];
        p.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 12,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(p.hovered, Some(1));
        assert_eq!(
            p.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 12,
                row: 6,
                modifiers: KeyModifiers::NONE,
            }),
            PickerOutcome::Chosen("MiniMax-M3".into())
        );
    }

    #[test]
    fn navigation_clamps() {
        let mut p = picker();
        p.handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        for _ in 0..10 {
            p.handle_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert_eq!(
            p.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            PickerOutcome::Chosen("kimi-k3".into())
        );
    }
}
