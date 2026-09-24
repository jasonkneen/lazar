pub mod picker;

use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::theme::Theme;
use crate::render::transcript::wrap_line;
use picker::Picker;

/// Which picker is open, so the app knows what a chosen id means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Provider,
    Session,
    Theme,
    Weight,
    Icons,
    Slash,
    Path,
    Skill,
    AddDirScope,
}

#[derive(Debug, Clone)]
pub enum Overlay {
    Picker {
        kind: PickerKind,
        picker: Picker,
        /// Extra payload (e.g. the directory being granted).
        payload: Option<String>,
    },
    Help,
    EventFeed,
    Subagents,
}

/// Render a centred text panel (help, event feed).
pub fn render_text_panel(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    title: &str,
    lines: Vec<Line<'static>>,
    scroll: u16,
) {
    let max_width = area.width.saturating_sub(4).max(1);
    let natural_width = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .saturating_add(4) as u16;
    let width = natural_width.max(48).min(max_width).max(1);
    let content_width = width.saturating_sub(4).max(1);
    let visual_lines = lines
        .iter()
        .map(|line| wrap_line(line, content_width).len())
        .sum::<usize>();
    let max_height = area.height.saturating_sub(4).max(1);
    let height = (visual_lines as u16)
        .saturating_add(2)
        .max(5)
        .min(max_height);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(popup);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if theme.has_unicode_borders() {
            BorderType::Rounded
        } else {
            BorderType::Plain
        })
        .border_style(theme.accent)
        .title(Span::styled(format!(" {title} "), theme.title))
        .title_bottom(Span::styled(
            theme.glyph(" Esc close · ↑↓ scroll ", " Esc close | Up/Down scroll "),
            theme.dim,
        ));
    let inner = block.inner(popup).inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    frame.render_widget(block, popup);
    let visual_lines = lines
        .iter()
        .map(|line| wrap_line(line, inner.width).len())
        .sum::<usize>();
    let max_scroll = visual_lines.saturating_sub(inner.height as usize) as u16;
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll.min(max_scroll), 0));
    frame.render_widget(paragraph, inner);
}

pub fn help_lines(theme: &Theme) -> Vec<Line<'static>> {
    let input: Vec<(&str, &str)> = vec![
        (
            "Enter",
            "send the prompt (queues it while a turn is running)",
        ),
        ("Shift+Enter / Alt+Enter / Ctrl+J", "newline"),
        ("Ctrl+O", "model picker (type a custom id to use any model)"),
        ("Ctrl+P", "provider picker (from ~/lazar/providers.toml)"),
        (
            "Ctrl+S",
            "steer: cancel the running turn and send the composer now",
        ),
        ("Ctrl+X", "drop the queued message"),
        ("/ or @", "open command/skill or workspace-path completion"),
        ("$", "open the installed-skills picker"),
        ("Tab", "reopen completion for the current token"),
        ("Esc", "close an overlay / clear the composer"),
    ];
    let navigation: Vec<(&str, &str)> = vec![
        ("Mouse wheel", "scroll the message transcript"),
        ("PgUp / PgDn", "scroll one page"),
        (
            theme.glyph("Ctrl+↑ / Ctrl+↓", "Ctrl+Up / Ctrl+Down"),
            "scroll one line",
        ),
        ("Ctrl+Home / Ctrl+End", "jump to top / bottom"),
        ("Ctrl+L", "clear the transcript view"),
        ("Ctrl+T", "fold / unfold all tool outputs"),
        ("Ctrl+H", "show / hide the hook trace"),
        (
            "Ctrl+A",
            "debug/verbose view: hooks, internal kernel events, full detail",
        ),
        ("Ctrl+E", "raw kernel event feed"),
        ("Ctrl+G", "sub-agent panel"),
        (
            "←",
            "open the Agents screen (empty composer, or cursor at column 0)",
        ),
        (
            "↓ (empty composer)",
            "focus the footer agents panel; ↑↓ select, Enter open, Esc back",
        ),
        (
            "Agents screen",
            "↑↓ select (empty composer) · type to describe a new task, Enter starts it · Esc / → back to chat",
        ),
        (
            "Job detail",
            "a small chat: ↑↓/PgUp/PgDn scroll (empty composer) · type to steer/reply, Enter sends · Ctrl+T unfold tools · Esc back to list",
        ),
        ("Ctrl+C", "cancel the running turn, or quit when idle"),
        ("Ctrl+Q", "quit"),
    ];
    let commands: Vec<(&str, &str)> = vec![
        (
            "/provider [id]",
            "list or switch provider (restarts the kernel, keeps the session)",
        ),
        ("/model [id]", "list or switch model"),
        (
            "/theme [name]",
            "preview and persist auto, nerv, groknight, grokday, or tokyonight",
        ),
        ("/weight [thin | regular]", "set terminal text emphasis"),
        ("/icons [nerd | unicode | ascii]", "set the icon glyph set"),
        ("/session [new | list | resume <id>]", "session control"),
        (
            "/add-dir [path]",
            "grant sandbox write access to a directory",
        ),
        ("/clear", "clear the transcript view"),
        (
            "/debug",
            "toggle debug/verbose view (hooks, internal events, full detail)",
        ),
        (
            "Ctrl+D",
            "dump the transcript to workspace/lazartui-dump-<ts>.md",
        ),
        ("/help", "this panel"),
        (
            "/<skill>",
            "prefix the prompt with [skill:<name>] (from skills/INDEX.md)",
        ),
        (
            "$<skill>",
            "sent as typed; the kernel resolves it as a skill directive itself",
        ),
    ];
    let mut lines = Vec::new();
    for (section, rows) in [
        ("INPUT", input),
        ("NAVIGATION", navigation),
        ("COMMANDS", commands),
    ] {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(section, theme.title)));
        lines.extend(rows.into_iter().map(|(key, description)| {
            Line::from(vec![
                Span::styled(format!("  {key:<38}"), theme.accent),
                Span::raw(description.to_string()),
            ])
        }));
    }
    lines
}
