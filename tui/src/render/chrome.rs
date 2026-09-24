//! Header and footer bars.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::Theme;
use crate::status::{AgentRow, Segment, Tone};

pub const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
pub const SPINNER_ASCII: [&str; 4] = ["|", "/", "-", "\\"];

pub struct HeaderInfo<'a> {
    pub session_id: &'a str,
    pub status: &'a str,
    pub turn_running: bool,
    pub spinner_frame: usize,
    pub sandbox_off: bool,
}

pub fn render_header(frame: &mut Frame, area: Rect, theme: &Theme, info: &HeaderInfo) {
    let spinner = if info.turn_running {
        if theme.has_unicode_borders() {
            SPINNER[info.spinner_frame % SPINNER.len()]
        } else {
            SPINNER_ASCII[info.spinner_frame % SPINNER_ASCII.len()]
        }
    } else if info.status == "error" {
        theme.icon("󰅖", "×", "x")
    } else if info.status == "ready" || info.status == "end_turn" {
        theme.icon("󰄬", "ok", "ok")
    } else {
        theme.glyph("○", "*")
    };
    let mut spans = vec![
        Span::styled(
            format!(" {} lazar ", theme.icon("󰞷", "", "")),
            theme.title.patch(theme.header_bg),
        ),
        Span::styled(format!(" {spinner} "), theme.accent.patch(theme.header_bg)),
        Span::styled(
            info.status.to_string(),
            Style::default().patch(theme.header_bg),
        ),
        Span::styled(
            format!("  {} {} ", theme.icon("󰦛", "sess", "sess"), info.session_id),
            theme.dim.patch(theme.header_bg),
        ),
    ];
    if info.sandbox_off {
        spans.push(Span::styled(
            " SANDBOX OFF ",
            theme.warn.patch(theme.header_bg),
        ));
    }
    let [bar, rule] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme.header_bg), bar);
    // A top-aligned eighth block in the bar's colour: the next row reads as
    // space, but the header gets a thin underline.
    if rule.height > 0 {
        // A hairline overline in the quiet border colour: visible as an
        // underline, lighter than the header bar itself.
        let underline = theme.composer_border;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                theme.glyph("‾", " ").repeat(rule.width as usize),
                underline,
            ))),
            rule,
        );
    }
}

/// A Claude-Code-style two-line status area under the composer, plus (when
/// sub-agents exist) an agent panel below it. The line/row composition rules
/// (what shows, what's omitted, which tone) live in `crate::status`, tested
/// without a terminal; this only turns `Segment`/`AgentRow` into styled,
/// width-clipped `Line`s.
pub struct StatusInfo {
    pub line1: Vec<Segment>,
    pub line2: Vec<Segment>,
    pub agent_rows: Vec<AgentRow>,
    pub agents_more: usize,
    /// Index into `agent_rows` the footer-panel focus (Down from an empty
    /// composer) has selected, if any — highlighted with `theme.selection`.
    pub focused_agent_row: Option<usize>,
}

impl StatusInfo {
    /// Total rows this status area needs, so callers can size the layout
    /// slot before rendering into it.
    pub fn height(&self) -> u16 {
        let mut rows = 1 + usize::from(!self.line2.is_empty());
        if !self.agent_rows.is_empty() {
            rows += self.agent_rows.len() + usize::from(self.agents_more > 0);
        }
        rows as u16
    }
}

fn tone_style(theme: &Theme, tone: Tone) -> Style {
    match tone {
        Tone::Normal => theme.footer,
        Tone::Dim => theme.dim,
        Tone::Accent => theme.accent,
        Tone::Ok => theme.ok,
        Tone::Warn => theme.warn,
        Tone::Fail => theme.fail,
    }
}

fn segments_line(theme: &Theme, segments: &[Segment], separator: &str) -> Line<'static> {
    let mut spans = Vec::with_capacity(segments.len() * 2);
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(separator.to_string(), theme.dim));
        }
        spans.push(Span::styled(segment.text.clone(), tone_style(theme, segment.tone)));
    }
    Line::from(spans)
}

