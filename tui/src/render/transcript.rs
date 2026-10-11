//! Transcript blocks to wrapped, styled lines, with a per-block cache so a
//! long conversation does not re-parse every frame.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::block_frame::BlockFrame;
use super::markdown::{render_markdown, Highlighter};
use super::theme::Theme;
use crate::transcript::{BannerKind, Block, SourceKind, Transcript, VerifyStatus};

/// Wrap one line to `width` columns, breaking at spaces where possible and
/// hard-breaking otherwise. Styles are preserved per span.
pub fn wrap_line(line: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_w = 0usize;
    let mut pending_space = false;

    for span in &line.spans {
        let style = span.style;
        let mut word = String::new();
        let chars: Vec<char> = span.content.chars().collect();
        let mut i = 0;
        while i <= chars.len() {
            let at_end = i == chars.len();
            let c = if at_end { ' ' } else { chars[i] };
            if at_end || c == ' ' {
                if !word.is_empty() {
                    let ww = UnicodeWidthStr::width(word.as_str());
                    let separator = usize::from(pending_space && current_w > 0);
                    if current_w + separator + ww > width && current_w > 0 {
                        out.push(Line::from(std::mem::take(&mut current)));
                        current_w = 0;
                    }
                    if pending_space && current_w > 0 {
                        current.push(Span::styled(" ".to_string(), style));
                        current_w += 1;
                    }
                    if ww > width {
                        let mut chunk = String::new();
                        let mut chunk_w = 0;
                        for ch in word.chars() {
                            let cw = UnicodeWidthStr::width(ch.to_string().as_str());
                            if current_w + chunk_w + cw > width {
                                if !chunk.is_empty() {
                                    current.push(Span::styled(std::mem::take(&mut chunk), style));
                                }
                                out.push(Line::from(std::mem::take(&mut current)));
                                current_w = 0;
                                chunk_w = 0;
                            }
                            chunk.push(ch);
                            chunk_w += cw;
                        }
                        if !chunk.is_empty() {
                            current.push(Span::styled(chunk, style));
                            current_w += chunk_w;
                        }
                    } else {
                        current.push(Span::styled(std::mem::take(&mut word), style));
                        current_w += ww;
                    }
                    pending_space = false;
                }
                word.clear();
                if !at_end {
                    pending_space = true;
                }
            } else {
                word.push(c);
            }
            i += 1;
        }
    }
    if !current.is_empty() || out.is_empty() {
        out.push(Line::from(current));
    }
    out
}

#[derive(Default)]
pub struct RenderCache {
    entries: Vec<CacheEntry>,
}

struct CacheEntry {
    key: u64,
    width: u16,
    theme_key: u64,
    debug: bool,
    lines: Vec<Line<'static>>,
}

impl RenderCache {
    /// All visual lines for the transcript at `width`.
    pub fn lines(
        &mut self,
        transcript: &Transcript,
        width: u16,
        theme: &Theme,
        hl: &Highlighter,
        spinner_frame: usize,
    ) -> Vec<Line<'static>> {
        self.entries.truncate(transcript.blocks.len());
        let plan = tool_fold_plan(&transcript.blocks, transcript.hooks_visible(), false);
        let mut out = Vec::new();
        for (i, block) in transcript.blocks.iter().enumerate() {
            // Entries are indexed by block; a folded block still needs its
            // slot (with a key that can never match) so later indices line up.
            if self.entries.len() <= i {
                self.entries.push(CacheEntry { key: u64::MAX, width: 0, theme_key: 0, debug: false, lines: Vec::new() });
            }
            match &plan[i] {
                Fold::Show => {}
                Fold::Hide => continue,
                Fold::Summary(members) => {
                    out.extend(tool_summary_lines(&transcript.blocks, members, width, theme));
                    self.entries[i].key = u64::MAX;
                    continue;
                }
            }
            if matches!(block, Block::Hook { .. }) && !transcript.hooks_visible() {
                if i < self.entries.len() {
                    self.entries[i].lines.clear();
                }
                continue;
            }
            // A per-block revision counter (bumped by Transcript on mutation)
            // instead of hashing `format!("{block:?}")` every frame: cheap to
            // compare, and doesn't walk tool output / markdown text just to
            // decide whether it changed.
            let key = transcript.revision(i);
            let theme_key = theme.cache_key();
            let debug = transcript.debug;
            let fresh = self
                .entries
                .get(i)
                .map(|e| {
                    e.key != key || e.width != width || e.theme_key != theme_key || e.debug != debug
                })
                .unwrap_or(true);
            if fresh {
                let rendered = render_block(block, width, theme, hl, debug, spinner_frame);
                let entry = CacheEntry {
                    key,
                    width,
                    theme_key,
                    debug,
                    lines: rendered,
                };
                if i < self.entries.len() {
                    self.entries[i] = entry;
                } else {
                    self.entries.push(entry);
                }
            }
            out.extend(self.entries[i].lines.iter().cloned());
        }
        // Every block opens with a spacer row; the first one would sit under
        // the header's own underline row, so drop it.
        if out.first().is_some_and(|line| line.width() == 0) {
            out.remove(0);
        }
        out
    }
}

/// How one block takes part in cascading tool folding.
#[derive(Debug, Clone, PartialEq)]
enum Fold {
    /// Render the block normally.
    Show,
    /// Folded into a summary drawn at an earlier index.
    Hide,
    /// Draw one summary row for these tool blocks.
    Summary(Vec<usize>),
}

fn tool_program(command: &str) -> &str {
    let trimmed = command.trim_start();
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    if end == 0 { "(command)" } else { &trimmed[..end] }
}

