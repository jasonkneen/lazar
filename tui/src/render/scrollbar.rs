//! A compact scrollbar shared by the transcript and completion surfaces.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::theme::Theme;

pub fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    total: usize,
    viewport: usize,
    offset: usize,
    following: bool,
    theme: &Theme,
) {
    let track_height = area.height as usize;
    if area.width < 2 || track_height == 0 || total <= viewport {
        return;
    }
    let thumb_height = (viewport * track_height / total).max(1).min(track_height);
    let travel = track_height.saturating_sub(thumb_height);
    let max_offset = total.saturating_sub(viewport);
    let thumb_start = offset
        .min(max_offset)
        .saturating_mul(travel)
        .checked_div(max_offset)
        .unwrap_or(0);
    let thumb_style = if following {
        theme.scrollbar_thumb
    } else {
        theme.accent
    };
    let thumb_glyph = theme.glyph("┃", "#");
    let track_glyph = theme.glyph("│", "|");
    let lines = (0..track_height)
        .map(|row| {
            let thumb = row >= thumb_start && row < thumb_start + thumb_height;
            Line::from(Span::styled(
                if thumb { thumb_glyph } else { track_glyph },
                if thumb {
                    thumb_style
                } else {
                    theme.scrollbar_track
                },
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn thumb_math_handles_tiny_viewports() {
        let total = 100usize;
        let viewport = 1usize;
        let track = 1usize;
        let thumb = (viewport * track / total).max(1).min(track);
        assert_eq!(thumb, 1);
    }
}