/// Clip a line to `max_width` columns without wrapping, appending `…` when
/// anything was cut. Colours per span are preserved up to the cut point.
fn clip_line(line: Line<'static>, max_width: usize) -> Line<'static> {
    let total: usize = line.spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
    if total <= max_width || max_width == 0 {
        return line;
    }
    let line_style = line.style;
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = Vec::new();
    'spans: for span in &line.spans {
        let mut text = String::new();
        let mut cut_short = false;
        for ch in span.content.chars() {
            let w = ch.width().unwrap_or(1);
            if used + w > budget {
                cut_short = true;
                break;
            }
            text.push(ch);
            used += w;
        }
        if !text.is_empty() {
            out.push(Span::styled(text, span.style));
        }
        if cut_short {
            break 'spans;
        }
    }
    out.push(Span::raw("…"));
    Line::from(out).style(line_style)
}

/// The "✻ Thinking… (12s · …)" line above the composer while a turn runs.
/// A no-op on a zero-height area (idle: the caller reserves no row).
pub fn render_activity(frame: &mut Frame, area: Rect, theme: &Theme, segments: &[Segment]) {
    if area.height == 0 || segments.is_empty() {
        return;
    }
    let width = area.width as usize;
    let line = clip_line(segments_line(theme, segments, " "), width);
    frame.render_widget(Paragraph::new(line), area);
}

pub fn render_status(frame: &mut Frame, area: Rect, theme: &Theme, info: &StatusInfo) {
    let width = area.width as usize;
    let dot = theme.glyph(" · ", " | ");
    let mut lines = vec![clip_line(segments_line(theme, &info.line1, "  "), width)];
    if !info.line2.is_empty() {
        lines.push(clip_line(segments_line(theme, &info.line2, dot), width));
    }
    for (i, row) in info.agent_rows.iter().enumerate() {
        let style = tone_style(theme, row.tone);
        let mut line = Line::from(vec![
            Span::styled(format!("{} ", row.glyph), style),
            Span::styled(row.text.clone(), style),
        ]);
        if info.focused_agent_row == Some(i) {
            line = line.style(theme.selection);
        }
        lines.push(clip_line(line, width));
    }
    if info.agents_more > 0 {
        let line = Line::from(Span::styled(format!("  +{} more", info.agents_more), theme.dim));
        lines.push(clip_line(line, width));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Segment;

    fn plain_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn status_height_counts_line2_and_agent_rows_and_the_more_row() {
        let quiet = StatusInfo {
            line1: vec![Segment::plain("x")],
            line2: vec![],
            agent_rows: vec![],
            agents_more: 0,
            focused_agent_row: None,
        };
        assert_eq!(quiet.height(), 1);

        let with_line2 = StatusInfo {
            line1: vec![Segment::plain("x")],
            line2: vec![Segment::plain("y")],
            agent_rows: vec![],
            agents_more: 0,
            focused_agent_row: None,
        };
        assert_eq!(with_line2.height(), 2);

        let with_agents = StatusInfo {
            line1: vec![Segment::plain("x")],
            line2: vec![Segment::plain("y")],
            agent_rows: vec![
                AgentRow { glyph: "✓".into(), tone: Tone::Ok, text: "a".into() },
                AgentRow { glyph: "✓".into(), tone: Tone::Ok, text: "b".into() },
            ],
            agents_more: 3,
            focused_agent_row: None,
        };
        // line1 + line2 + 2 rows + "+N more" = 5.
        assert_eq!(with_agents.height(), 5);
    }

    #[test]
    fn clip_line_truncates_with_an_ellipsis_and_leaves_short_lines_alone() {
        let short = Line::from(Span::raw("hi"));
        assert_eq!(clip_line(short.clone(), 10), short);

        let long = Line::from(Span::raw("hello world this is long"));
        let clipped = clip_line(long, 10);
        assert_eq!(UnicodeWidthStr::width(plain_text(&clipped).as_str()), 10);
        assert!(plain_text(&clipped).ends_with('…'));
    }
}