fn tool_is_live(block: &Block) -> bool {
    matches!(block, Block::Tool { running: true, .. } | Block::Tool { output: None, .. })
}

/// Cascading fold: a finished run of consecutive tool calls (something came
/// after it) collapses to one row; the live run at the tail collapses each
/// stretch of same-program calls, keeping the running call on its own row.
/// Unfolding (^T) shows every call.
fn tool_fold_plan(blocks: &[Block], hooks_visible: bool, tools_unfolded: bool) -> Vec<Fold> {
    let mut plan = vec![Fold::Show; blocks.len()];
    let skippable = |b: &Block| matches!(b, Block::Hook { .. }) && !hooks_visible;
    let mut i = 0;
    while i < blocks.len() {
        if !matches!(blocks[i], Block::Tool { .. }) {
            i += 1;
            continue;
        }
        let mut run = Vec::new();
        let mut j = i;
        while j < blocks.len() && (matches!(blocks[j], Block::Tool { .. }) || skippable(&blocks[j])) {
            if matches!(blocks[j], Block::Tool { .. }) {
                run.push(j);
            }
            j += 1;
        }
        // Collapse any finished run of 2+ tools into one summary row.
        // Only the live run (if any) escapes fold; it stays as individual rows.
        let finished = j < blocks.len() && !run.iter().any(|&k| tool_is_live(&blocks[k]));
        if finished && run.len() > 1 && !tools_unfolded {
            plan[run[0]] = Fold::Summary(run.clone());
            for &k in &run[1..] {
                plan[k] = Fold::Hide;
            }
        } else {
            // Live tail: keep the running call visible, but fold finished same-program stretches.
            let mut s = 0;
            while s < run.len() {
                let program = match &blocks[run[s]] { Block::Tool { command, .. } => tool_program(command), _ => "" };
                let mut e = s;
                while e + 1 < run.len()
                    && !tool_is_live(&blocks[run[e + 1]])
                    && !tool_is_live(&blocks[run[s]])
                    && matches!(&blocks[run[e + 1]], Block::Tool { command, .. } if tool_program(command) == program)
                {
                    e += 1;
                }
                if e > s && !tools_unfolded {
                    plan[run[s]] = Fold::Summary(run[s..=e].to_vec());
                    for &k in &run[s + 1..=e] {
                        plan[k] = Fold::Hide;
                    }
                }
                s = e + 1;
            }
        }
        i = j;
    }
    plan
}

/// One row standing for several tool calls: `grep ×4` for a same-program
/// stretch, `12 tool calls · grep ×4 · sed ×6` for a whole finished run.
fn tool_summary_lines(blocks: &[Block], members: &[usize], width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let mut groups: Vec<(String, usize)> = Vec::new();
    let mut failed = 0;
    for &k in members {
        if let Block::Tool { command, exit_code, .. } = &blocks[k] {
            let program = tool_program(command).to_string();
            match groups.last_mut() {
                Some((p, n)) if *p == program => *n += 1,
                _ => groups.push((program, 1)),
            }
            if exit_code.is_some_and(|c| c != 0) {
                failed += 1;
            }
        }
    }
    let icon = theme.icon("󰞷 ", "◆ ", "$ ");
    let status = if failed > 0 {
        Span::styled(format!(" ✗ {failed} failed"), theme.fail)
    } else {
        Span::styled(" ✓ ok".to_string(), theme.ok)
    };
    let mut spans = vec![Span::styled(icon.to_string(), theme.accent)];
    if groups.len() == 1 {
        let (program, n) = &groups[0];
        spans.push(Span::styled(program.clone(), theme.tool_cmd));
        spans.push(Span::styled(format!(" ×{n}"), theme.tool_args));
    } else {
        let parts = groups
            .iter()
            .map(|(p, n)| if *n > 1 { format!("{p} ×{n}") } else { p.clone() })
            .collect::<Vec<_>>()
            .join(" · ");
        spans.push(Span::styled(format!("{} tool calls", members.len()), theme.tool_cmd));
        let budget = (width as usize).saturating_sub(icon.width() + 16 + status.content.width());
        spans.push(Span::styled(fit(&format!(" · {parts}"), budget), theme.tool_args));
    }
    spans.push(status);
    let hint = format!("  {} {} calls folded (^T to unfold)", theme.icon("󰅂", "▸", ">"), members.len());
    vec![
        Line::default(),
        Line::from(spans),
        Line::from(Span::styled(fit(&hint, width as usize), theme.dim)),
    ]
}

/// Split a line's spans at display column `cols`.
fn split_at_column(line: Line<'static>, cols: usize) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let mut prefix = Vec::new();
    let mut body = Vec::new();
    let mut used = 0;
    for span in line.spans {
        if used >= cols {
            body.push(span);
            continue;
        }
        let mut head = String::new();
        let mut tail = String::new();
        for ch in span.content.chars() {
            if used < cols {
                used += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                head.push(ch);
            } else {
                tail.push(ch);
            }
        }
        if !head.is_empty() {
            prefix.push(Span::styled(head, span.style));
        }
        if !tail.is_empty() {
            body.push(Span::styled(tail, span.style));
        }
    }
    (prefix, body)
}

/// Columns a wrapped line's continuation rows should indent: leading spaces
/// plus a list or quote marker (`• `, `- `, `* `, `12. `, `│ `).
fn hanging_indent(text: &str) -> usize {
    let spaces = text.len() - text.trim_start_matches(' ').len();
    let rest = &text[spaces..];
    let marker = ["• ", "- ", "* ", "│ ", "▎ "]
        .iter()
        .find(|m| rest.starts_with(**m))
        .map(|m| m.width())
        .or_else(|| {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && rest[digits..].starts_with(". ")).then_some(digits + 2)
        })
        .unwrap_or(0);
    spaces + marker
}

