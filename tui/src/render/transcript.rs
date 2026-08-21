//! Transcript blocks to wrapped, styled lines, with a per-block cache so a
//! long conversation does not re-parse every frame.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::block_frame::BlockFrame;
use super::markdown::{render_markdown, Highlighter};
use super::theme::Theme;
use crate::transcript::{BannerKind, Block, Transcript};

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
    ) -> Vec<Line<'static>> {
        self.entries.truncate(transcript.blocks.len());
        let mut out = Vec::new();
        for (i, block) in transcript.blocks.iter().enumerate() {
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
            let fresh = self
                .entries
                .get(i)
                .map(|e| e.key != key || e.width != width || e.theme_key != theme_key)
                .unwrap_or(true);
            if fresh {
                let rendered = render_block(block, width, theme, hl);
                let entry = CacheEntry {
                    key,
                    width,
                    theme_key,
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
        out
    }
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
        let indent = text.len() - text.trim_start_matches(' ').len();
        let indent = if indent >= width as usize { 0 } else { indent };
        let indent_style = line.spans.first().map(|s| s.style).unwrap_or_default();
        let line_style = line.style;
        for wrapped in wrap_line(&line, width.saturating_sub(indent as u16).max(1)) {
            let mut spans = Vec::with_capacity(wrapped.spans.len() + 1);
            if indent > 0 {
                spans.push(Span::styled(" ".repeat(indent), indent_style));
            }
            spans.extend(wrapped.spans);
            out.push(Line::from(spans).style(line_style));
        }
    }
    out
}

#[cfg(test)]
pub static RENDER_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn render_block(block: &Block, width: u16, theme: &Theme, hl: &Highlighter) -> Vec<Line<'static>> {
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
            let mut body = render_markdown(markdown, theme, hl);
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
            ..
        } => {
            out.push(Line::default());
            let status = match (output, exit_code) {
                (None, _) => Span::styled(" ~ running".to_string(), theme.warn),
                (Some(_), Some(0)) => Span::styled(" ok exit 0".to_string(), theme.ok),
                (Some(_), Some(c)) => Span::styled(format!(" x exit {c}"), theme.fail),
                (Some(_), None) => Span::styled(" done".to_string(), theme.dim),
            };
            let cmd_display = if command.is_empty() {
                "(command)".to_string()
            } else {
                command.clone()
            };
            out.push(Line::from(vec![
                Span::styled(theme.icon("󰞷 ", "◆ ", "$ "), theme.accent),
                Span::styled(cmd_display, theme.tool_cmd),
                status,
            ]));
            if let Some(text) = output {
                let total = text.lines().count();
                if *folded {
                    out.push(Line::from(Span::styled(
                        format!(
                            "  {} {total} lines folded (^T to unfold)",
                            theme.icon("󰅂", "▸", ">")
                        ),
                        theme.dim,
                    )));
                } else {
                    for l in text.lines() {
                        out.push(Line::from(vec![
                            Span::styled(theme.glyph("  │ ", "  | "), theme.dim),
                            Span::styled(l.to_string(), theme.tool_out),
                        ]));
                    }
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
        cache.lines(&t, 40, &theme, &hl);
        let after_first = RENDER_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        cache.lines(&t, 40, &theme, &hl);
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
        let first = text(&cache.lines(&t, 40, &theme, &hl));
        assert!(first.iter().any(|l| l.trim() == "› hi"));
        assert!(first.iter().any(|l| l.contains("Hello there")));
        t.append_assistant(" again");
        let second = text(&cache.lines(&t, 40, &theme, &hl));
        assert!(second.iter().any(|l| l.contains("Hello there again")));
        assert_eq!(cache.entries.len(), 2);

        t.show_hooks = false;
        t.push_hook(&serde_json::json!({"kind":"hook_end","event":"pre-tool","script":"x.sh","action":"continue"}));
        let hidden = text(&cache.lines(&t, 40, &theme, &hl));
        assert!(!hidden.iter().any(|l| l.contains("hook pre-tool")));
        t.show_hooks = true;
        let shown = text(&cache.lines(&t, 40, &theme, &hl));
        assert!(shown.iter().any(|l| l.contains("hook pre-tool")));
    }

    #[test]
    fn wrapped_user_text_keeps_the_prompt_indent() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let hl = Highlighter::new();
        let mut transcript = Transcript::new();
        transcript.push_user("one two three four five");
        let mut cache = RenderCache::default();
        let lines = text(&cache.lines(&transcript, 16, &theme, &hl));
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
        let user = render_block(&Block::User { text: "Hello".into() }, 24, &theme, &hl);
        let rendered = text(&user);
        assert_eq!(rendered[1], "▄".repeat(24));
        assert_eq!(rendered.last().unwrap(), &"▀".repeat(24));
        assert_eq!(user[2].style.bg, theme.user_surface.bg);
        let assistant = render_block(&Block::Assistant {
            markdown: "Hello from Lazar.".into(), streaming: false,
        }, 24, &theme, &hl);
        assert!(text(&assistant).iter().any(|line| line.contains("Hello from Lazar.")));
        assert!(assistant.iter().all(|line| line.style.bg.is_none()
            && line.spans.iter().all(|span| span.style.bg.is_none())));
    }

    #[test]
    fn long_assistant_paragraphs_wrap_instead_of_clipping() {
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let md = "Want me to look at the scroll/viewport logic in `app.rs` to see if there's a clamp bug? Or rebuild the TUI from source so you're running current code?\n\n- a list item that is long enough to need wrapping onto a second line";
        let lines = render_block(&Block::Assistant { markdown: md.into(), streaming: false }, 40, &theme, &Highlighter::new());
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
        let lines = render_block(&block, 40, &theme, &Highlighter::new());
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
}

