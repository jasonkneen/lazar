//! Markdown to ratatui lines: headings, emphasis, inline code, fenced code
//! with syntax highlighting, lists (nested, ordered, tasks), block quotes,
//! tables, rules, links.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SyntectTheme, ThemeSet};
use syntect::parsing::SyntaxSet;
use unicode_width::UnicodeWidthStr;

use super::theme::{Theme, ThemeKind};

pub struct Highlighter {
    syntaxes: SyntaxSet,
    dark: SyntectTheme,
    light: SyntectTheme,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    pub fn new() -> Self {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let dark = themes
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .or_else(|| themes.themes.values().next().cloned())
            .expect("syntect ships default themes");
        let light = themes
            .themes
            .get("InspiredGitHub")
            .or_else(|| themes.themes.get("Solarized (light)"))
            .cloned()
            .unwrap_or_else(|| dark.clone());
        Self {
            syntaxes,
            dark,
            light,
        }
    }

    fn highlight(
        &self,
        code: &str,
        lang: &str,
        base: Style,
        ui_theme: &Theme,
    ) -> Vec<Line<'static>> {
        let syntax = self
            .syntaxes
            .find_syntax_by_token(lang)
            .or_else(|| self.syntaxes.find_syntax_by_extension(lang))
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let syntax_theme = if ui_theme.resolved_kind == ThemeKind::GrokDay {
            &self.light
        } else {
            &self.dark
        };
        let mut hl = HighlightLines::new(syntax, syntax_theme);
        let mut out = Vec::new();
        for raw in code.lines() {
            let line_with_nl = format!("{raw}\n");
            let spans: Vec<Span<'static>> = match hl.highlight_line(&line_with_nl, &self.syntaxes) {
                Ok(regions) => regions
                    .into_iter()
                    .map(|(style, text)| {
                        let fg = style.foreground;
                        Span::styled(
                            text.trim_end_matches('\n').to_string(),
                            base.fg(ui_theme
                                .quantize_color(ratatui::style::Color::Rgb(fg.r, fg.g, fg.b))),
                        )
                    })
                    .collect(),
                Err(_) => vec![Span::styled(raw.to_string(), base)],
            };
            out.push(Line::from(spans));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Inline {
    bold: bool,
    italic: bool,
    strike: bool,
    link: bool,
}

#[derive(Debug, Clone)]
enum ListKind {
    Bullet,
    Ordered(u64),
}

struct Builder<'t> {
    theme: &'t Theme,
    hl: &'t Highlighter,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    inline: Inline,
    lists: Vec<ListKind>,
    quote_depth: usize,
    code: Option<(String, String)>,
    heading: Option<HeadingLevel>,
    table: Option<Vec<Vec<String>>>,
    table_cell: Option<String>,
    /// Pending list-item prefix to emit at the start of the next text.
    item_prefix: Option<String>,
    need_blank: bool,
}