/// `text` cut to `max` columns, ending in … when anything was dropped.
/// `pub(crate)`: also used by `render::agents` for the Agents screen's row
/// truncation.
pub(crate) fn fit(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(ch);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

fn wrap_all(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    lines.iter().flat_map(|l| wrap_line(l, width)).collect()
}

/// Wrap only lines wider than `width`, leaving the rest (code alignment,
/// blank lines) untouched. Leading indentation becomes a hanging indent so
/// wrapped list items and quotes stay aligned.
fn wrap_overlong(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= width as usize {
            out.push(line);
            continue;
        }
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let indent = hanging_indent(&text);
        let indent = if indent >= width as usize / 2 { 0 } else { indent };
        let line_style = line.style;
        // The prefix (indent + bullet/number/quote) stays on the first row;
        // the body wraps in the remaining width and later rows hang under it.
        let (prefix, body) = split_at_column(line, indent);
        let pad_style = prefix.first().map(|s| s.style).unwrap_or_default();
        let rows = wrap_line(&Line::from(body), width.saturating_sub(indent as u16).max(1));
        for (row, wrapped) in rows.into_iter().enumerate() {
            let mut spans = if row == 0 {
                prefix.clone()
            } else if indent > 0 {
                vec![Span::styled(" ".repeat(indent), pad_style)]
            } else {
                Vec::new()
            };
            spans.extend(wrapped.spans);
            out.push(Line::from(spans).style(line_style));
        }
    }
    out
}

