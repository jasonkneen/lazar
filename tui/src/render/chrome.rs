//! Header and footer bars.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::Theme;

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
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme.header_bg),
        area,
    );
}

pub struct FooterInfo<'a> {
    pub cwd: &'a str,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub scroll: &'a str,
    pub queued: bool,
    pub notice: &'a str,
    pub turn_elapsed: Option<u64>,
}

pub fn render_footer(frame: &mut Frame, area: Rect, theme: &Theme, info: &FooterInfo) {
    let left = if !info.notice.is_empty() {
        format!("  {}", info.notice)
    } else {
        format!("  {}", info.cwd)
    };
    let mut low_priority = vec![
        format!("in {}", info.input_tokens),
        format!("out {}", info.output_tokens),
    ];
    let mut high_priority = Vec::new();
    if info.queued {
        high_priority.push("queued (^X drop)".into());
    }
    if let Some(seconds) = info.turn_elapsed.filter(|seconds| *seconds > 0) {
        high_priority.push(if seconds < 60 {
            format!("{seconds}s")
        } else {
            format!("{}m{:02}s", seconds / 60, seconds % 60)
        });
    }
    high_priority.push(info.scroll.to_string());
    let width = area.width as usize;
    let separator = theme.glyph(" · ", " | ");
    let mut right = low_priority
        .iter()
        .chain(&high_priority)
        .cloned()
        .collect::<Vec<_>>()
        .join(separator);
    while UnicodeWidthStr::width(right.as_str()) + 8 > width && !low_priority.is_empty() {
        low_priority.remove(0);
        right = low_priority
            .iter()
            .chain(&high_priority)
            .cloned()
            .collect::<Vec<_>>()
            .join(separator);
    }
    right = truncate_width(&right, width.saturating_sub(1));
    let right_w = UnicodeWidthStr::width(right.as_str());
    let left_max = width.saturating_sub(right_w + 1);
    let left = truncate_width(&left, left_max);
    let left_w = UnicodeWidthStr::width(left.as_str());
    let pad = width.saturating_sub(left_w + right_w);
    let line = Line::from(vec![
        Span::styled(
            left,
            if info.notice.is_empty() {
                theme.footer
            } else {
                theme.warn
            },
        ),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, theme.footer),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate_width(text: &str, max_width: usize) -> String {
    let mut width = 0;
    text.chars()
        .take_while(|c| {
            let next = width + c.width().unwrap_or(1);
            if next > max_width {
                false
            } else {
                width = next;
                true
            }
        })
        .collect()
}