impl<'t> Builder<'t> {
    fn style(&self) -> Style {
        // Foreground only: the transcript area (or a BlockFrame surface) paints
        // the background, so markdown text can sit on any fill. Code spans set
        // their own bg and keep it inside a frame.
        let mut s = Style::default().fg(self.theme.canvas.fg.unwrap_or_default());
        if self.inline.bold {
            s = self.theme.strong(s);
        }
        if self.inline.italic {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.inline.strike {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.inline.link {
            s = s.patch(self.theme.link);
        }
        if let Some(level) = self.heading {
            s = s.patch(match level {
                HeadingLevel::H1 => self.theme.heading,
                HeadingLevel::H2 => self.theme.heading_h2,
                HeadingLevel::H3 => self.theme.heading_h3,
                _ => self.theme.heading,
            });
        }
        if self.quote_depth > 0 {
            s = s.patch(self.theme.quote);
        }
        s
    }

    fn indent(&self) -> String {
        let list_indent = self.lists.len().saturating_sub(1) * 2;
        let mut prefix = self.theme.glyph("│ ", "| ").repeat(self.quote_depth);
        prefix.push_str(&" ".repeat(list_indent));
        prefix
    }

    fn ensure_blank(&mut self) {
        if self.need_blank && !self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        self.need_blank = false;
    }

    fn push_text(&mut self, text: &str) {
        if let Some((_, buf)) = self.code.as_mut() {
            buf.push_str(text);
            return;
        }
        if let Some(cell) = self.table_cell.as_mut() {
            cell.push_str(text);
            return;
        }
        if self.current.is_empty() {
            self.ensure_blank();
            let mut prefix = self.indent();
            if let Some(p) = self.item_prefix.take() {
                prefix.push_str(&p);
            } else if !self.lists.is_empty() {
                // continuation line inside a list item
                prefix.push_str("  ");
            }
            if !prefix.is_empty() {
                self.current.push(Span::styled(prefix, self.theme.dim));
            }
            if let Some(level) = self.heading {
                let marker = match level {
                    HeadingLevel::H1 => "# ",
                    HeadingLevel::H2 => "## ",
                    _ => "### ",
                };
                self.current
                    .push(Span::styled(marker.to_string(), self.theme.accent));
            }
        }
        let style = self.style();
        self.current.push(Span::styled(text.to_string(), style));
    }

    fn flush_line(&mut self) {
        if !self.current.is_empty() {
            let spans = std::mem::take(&mut self.current);
            self.lines.push(Line::from(spans));
        }
    }

    fn flush_code(&mut self) {
        let Some((lang, code)) = self.code.take() else {
            return;
        };
        self.flush_line();
        self.ensure_blank();
        let label = if lang.is_empty() {
            "code"
        } else {
            lang.as_str()
        };
        let indent = self.indent();
        self.lines.push(Line::from(vec![
            Span::styled(indent.clone(), self.theme.dim),
            Span::styled(
                format!("{} {label} ", self.theme.glyph("╭─", "+-")),
                self.theme.dim,
            ),
        ]));
        let base = self.theme.code;
        for line in self
            .hl
            .highlight(code.trim_end_matches('\n'), &lang, base, self.theme)
        {
            let mut spans = vec![
                Span::styled(indent.clone(), self.theme.dim),
                Span::styled(self.theme.glyph("│ ", "| "), self.theme.dim),
            ];
            spans.extend(line.spans);
            self.lines.push(Line::from(spans));
        }
        self.lines.push(Line::from(vec![
            Span::styled(indent, self.theme.dim),
            Span::styled(self.theme.glyph("╰─", "+-"), self.theme.dim),
        ]));
        self.need_blank = true;
    }

    fn flush_table(&mut self) {
        let Some(rows) = self.table.take() else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        self.ensure_blank();
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; cols];
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.width());
            }
        }
        let indent = self.indent();
        for (ri, row) in rows.iter().enumerate() {
            let mut spans = vec![Span::styled(indent.clone(), self.theme.dim)];
            for (i, width) in widths.iter().enumerate() {
                let cell = row.get(i).cloned().unwrap_or_default();
                let padded = format!("{cell}{}", " ".repeat(width.saturating_sub(cell.width())));
                let style = if ri == 0 {
                    self.theme.emphasize(Style::default())
                } else {
                    Style::default()
                };
                spans.push(Span::styled(padded, style));
                if i + 1 < cols {
                    spans.push(Span::styled(self.theme.glyph(" │ ", " | "), self.theme.dim));
                }
            }
            self.lines.push(Line::from(spans));
            if ri == 0 {
                let rule: String = widths
                    .iter()
                    .map(|w| self.theme.glyph("─", "-").repeat(*w))
                    .collect::<Vec<_>>()
                    .join(self.theme.glyph("─┼─", "-+-"));
                self.lines.push(Line::from(vec![
                    Span::styled(indent.clone(), self.theme.dim),
                    Span::styled(rule, self.theme.dim),
                ]));
            }
        }
        self.need_blank = true;
    }
}