#[cfg(test)]
pub static RENDER_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `pub(crate)`: also used directly by `render::agents` to render a job
/// detail's `timeline_blocks` output with the same look as the live
/// transcript, without going through `RenderCache` (whose per-block
/// revision counters don't apply to a one-off, periodically-refetched list).
pub(crate) fn render_block(
    block: &Block,
    width: u16,
    theme: &Theme,
    hl: &Highlighter,
    debug: bool,
    spinner_frame: usize,
) -> Vec<Line<'static>> {
    #[cfg(test)]
    RENDER_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut out: Vec<Line<'static>> = Vec::new();
    match block {
        Block::User { text } => {
            out.push(Line::default());
            let inner_width = width.saturating_sub(4).max(1);
            let mut message = Vec::new();
            let text_width = inner_width.saturating_sub(2).max(1);
            let mut first_visual_line = true;
            for l in text.lines() {
                let wrapped = wrap_line(&styled_user_line(l, theme), text_width);
                for line in wrapped {
                    let glyph = if first_visual_line {
                        theme.icon("󰅂 ", "› ", "> ")
                    } else {
                        "  "
                    };
                    first_visual_line = false;
                    let mut spans = vec![Span::styled(glyph, theme.user)];
                    spans.extend(line.spans);
                    message.push(Line::from(spans));
                }
            }
            out.push(Line::from(Span::styled(
                theme.glyph("▄", "-").repeat(width as usize), theme.user_cap,
            )));
            out.extend(
                BlockFrame::new(theme.user_surface)
                    .padding(2, 2)
                    .render(message, width),
            );
            out.push(Line::from(Span::styled(
                theme.glyph("▀", "-").repeat(width as usize), theme.user_cap,
            )));
            out
        }
        Block::Assistant {
            markdown,
            streaming,
        } => {
            // Core owns the display protocol; rendering never parses tags.
            let shown = markdown;
            // Whitespace-only text (models emit "\n\n" between tool calls) would add
            // blank rows; render nothing so tool rows stay one line apart.
            if !*streaming && shown.trim().is_empty() {
                return out;
            }
            let mut body = render_markdown(shown, theme, hl);
            if *streaming {
                let cursor = Span::styled(theme.glyph("▍", "_"), theme.accent);
                match body.last_mut() {
                    Some(last) => last.spans.push(cursor),
                    None => body.push(Line::from(cursor)),
                }
            }
            // The frame clips; wrap first so a paragraph (one logical line
            // out of the markdown renderer) is not cut at the frame edge.
            let body = wrap_overlong(body, width.saturating_sub(4).max(1));
            out.push(Line::default());
            out.extend(
                BlockFrame::new(theme.assistant_surface)
                    .padding(2, 2)
                    .render(body, width),
            );
            out
        }
        Block::Tool {
            command,
            output,
            exit_code,
            folded,
            running,
            count,
            ..
        } => {
            out.push(Line::default());
            // A live braille spinner replaces the static "~ running" glyph so
            // the user sees movement while the call is in flight. Once output
            // lands the spinner is dropped in favour of a ✓/✗ status glyph.
            let status = if *running {
                let glyphs = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let g = glyphs[spinner_frame % glyphs.len()];
                Span::styled(format!(" {g} running"), theme.warn)
            } else {
                match (output, exit_code) {
                    (Some(_), Some(0)) => Span::styled(" ✓ ok exit 0".to_string(), theme.ok),
                    (Some(_), Some(c)) => Span::styled(format!(" ✗ exit {c}"), theme.fail),
                    (Some(_), None) => Span::styled(" ✓ done".to_string(), theme.ok),
                    (None, _) => Span::styled(" ~ running".to_string(), theme.warn),
                }
            };
            // The program name keeps the tool colour; its arguments are quieter.
            let (program, args) = if command.is_empty() {
                ("(command)", "")
            } else {
                let trimmed = command.trim_start();
                let split = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
                trimmed.split_at(split)
            };
            let icon = theme.icon("󰞷 ", "◆ ", "$ ");
            if *folded || output.is_none() {
                // Folded or still running: the header is one row — the
                // command's first line, cut with … so the status always fits.
                // When folded the raw command is hidden; ^T restores it.
                let first = args.lines().next().unwrap_or("");
                let more = command.trim().lines().nth(1).is_some();
                let budget = (width as usize)
                    .saturating_sub(icon.width() + program.width() + status.content.width());
                let count_suffix = if *count > 1 {
                    format!(" (×{})", count)
                } else {
                    String::new()
                };
                let shown = fit(&format!("{first}{count_suffix}{}", if more { " …" } else { "" }), budget);
                out.push(Line::from(vec![
                    Span::styled(icon.to_string(), theme.accent),
                    Span::styled(program.to_string(), theme.tool_cmd),
                    Span::styled(shown, theme.tool_args),
                    status,
                ]));
                if let Some(text) = output {
                    let total = text.lines().count();
                    let hint = format!(
                        "  {} {total} lines folded (^T to unfold)",
                        theme.icon("󰅂", "▸", ">")
                    );
                    out.push(Line::from(Span::styled(fit(&hint, width as usize), theme.dim)));
                }
                return out;
            }
            // Unfolded: the whole command and its output, wrapped.
            let mut header = vec![
                Span::styled(icon.to_string(), theme.accent),
                Span::styled(program.to_string(), theme.tool_cmd),
            ];
            if !args.is_empty() {
                header.push(Span::styled(args.to_string(), theme.tool_args));
            }
            header.push(status);
            out.push(Line::from(header));
            if let Some(text) = output {
                for l in text.lines() {
                    out.push(Line::from(vec![
                        Span::styled(theme.glyph("  │ ", "  | "), theme.dim),
                        Span::styled(l.to_string(), theme.tool_out),
                    ]));
                }
            }
            wrap_all(out, width)
        }
        Block::Banner { kind, text } => {
            out.push(Line::default());
            let (label, style) = match kind {
                BannerKind::Error => ("ERROR", theme.fail),
            };
            for (i, l) in text.lines().enumerate() {
                let prefix = if i == 0 {
                    format!("{} {label}: ", theme.icon("󰀪", "!", "!"))
                } else {
                    "   ".to_string()
                };
                out.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(l.to_string(), Style::default()),
                ]));
            }
            wrap_all(out, width)
        }
        Block::Hook {
            event,
            script,
            action,
            exit_code,
            duration_ms,
            timed_out,
        } => {
            let separator = theme.glyph(" · ", " | ");
            let arrow = theme.glyph(" → ", " -> ");
            let mut detail = format!(
                "{} hook {event}{separator}{script}{arrow}{}",
                theme.icon("󰛢", "⌁", "~"),
                action.as_deref().unwrap_or("continue")
            );
            if let Some(c) = exit_code {
                detail.push_str(&format!("{separator}exit {c}"));
            }
            if let Some(d) = duration_ms {
                detail.push_str(&format!("{separator}{d}ms"));
            }
            if *timed_out {
                detail.push_str(&format!("{separator}TIMED OUT"));
            }
            let style = if matches!(action.as_deref(), Some("veto")) || *timed_out {
                theme.warn
            } else {
                theme.dim
            };
            wrap_all(vec![Line::from(Span::styled(detail, style))], width)
        }
        Block::Marker { text } => {
            out.push(Line::default());
            out.push(Line::from(Span::styled(
                format!(
                    "{} {text} {}",
                    theme.glyph("──", "--"),
                    theme.glyph("──", "--")
                ),
                theme.marker,
            )));
            wrap_all(out, width)
        }
        Block::Verify {
            status,
            final_,
            claims,
            ..
        } => {
            let failed = claims.iter().filter(|c| !c.ok).count();
            let total = claims.len();
            out.push(Line::default());
            let headline = match status {
                VerifyStatus::Passed => Line::from(Span::styled(
                    format!(
                        "{} verified {} {total} {}",
                        theme.glyph("✓", "ok"),
                        theme.glyph("·", "-"),
                        if total == 1 { "claim" } else { "claims" }
                    ),
                    theme.ok,
                )),
                VerifyStatus::Failed if !*final_ => Line::from(Span::styled(
                    format!(
                        "{} verification failed {} {failed} of {total} {} correcting",
                        theme.glyph("✗", "x"),
                        theme.glyph("·", "-"),
                        theme.glyph("—", "-"),
                    ),
                    theme.warn,
                )),
                VerifyStatus::Failed => Line::from(Span::styled(
                    format!(
                        "{} verification failed {} {failed} of {total}",
                        theme.glyph("✗", "x"),
                        theme.glyph("·", "-"),
                    ),
                    theme.fail,
                )),
            };
            out.push(headline);
            if debug {
                for claim in claims {
                    let (mark, style) = if claim.ok {
                        (theme.glyph("✓", "ok"), theme.ok)
                    } else {
                        (theme.glyph("✗", "x"), theme.fail)
                    };
                    out.push(Line::from(vec![
                        Span::styled(format!("  {mark} "), style),
                        Span::styled(claim.claim.clone(), style),
                        Span::styled(format!(" {} ", theme.glyph("—", "-")), theme.dim),
                        Span::styled(claim.detail.clone(), theme.dim),
                    ]));
                }
            }
            wrap_all(out, width)
        }
        Block::Sources { entries, speech } => {
            if entries.is_empty() && !(debug && !speech.is_empty()) {
                return out;
            }
            out.push(Line::default());
            if !entries.is_empty() {
                out.push(Line::from(Span::styled("sources", theme.dim)));
                for entry in entries {
                    let (icon, label, detail, missing) = match entry.kind {
                        SourceKind::File => {
                            // Debug shows the full path; otherwise the
                            // document's title, or just its file name.
                            let path = entry.path.as_deref().unwrap_or(&entry.raw);
                            let name = entry.name.clone().unwrap_or_else(|| {
                                path.rsplit('/').next().unwrap_or(path).to_string()
                            });
                            let base = if debug {
                                path.to_string()
                            } else {
                                entry.title.clone().unwrap_or_else(|| name.clone())
                            };
                            let label = match entry.line {
                                Some(line) => format!("{base}:{line}"),
                                None => base,
                            };
                            let detail = (!debug && entry.title.is_some()).then_some(name);
                            (theme.icon("󰈔", "·", "[f]"), label, detail, entry.exists == Some(false))
                        }
                        SourceKind::Url => (theme.icon("󰖟", "~", "[url]"), entry.raw.clone(), None, false),
                    };
                    // One row per source: drop the dim file name first, then cut
                    // the label with … — never wrap.
                    let prefix = format!("  {icon} ");
                    let flag = if missing { " ✗ missing" } else { "" };
                    let room = (width as usize).saturating_sub(prefix.width() + flag.width());
                    let detail = detail
                        .map(|d| format!("  {d}"))
                        .filter(|d| label.width() + d.width() <= room);
                    let label = fit(&label, room.saturating_sub(detail.as_ref().map_or(0, |d| d.width())));
                    let mut spans = vec![Span::styled(prefix, theme.link), Span::styled(label, theme.link)];
                    if let Some(detail) = detail {
                        spans.push(Span::styled(detail, theme.dim));
                    }
                    if missing {
                        spans.push(Span::styled(flag, theme.fail));
                    }
                    out.push(Line::from(spans));
                }
            }
            if debug && !speech.is_empty() {
                out.push(Line::from(vec![
                    Span::styled("speak: ", theme.dim),
                    Span::styled(speech.clone(), theme.dim),
                ]));
            }
            wrap_all(out, width)
        }
    }
}

