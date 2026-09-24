//! The full-screen Agents view (Left key from the composer) and its
//! job-detail sub-screen (Enter on a row) — modelled on Claude Code's
//! background-sessions screen. Composition rules (grouping, age, ordering)
//! live in `crate::jobs`; the job-detail body reuses
//! `render::transcript::render_block` directly so it looks like the live
//! chat transcript.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::markdown::Highlighter;
use super::theme::Theme;
use super::transcript::{fit, render_block};
use crate::agents_state::JobDetailState;
use crate::jobs::{format_age, group_jobs, Job};
use crate::transcript::Block;

pub struct AgentsListInfo<'a> {
    pub model: &'a str,
    pub cwd: &'a str,
    pub jobs: &'a [Job],
    /// Index into the flattened Working/Attention/Completed row order.
    pub selected: usize,
    pub spinner: &'a str,
}

pub fn render_agents_list(frame: &mut Frame, area: Rect, theme: &Theme, info: &AgentsListInfo) {
    let (working, attention, completed) = group_jobs(info.jobs);
    let [title_area, sub_area, counts_area, _gap, list_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("lazar agents", theme.title))),
        title_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{}  ·  {}", info.model, info.cwd),
            theme.dim,
        ))),
        sub_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "{} working · {} need attention · {} completed",
                working.len(),
                attention.len(),
                completed.len()
            ),
            theme.dim,
        ))),
        counts_area,
    );

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut flat = 0usize;
    let sections: [(&str, &[&Job], &str, Style); 3] = [
        ("Working", &working, info.spinner, theme.accent),
        ("Needs attention", &attention, "✗", theme.fail),
        ("Completed", &completed, "✓", theme.ok),
    ];
    for (label, jobs, glyph, tone) in sections {
        if jobs.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(label, theme.dim)));
        for job in jobs {
            let selected = flat == info.selected;
            lines.push(job_row(job, theme, selected, list_area.width, glyph, tone));
            flat += 1;
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No sub-agents in the last 24h",
            theme.dim,
        )));
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "enter open · esc back · ↑↓ select",
            theme.dim,
        ))),
        hint_area,
    );
}

fn job_row(job: &Job, theme: &Theme, selected: bool, width: u16, glyph: &str, tone: Style) -> Line<'static> {
    let age = format_age(job.age_s);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let reserved = glyph.width() + 1 + age.width();
    let budget = (width as usize).saturating_sub(reserved).max(4);
    let task_budget = (budget * 3 / 5).max(6);
    let task = fit(&job.task, task_budget);
    let used = task.width();
    let summary_budget = budget.saturating_sub(used).saturating_sub(2);
    let summary = if summary_budget > 3 && !job.summary.is_empty() {
        fit(&job.summary, summary_budget)
    } else {
        String::new()
    };

    let mut spans = vec![
        Span::styled(format!("{glyph} "), tone),
        Span::styled(task, bold),
    ];
    if !summary.is_empty() {
        spans.push(Span::styled(format!("  {summary}"), theme.dim));
    }
    let content_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad = (width as usize).saturating_sub(content_w + age.width());
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    spans.push(Span::styled(age, theme.dim));

    let line = Line::from(spans);
    if selected {
        line.style(theme.selection)
    } else {
        line
    }
}

/// A `Block::Tool` with its fold flag overridden — `tools_unfolded` toggles
/// every tool in the timeline at once, mirroring the live transcript's
/// Ctrl+T behaviour (`Transcript::toggle_tool_fold`).
fn with_fold(block: &Block, unfolded: bool) -> Block {
    match block {
        Block::Tool {
            id,
            command,
            output,
            exit_code,
            ..
        } => Block::Tool {
            id: id.clone(),
            command: command.clone(),
            output: output.clone(),
            exit_code: *exit_code,
            folded: !unfolded,
        },
        other => other.clone(),
    }
}

pub fn render_job_detail(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    hl: &Highlighter,
    detail: &JobDetailState,
    spinner: &str,
) {
    let glyph = match detail.status.as_str() {
        "running" | "starting" => spinner,
        "ok" => "✓",
        _ => "✗",
    };
    let [header_area, _gap, body_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(detail.task.clone(), theme.title),
            Span::raw("  "),
            Span::styled(glyph.to_string(), theme.accent),
            Span::raw("  "),
            Span::styled(format!("{}s", detail.elapsed_s), theme.dim),
        ])),
        header_area,
    );

    if let Some(err) = &detail.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!("error: {err}"), theme.fail))),
            body_area,
        );
    } else if detail.loading && detail.blocks.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("loading…", theme.dim))),
            body_area,
        );
    } else {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for block in &detail.blocks {
            let block = with_fold(block, detail.tools_unfolded);
            lines.extend(render_block(&block, body_area.width, theme, hl, false));
        }
        if lines.first().is_some_and(|line| line.width() == 0) {
            lines.remove(0);
        }
        let max_scroll = lines.len().saturating_sub(body_area.height as usize) as u16;
        let scroll = detail.scroll.min(max_scroll);
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body_area);
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↑↓ pgup/pgdn scroll · enter/ctrl+t unfold tools · esc back",
            theme.dim,
        ))),
        hint_area,
    );
}