pub fn render_markdown(md: &str, theme: &Theme, hl: &Highlighter) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(md, options);
    let mut b = Builder {
        theme,
        hl,
        lines: Vec::new(),
        current: Vec::new(),
        inline: Inline::default(),
        lists: Vec::new(),
        quote_depth: 0,
        code: None,
        heading: None,
        table: None,
        table_cell: None,
        item_prefix: None,
        need_blank: false,
    };
    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph if b.lists.is_empty() => b.flush_line(),
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    b.flush_line();
                    b.need_blank = true;
                    b.heading = Some(level);
                }
                Tag::BlockQuote(_) => {
                    b.flush_line();
                    b.need_blank = true;
                    b.quote_depth += 1;
                }
                Tag::CodeBlock(kind) => {
                    b.flush_line();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => {
                            l.split_whitespace().next().unwrap_or("").to_string()
                        }
                        CodeBlockKind::Indented => String::new(),
                    };
                    b.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    b.flush_line();
                    if b.lists.is_empty() {
                        b.need_blank = true;
                    }
                    b.lists.push(match start {
                        Some(n) => ListKind::Ordered(n),
                        None => ListKind::Bullet,
                    });
                }
                Tag::Item => {
                    b.flush_line();
                    let prefix = match b.lists.last_mut() {
                        Some(ListKind::Ordered(n)) => {
                            let p = format!("{n}. ");
                            *n += 1;
                            p
                        }
                        _ => format!("{} ", theme.glyph("•", "*")),
                    };
                    b.item_prefix = Some(prefix);
                }
                Tag::Emphasis => b.inline.italic = true,
                Tag::Strong => b.inline.bold = true,
                Tag::Strikethrough => b.inline.strike = true,
                Tag::Link { .. } => b.inline.link = true,
                Tag::Table(_) => {
                    b.flush_line();
                    b.table = Some(Vec::new());
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = b.table.as_mut() {
                        t.push(Vec::new());
                    }
                }
                Tag::TableCell => b.table_cell = Some(String::new()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    b.flush_line();
                    if b.lists.is_empty() {
                        b.need_blank = true;
                    }
                }
                TagEnd::Heading(_) => {
                    b.flush_line();
                    b.heading = None;
                    b.need_blank = true;
                }
                TagEnd::BlockQuote(_) => {
                    b.flush_line();
                    b.quote_depth = b.quote_depth.saturating_sub(1);
                    b.need_blank = true;
                }
                TagEnd::CodeBlock => b.flush_code(),
                TagEnd::List(_) => {
                    b.flush_line();
                    b.lists.pop();
                    if b.lists.is_empty() {
                        b.need_blank = true;
                    }
                }
                TagEnd::Item => {
                    b.flush_line();
                    b.item_prefix = None;
                }
                TagEnd::Emphasis => b.inline.italic = false,
                TagEnd::Strong => b.inline.bold = false,
                TagEnd::Strikethrough => b.inline.strike = false,
                TagEnd::Link => b.inline.link = false,
                TagEnd::Table => b.flush_table(),
                TagEnd::TableCell => {
                    let cell = b.table_cell.take().unwrap_or_default();
                    if let Some(row) = b.table.as_mut().and_then(|t| t.last_mut()) {
                        row.push(cell.trim().to_string());
                    }
                }
                _ => {}
            },
            Event::Text(text) => b.push_text(&text),
            Event::Code(code) => {
                if b.table_cell.is_some() {
                    b.push_text(&code);
                } else {
                    if b.current.is_empty() {
                        b.push_text("");
                    }
                    b.current
                        .push(Span::styled(code.to_string(), theme.code));
                }
            }
            Event::SoftBreak => b.push_text(" "),
            Event::HardBreak => b.flush_line(),
            Event::Rule => {
                b.flush_line();
                b.ensure_blank();
                b.lines.push(Line::from(Span::styled(
                    theme.glyph("─", "-").repeat(40),
                    theme.dim,
                )));
                b.need_blank = true;
            }
            Event::TaskListMarker(done) => {
                let marker = if done { "[x] " } else { "[ ] " };
                b.item_prefix = Some(format!(
                    "{}{marker}",
                    b.item_prefix.take().unwrap_or_default()
                ));
            }
            Event::Html(html) | Event::InlineHtml(html) => b.push_text(&html),
            Event::FootnoteReference(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }
    b.flush_line();
    b.flush_code();
    b.flush_table();
    while matches!(b.lines.last(), Some(l) if l.spans.is_empty()) {
        b.lines.pop();
    }
    b.lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_text_stands_out_in_thin_mode_and_code_has_no_backticks() {
        let hl = Highlighter::new();
        let md = "**Bug:** see `wrap_overlong()` now";
        let thin = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode).with_thin(true);
        let lines = render_markdown(md, &thin, &hl);
        let bug = lines[0].spans.iter().find(|s| s.content.contains("Bug:")).unwrap();
        assert_eq!(bug.style.fg, thin.accent.fg, "thin strong uses the accent colour");
        assert!(!plain(&lines)[0].contains('`'), "{:?}", plain(&lines));
        let code = lines[0].spans.iter().find(|s| s.content == "wrap_overlong()").unwrap();
        assert_eq!(code.style.bg, thin.code.bg);

        let regular = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode).with_thin(false);
        let lines = render_markdown(md, &regular, &hl);
        let bug = lines[0].spans.iter().find(|s| s.content.contains("Bug:")).unwrap();
        assert!(bug.style.add_modifier.contains(Modifier::BOLD));
    }

    fn plain(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn renders_headings_lists_code_and_tables() {
        let hl = Highlighter::new();
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let md = "# Title\n\nSome *em* and **strong** and `code`.\n\n- one\n- two\n  1. nested\n\n```bash\necho hi\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let lines = plain(&render_markdown(md, &theme, &hl));
        assert_eq!(lines[0], "# Title");
        assert!(
            lines.iter().any(|l| l == "Some em and strong and code."),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l == "• one"));
        assert!(lines.iter().any(|l| l == "  1. nested"), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("╭─ bash")));
        assert!(lines.iter().any(|l| l == "│ echo hi"), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("a │ b")));
        assert!(lines.iter().any(|l| l.contains("1 │ 2")));
    }

    #[test]
    fn unterminated_fence_while_streaming_still_renders() {
        let hl = Highlighter::new();
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let lines = plain(&render_markdown(
            "text\n\n```rust\nfn main() {",
            &theme,
            &hl,
        ));
        assert!(lines.iter().any(|l| l.contains("fn main() {")), "{lines:?}");
    }

    #[test]
    fn blockquotes_and_task_lists() {
        let hl = Highlighter::new();
        let theme = Theme::nerv_with_icons(crate::render::theme::IconMode::Unicode);
        let lines = plain(&render_markdown(
            "> quoted\n\n- [x] done\n- [ ] todo\n",
            &theme,
            &hl,
        ));
        assert!(lines.iter().any(|l| l == "│ quoted"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "• [x] done"), "{lines:?}");
    }
}