fn styled_user_line(text: &str, theme: &Theme) -> Line<'static> {
    let spans = text
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            let word = token.trim_end();
            let style = if word.starts_with('/') {
                theme.accent
            } else if word.starts_with('@') {
                theme.link
            } else {
                theme.user_text
            };
            Span::styled(token.to_string(), style)
        })
        .collect::<Vec<_>>();
    if spans.is_empty() {
        Line::from(Span::styled(String::new(), theme.user_text))
    } else {
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    }

    #[test]
    fn wraps_at_word_boundaries_and_hard_breaks_long_words() {
        let line = Line::from(vec![
            Span::raw("hello wide "),
            Span::styled("world", Style::default()),
            Span::raw(" abcdefghijkl"),
        ]);
        let wrapped = text(&wrap_line(&line, 10));
        assert_eq!(
            wrapped,
            vec!["hello wide", "world", "abcdefghij", "kl"],
            "{wrapped:?}"
        );
        assert_eq!(text(&wrap_line(&Line::default(), 10)), vec![""]);
    }

    #[test]
    fn unchanged_transcript_and_width_does_not_rerender() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let mut t = Transcript::new();
        t.push_user("hi");
        t.append_assistant("Hello there");
        t.finish_assistant();
        let mut cache = RenderCache::default();
        cache.lines(&t, 40, &theme, &hl, 0);
        let after_first = RENDER_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        cache.lines(&t, 40, &theme, &hl, 0);
        let after_second = RENDER_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            after_first, after_second,
            "a second lines() call with nothing changed must hit the cache, not re-render"
        );
    }

    #[test]
    fn cache_only_rerenders_changed_blocks() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let mut t = Transcript::new();
        t.push_user("hi");
        t.append_assistant("Hello **there**");
        let mut cache = RenderCache::default();
        let first = text(&cache.lines(&t, 40, &theme, &hl, 0));
        assert!(first.iter().any(|l| l.trim() == "› hi"));
        assert!(first.iter().any(|l| l.contains("Hello there")));
        t.append_assistant(" again");
        let second = text(&cache.lines(&t, 40, &theme, &hl, 0));
        assert!(second.iter().any(|l| l.contains("Hello there again")));
        assert_eq!(cache.entries.len(), 2);

        t.show_hooks = false;
        t.push_hook(&serde_json::json!({"kind":"hook_end","event":"pre-tool","script":"x.sh","action":"continue"}));
        let hidden = text(&cache.lines(&t, 40, &theme, &hl, 0));
        assert!(!hidden.iter().any(|l| l.contains("hook pre-tool")));
        t.show_hooks = true;
        let shown = text(&cache.lines(&t, 40, &theme, &hl, 0));
        assert!(shown.iter().any(|l| l.contains("hook pre-tool")));
    }

    #[test]
    fn wrapped_user_text_keeps_the_prompt_indent() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let mut transcript = Transcript::new();
        transcript.push_user("one two three four five");
        let mut cache = RenderCache::default();
        let lines = text(&cache.lines(&transcript, 16, &theme, &hl, 0));
        let first = lines.iter().find(|line| line.contains("one")).unwrap();
        let continuation = lines.iter().find(|line| line.contains("three")).unwrap();
        assert!(first.starts_with("  › "), "{first:?}");
        assert!(continuation.starts_with("    "), "{continuation:?}");
        assert_eq!(first.chars().count(), 16);
        assert_eq!(continuation.chars().count(), 16);
    }

    #[test]
    fn user_messages_have_half_block_padding_and_assistant_messages_are_transparent() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let user = render_block(&Block::User { text: "Hello".into() }, 24, &theme, &hl, false, 0);
        let rendered = text(&user);
        assert_eq!(rendered[1], "▄".repeat(24));
        assert_eq!(rendered.last().unwrap(), &"▀".repeat(24));
        assert_eq!(user[2].style.bg, theme.user_surface.bg);
        let assistant = render_block(&Block::Assistant {
            markdown: "Hello from Lazar.".into(), streaming: false,
        }, 24, &theme, &hl, false, 0);
        assert!(text(&assistant).iter().any(|line| line.contains("Hello from Lazar.")));
        assert!(assistant.iter().all(|line| line.style.bg.is_none()
            && line.spans.iter().all(|span| span.style.bg.is_none())));
    }

    #[test]
    fn a_long_folded_command_stays_on_one_header_row() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let block = Block::Tool {
            id: String::new(),
            command: format!("cat >> /tmp/notes.md << 'EOF'\n{}\nEOF", "word ".repeat(80)),
            output: Some("done\n".into()),
            exit_code: Some(0),
            folded: true,
            count: 1,
            running: false,
            };
        let rendered = text(&render_block(&block, 60, &theme, &Highlighter::new(), false, 0));
        assert_eq!(rendered.len(), 3, "blank, header, fold hint: {rendered:?}");
        assert!(rendered[1].chars().count() <= 60, "{rendered:?}");
        assert!(rendered[1].contains("cat") && rendered[1].contains('…') && rendered[1].ends_with("ok exit 0"), "{rendered:?}");
        assert!(rendered[2].contains("1 lines folded"), "{rendered:?}");
    }

    #[test]
    fn folded_tool_rows_are_one_line_apart_with_quiet_arguments() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let tool = |command: &str| Block::Tool {
            id: String::new(),
            command: command.into(),
            output: Some("a\nb\n".into()),
            exit_code: Some(0),
            folded: true,
            count: 1,
            running: false,
            };
        let empty = Block::Assistant { markdown: "\n\n\n".into(), streaming: false };
        let mut lines = Vec::new();
        for block in [tool("tail -n 30 journal.md"), empty, tool("git log")] {
            lines.extend(render_block(&block, 80, &theme, &hl, false, 0));
        }
        let rendered = text(&lines);
        assert_eq!(rendered.len(), 6, "{rendered:?}");
        assert_eq!(rendered[3], "", "exactly one blank row between tools: {rendered:?}");
        let header = &lines[1];
        let program = header.spans.iter().find(|s| s.content == "tail").unwrap();
        assert_eq!(program.style, theme.tool_cmd);
        let args = header.spans.iter().find(|s| s.content.contains("-n 30")).unwrap();
        assert_eq!(args.style, theme.tool_args);
        assert_ne!(theme.tool_args, theme.tool_cmd);
    }

    #[test]
    fn wrapped_list_items_hang_under_their_text() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let md = "- memory — plain markdown notes that survive across invocations and stay readable\n\n1. first numbered item that is long enough to wrap onto another row";
        let lines = render_block(&Block::Assistant { markdown: md.into(), streaming: false }, 40, &theme, &Highlighter::new(), false, 0);
        let rendered = text(&lines);
        let bullet = rendered.iter().position(|l| l.contains("• memory")).unwrap();
        let col = |line: &str, word: &str| line[..line.find(word).unwrap()].chars().count();
        let bullet_col = col(&rendered[bullet], "memory");
        let next = &rendered[bullet + 1];
        assert_eq!(next.len() - next.trim_start().len(), bullet_col, "{rendered:?}");
        let numbered = rendered.iter().position(|l| l.contains("1. first")).unwrap();
        let text_col = col(&rendered[numbered], "first");
        let next = &rendered[numbered + 1];
        assert_eq!(next.len() - next.trim_start().len(), text_col, "{rendered:?}");
        assert!(rendered.iter().all(|l| l.chars().count() <= 40), "{rendered:?}");
    }

    #[test]
    fn sources_show_titles_or_file_names_on_one_row() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let entry = |title: Option<&str>| crate::transcript::SourceEntry {
            raw: "/Users/x/lazar/user/memory/research/threejs-3d-games-flight-simulators.md".into(),
            kind: crate::transcript::SourceKind::File,
            path: Some("/Users/x/lazar/user/memory/research/threejs-3d-games-flight-simulators.md".into()),
            line: Some(4),
            exists: Some(true),
            name: Some("threejs-3d-games-flight-simulators.md".into()),
            title: title.map(str::to_string),
        };
        let block = Block::Sources { entries: vec![entry(Some("Three.js flight sims")), entry(None)], speech: String::new() };
        let rendered = text(&render_block(&block, 50, &theme, &Highlighter::new(), false, 0));
        assert!(rendered.iter().any(|l| l.contains("Three.js flight sims:4")), "{rendered:?}");
        let wide = text(&render_block(&block, 100, &theme, &Highlighter::new(), false, 0));
        assert!(wide.iter().any(|l| l.contains("Three.js flight sims:4") && l.contains("threejs-3d-games")), "{wide:?}");
        assert!(!rendered.iter().any(|l| l.contains("/Users/x")), "no full paths: {rendered:?}");
        assert!(rendered.iter().all(|l| l.chars().count() <= 50), "{rendered:?}");
        assert_eq!(rendered.iter().filter(|l| l.trim_start().starts_with('·')).count(), 2, "one row each: {rendered:?}");
        let debug = text(&render_block(&block, 200, &theme, &Highlighter::new(), true, 0));
        assert!(debug.iter().any(|l| l.contains("/Users/x/lazar/user/memory/research/")), "{debug:?}");
    }

    #[test]
    fn long_assistant_paragraphs_wrap_instead_of_clipping() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let md = "Want me to look at the scroll/viewport logic in `app.rs` to see if there's a clamp bug? Or rebuild the TUI from source so you're running current code?\n\n- a list item that is long enough to need wrapping onto a second line";
        let lines = render_block(&Block::Assistant { markdown: md.into(), streaming: false }, 40, &theme, &Highlighter::new(), false, 0);
        let rendered = text(&lines);
        let joined = rendered.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("running current code?"), "{rendered:?}");
        assert!(joined.contains("onto a second line"), "{rendered:?}");
        assert!(rendered.iter().all(|l| l.chars().count() <= 40), "{rendered:?}");
    }

    #[test]
    fn code_blocks_keep_their_own_background_inside_the_reply_box() {
        // Force truecolor: under `cargo test` there is no terminal, so the two
        // greys would both quantize to Black and the check would be vacuous.
        std::env::set_var("LAZARTUI_COLOR", "truecolor");
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        std::env::remove_var("LAZARTUI_COLOR");
        let block = Block::Assistant {
            markdown: "Run this:\n\n```sh\nls -la\n```\n".into(),
            streaming: false,
        };
        let lines = render_block(&block, 40, &theme, &Highlighter::new(), false, 0);
        // The highlighter may split the command into several spans; find the
        // line whose concatenated text carries it.
        let code_line = lines
            .iter()
            .find(|l| text(std::slice::from_ref(l))[0].contains("ls -la"))
            .expect("code line rendered");
        let code_span = code_line
            .spans
            .iter()
            .find(|s| s.content.contains("ls") || s.content.contains("-la"))
            .unwrap();
        assert_eq!(code_span.style.bg, theme.code.bg, "code bg survives the frame");
        assert_ne!(theme.code.bg, theme.assistant_surface.bg);
        // …while the padding around it is the box surface.
        assert_eq!(code_line.spans.first().unwrap().style.bg, theme.assistant_surface.bg);
    }

    #[test]
    fn display_ready_text_renders_identically_in_debug() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Assistant { markdown: "Done.\n\nMore text.".into(), streaming: false };
        assert_eq!(text(&render_block(&block, 80, &theme, &hl, true, 0)), text(&render_block(&block, 80, &theme, &hl, false, 0)));
    }

    #[test]
    fn empty_display_reply_renders_nothing() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Assistant { markdown: "\n\n".into(), streaming: false };
        assert!(render_block(&block, 40, &theme, &hl, false, 0).is_empty());
    }

    #[test]
    fn renderer_does_not_interpret_protocol_tags() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Assistant { markdown: "[VERIFY] literal input [/VERIFY]".into(), streaming: false };
        assert!(text(&render_block(&block, 80, &theme, &hl, false, 0)).join("\n").contains("[VERIFY]"));
    }

    fn file_source(raw: &str, line: Option<u64>, exists: Option<bool>) -> crate::transcript::SourceEntry {
        crate::transcript::SourceEntry {
            raw: raw.into(),
            kind: crate::transcript::SourceKind::File,
            path: Some(raw.into()),
            line,
            exists,
            name: None,
            title: None,
        }
    }

    fn url_source(raw: &str) -> crate::transcript::SourceEntry {
        crate::transcript::SourceEntry {
            raw: raw.into(),
            kind: crate::transcript::SourceKind::Url,
            path: None,
            line: None,
            exists: None,
            name: None,
            title: None,
        }
    }

    #[test]
    fn renders_a_sources_block_with_file_url_and_missing_entries() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Sources {
            entries: vec![
                file_source("/abs/x.rs", Some(42), Some(true)),
                file_source("/abs/gone.rs", Some(7), Some(false)),
                url_source("https://example.com/page"),
            ],
            speech: String::new(),
        };
        let lines = text(&render_block(&block, 80, &theme, &hl, false, 0));
        let joined = lines.join("\n");
        assert!(joined.contains("sources"));
        // File names by default; full paths are debug-only.
        assert!(joined.contains("x.rs:42") && !joined.contains("/abs/x.rs"), "{joined}");
        assert!(joined.contains("gone.rs:7") && joined.contains("missing"));
        assert!(joined.contains("https://example.com/page"));
    }

    #[test]
    fn sources_block_shows_speech_only_in_debug_mode() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Sources { entries: vec![], speech: "It's fixed.".into() };
        let plain = render_block(&block, 80, &theme, &hl, false, 0);
        assert!(plain.is_empty(), "{plain:?}");
        let debug = text(&render_block(&block, 80, &theme, &hl, true, 0));
        assert!(debug.iter().any(|l| l.contains("speak:") && l.contains("It's fixed.")), "{debug:?}");
    }

    fn claim(claim: &str, ok: bool, detail: &str) -> crate::transcript::VerifyClaim {
        crate::transcript::VerifyClaim {
            claim: claim.into(),
            ok,
            detail: detail.into(),
        }
    }

    #[test]
    fn renders_a_passed_verify_event() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Verify {
            status: VerifyStatus::Passed,
            attempt: 1,
            final_: true,
            tools_run: 2,
            claims: vec![claim("exists=/tmp/x", true, "exists")],
        };
        let lines = text(&render_block(&block, 80, &theme, &hl, false, 0));
        assert!(lines.iter().any(|l| l.contains("verified") && l.contains("1 claim")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("exists")), "claim detail is debug-only: {lines:?}");
    }

    #[test]
    fn renders_a_failed_retry_verify_event_with_debug_claim_detail() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Verify {
            status: VerifyStatus::Failed,
            attempt: 1,
            final_: false,
            tools_run: 2,
            claims: vec![claim("exists=/tmp/x", true, "exists"), claim("contains=/tmp/y:z", false, "not found")],
        };
        let lines = text(&render_block(&block, 80, &theme, &hl, true, 0));
        assert!(lines.iter().any(|l| l.contains("1 of 2") && l.contains("correcting")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("contains=/tmp/y:z") && l.contains("not found")), "{lines:?}");
    }

    #[test]
    fn renders_a_final_failed_verify_event_without_correcting_suffix() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let block = Block::Verify {
            status: VerifyStatus::Failed,
            attempt: 2,
            final_: true,
            tools_run: 2,
            claims: vec![claim("exists=/tmp/x", false, "missing")],
        };
        let lines = text(&render_block(&block, 80, &theme, &hl, false, 0));
        assert!(lines.iter().any(|l| l.contains("1 of 1") && !l.contains("correcting")), "{lines:?}");
    }

    fn done_tool(command: &str, code: i32) -> Block {
        Block::Tool {
            id: command.into(),
            command: command.into(),
            output: Some("out".into()),
            exit_code: Some(code),
            folded: true,
            running: false,
            count: 1,
        }
    }

    #[test]
    fn finished_tool_run_folds_to_one_summary_row() {
        let blocks = vec![
            done_tool("cd a", 0),
            done_tool("cd b", 0),
            done_tool("grep x", 1),
            Block::Assistant { markdown: "found it".into(), streaming: false },
        ];
        let plan = tool_fold_plan(&blocks, false, false);
        assert_eq!(plan[0], Fold::Summary(vec![0, 1, 2]));
        assert_eq!(plan[1], Fold::Hide);
        assert_eq!(plan[3], Fold::Show);
        let row: String = tool_summary_lines(&blocks, &[0, 1, 2], 120, &Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode))[1]
            .spans.iter().map(|s| s.content.to_string()).collect();
        assert!(row.contains("3 tool calls · cd ×2 · grep"), "{row}");
        assert!(row.contains("1 failed"), "{row}");
    }

    #[test]
    fn live_run_folds_same_program_stretches_but_keeps_the_running_call() {
        let mut running = done_tool("grep y", 0);
        if let Block::Tool { output, running: r, exit_code, .. } = &mut running {
            *output = None;
            *r = true;
            *exit_code = None;
        }
        let blocks = vec![done_tool("cd a", 0), done_tool("cd b", 0), done_tool("cd c", 0), done_tool("sed 1p f", 0), running];
        let plan = tool_fold_plan(&blocks, false, false);
        assert_eq!(plan[0], Fold::Summary(vec![0, 1, 2]));
        assert_eq!(plan[3], Fold::Show);
        assert_eq!(plan[4], Fold::Show);
    }

    #[test]
    fn rendering_a_folded_run_then_more_blocks_does_not_panic() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let mut t = Transcript::new();
        for n in 0..3 {
            t.start_tool(&format!("t{n}"), &format!("cd d{n}"));
            t.finish_tool(&format!("t{n}"), "x\n".repeat(40), Some(0));
        }
        t.push_marker("after");
        t.start_tool("t9", "ls");
        let mut cache = RenderCache::default();
        let first = cache.lines(&t, 100, &theme, &hl, 0);
        let second = cache.lines(&t, 100, &theme, &hl, 1);
        let text: String = second.iter().flat_map(|l| l.spans.iter().map(|s| s.content.to_string())).collect();
        assert!(text.contains("cd ×3"), "{text}");
        assert_eq!(first.len(), second.len());
    }

    #[test]
    fn unfolded_tools_are_never_grouped() {
        let blocks = vec![done_tool("cd a", 0), done_tool("cd b", 0), Block::Assistant { markdown: "x".into(), streaming: false }];
        // When tools_unfolded is true, no summary fold is generated.
        assert!(tool_fold_plan(&blocks, false, true).iter().all(|f| *f == Fold::Show));
    }

    #[test]
    fn running_tool_shows_braille_spinner_and_advances_with_frame() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let running_block = Block::Tool {
            id: "t1".into(),
            command: "sleep 5".into(),
            output: None,
            exit_code: None,
            folded: false,
            running: true,
            count: 1,
        };
        // Frame 0: first braille glyph
        let lines0 = text(&render_block(&running_block, 60, &theme, &hl, false, 0));
        assert!(lines0.iter().any(|l| l.contains("running")));
        // Frame 1: second braille glyph
        let lines1 = text(&render_block(&running_block, 60, &theme, &hl, false, 1));
        assert!(lines1.iter().any(|l| l.contains("running")));
    }

    #[test]
    fn collapsed_tool_chip_shows_multiplier_suffix() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let collapsed_block = Block::Tool {
            id: "t1".into(),
            command: "git status".into(),
            output: Some("clean
".into()),
            exit_code: Some(0),
            folded: true,
            running: false,
            count: 5,
        };
        let lines = text(&render_block(&collapsed_block, 60, &theme, &hl, false, 0));
        assert!(lines.iter().any(|l| l.contains("(×5)")), "expected (×5) multiplier in: {lines:?}");
    }
}

