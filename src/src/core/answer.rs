//! Structured answers: one typed record per finished answer, so clients
//! render a reply, speech, sources and verification without guessing.
//!
//! Opt-in per process (`--answer-format structured` / LAZAR_ANSWER_FORMAT).
//! The model still streams ordinary text; the contract only asks it to tag the
//! parts a client cannot infer:
//!
//! ```text
//! [SPEAK]
//! One or two sentences for voice.
//! [/SPEAK]
//! [SOURCES]
//! /abs/path/file.rs:42
//! https://example.com/page
//! [/SOURCES]
//! ```
//!
//! Everything the kernel already knows as fact — tool calls, verification,
//! compaction — stays kernel-generated and is never asked of the model.
//! File sources are checked like `exists=` claims, so a cited file that is not
//! there is reported, not trusted.

use serde_json::{json, Value};
use std::path::Path;

pub(crate) const SPEAK: &str = "SPEAK";
pub(crate) const SOURCES: &str = "SOURCES";

pub(crate) const PROMPT_SECTION: &str = "ANSWER FORMAT
This client renders structured answers. Write your reply as normal text, then
add these tagged blocks at the end when they apply:
[SPEAK]
One or two plain sentences a voice assistant would say: the outcome, no
markdown, no paths or code.
[/SPEAK]
[SOURCES]
One per line: a file you read or changed (absolute path, optional :line) or
a URL you used.
[/SOURCES]
Always include [SPEAK]. Include [SOURCES] only when you relied on specific
files or pages. Keep [VERIFY] as described above; it may sit alongside these.";

/// Whether this process emits structured answers.
pub(crate) fn enabled() -> bool {
    matches!(
        std::env::var("LAZAR_ANSWER_FORMAT").ok().as_deref().map(str::trim),
        Some("structured")
    )
}

fn open_tag(line: &str, tag: &str) -> bool {
    line.trim().eq_ignore_ascii_case(&format!("[{tag}]"))
}

fn close_tag(line: &str, tag: &str) -> bool {
    let line = line.trim();
    line.eq_ignore_ascii_case(&format!("[/{tag}]")) || line.eq_ignore_ascii_case(&format!("[/{tag}"))
}

/// Contents of every `[tag]` block (an unclosed block runs to the end).
pub(crate) fn blocks(text: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        match current.as_mut() {
            None if open_tag(line, tag) => current = Some(Vec::new()),
            None => {}
            Some(_) if close_tag(line, tag) => out.push(current.take().unwrap().join("\n")),
            Some(lines) => lines.push(line),
        }
    }
    if let Some(lines) = current {
        out.push(lines.join("\n"));
    }
    out
}

/// Incremental display filter. Only a possible tag prefix is buffered; raw
/// model text remains untouched for verification, speech and session storage.
#[derive(Default)]
pub(crate) struct DisplayText {
    pending: String,
    inside: Option<&'static str>,
}

impl DisplayText {
    pub(crate) fn push(&mut self, text: &str) -> String {
        let mut out = String::new();
        for ch in text.chars() {
            self.pending.push(ch);
            loop {
                let lower = self.pending.to_ascii_lowercase();
                if let Some(tag) = self.inside {
                    let close = format!("[/{tag}");
                    if lower == close || close.starts_with(&lower) {
                        break;
                    }
                    if lower.starts_with(&close) {
                        self.pending.drain(..close.len());
                        if self.pending.starts_with(']') {
                            self.pending.remove(0);
                        }
                        self.inside = None;
                        continue;
                    }
                } else {
                    let tags = [("verify", "[verify]"), ("speak", "[speak]"), ("sources", "[sources]")];
                    if let Some((tag, _)) = tags.iter().find(|(_, open)| *open == lower) {
                        self.inside = Some(tag);
                        self.pending.clear();
                        break;
                    }
                    if tags.iter().any(|(_, open)| open.starts_with(&lower)) {
                        break;
                    }
                }
                let Some(first) = self.pending.chars().next() else { break };
                if self.inside.is_none() {
                    out.push(first);
                }
                self.pending.drain(..first.len_utf8());
                if self.pending.is_empty() { break; }
            }
        }
        out
    }

    pub(crate) fn finish(&mut self) -> String {
        let tail = if self.inside.is_none() { std::mem::take(&mut self.pending) } else { String::new() };
        self.pending.clear();
        self.inside = None;
        tail
    }
}

/// Display-ready text for complete records, including inline machine blocks.
pub(crate) fn display(text: &str) -> String {
    let mut filter = DisplayText::default();
    let mut shown = filter.push(text);
    shown.push_str(&filter.finish());
    shown
}

fn clean(line: &str) -> &str {
    let line = line.trim();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
        .trim();
    line.trim_matches('`').trim()
}

/// One cited source, checked where it can be.
pub(crate) fn source(line: &str, cwd: &Path) -> Value {
    let reference = clean(line);
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return json!({"ref": reference, "kind": "url"});
    }
    // `path:42` or `path:42-60` cites a line; a bare colon stays in the path.
    let (path, line_no) = match reference.rsplit_once(':') {
        Some((path, suffix)) => {
            let first = suffix.split('-').next().unwrap_or("");
            match first.parse::<u64>() {
                Ok(n) if !path.is_empty() => (path, Some(n)),
                _ => (reference, None),
            }
        }
        None => (reference, None),
    };
    let resolved = super::verify::resolve(path, cwd);
    let exists = resolved.as_ref().is_ok_and(|p| p.exists());
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let title = resolved.as_ref().ok().filter(|_| exists).and_then(|p| title_of(p));
    let shown = resolved
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string());
    let mut value = json!({"ref": reference, "kind": "file", "path": shown, "name": name, "exists": exists});
    if let Some(title) = title {
        value["title"] = json!(title);
    }
    if let Some(n) = line_no {
        value["line"] = json!(n);
    }
    value
}

/// A document's own title: frontmatter `title:` (or `name:`), else the first
/// `# ` heading. Only markdown-like files, and only their first 8 KB.
fn title_of(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if !matches!(ext.as_str(), "md" | "markdown" | "mdx") {
        return None;
    }
    let mut head = vec![0u8; 8 * 1024];
    let read = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
    let text = String::from_utf8_lossy(&head[..read]);
    let clean = |s: &str| {
        let s = s.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        (!s.is_empty()).then(|| s.chars().take(120).collect::<String>())
    };
    let mut lines = text.lines();
    if text.starts_with("---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            for key in ["title:", "name:"] {
                if let Some(value) = line.trim().strip_prefix(key) {
                    if let Some(title) = clean(value) {
                        return Some(title);
                    }
                }
            }
        }
    }
    lines.find_map(|line| line.strip_prefix("# ").and_then(clean))
}

/// The structured record for one finished answer.
pub(crate) fn build(text: &str, cwd: &Path, verify: Value) -> Value {
    let speech = blocks(text, SPEAK)
        .iter()
        .map(|block| block.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut seen = std::collections::HashSet::new();
    let sources: Vec<Value> = blocks(text, SOURCES)
        .iter()
        .flat_map(|block| block.lines().map(str::to_string).collect::<Vec<_>>())
        .filter(|line| !clean(line).is_empty())
        .filter(|line| seen.insert(clean(line).to_string()))
        .map(|line| source(&line, cwd))
        .collect();
    json!({
        "reply": display(text).trim(),
        "speech": speech,
        "sources": sources,
        "verify": verify,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strips_inline_repeated_and_unclosed_blocks() {
        for (raw, expected) in [
            ("Done.\n[VERIFY]exists=/x[/VERIFY]\nMore.", "Done.\n\nMore."),
            ("a[verify]x[/verify]b[SPEAK]y[/SPEAK]c[SOURCES]z[/SOURCES]d", "abcd"),
            ("before\n[VERIFY]x[/VERIFY", "before\n"),
            ("before\n[speak]hi", "before\n"),
            ("before\n[SOURCES]/x[/SOURCES", "before\n"),
            ("[VERIFY]only hidden", ""),
            ("héllo [other] 世界 [SPE", "héllo [other] 世界 [SPE"),
        ] {
            assert_eq!(display(raw), expected);
            for split in raw.char_indices().map(|(i, _)| i).chain(std::iter::once(raw.len())) {
                let mut filter = DisplayText::default();
                let mut shown = filter.push(&raw[..split]);
                shown.push_str(&filter.push(&raw[split..]));
                shown.push_str(&filter.finish());
                assert_eq!(shown, expected, "split {split}: {raw}");
            }
        }
    }

    #[test]
    fn display_stream_never_leaks_partial_machine_tags() {
        let mut filter = DisplayText::default();
        let mut shown = String::new();
        for ch in "héllo[SPEAK]secret[/SPEAK]世界[VERIFY]claim[/VERIFY]".chars() {
            shown.push_str(&filter.push(&ch.to_string()));
            assert!("héllo世界".starts_with(&shown), "leaked {shown}");
        }
        shown.push_str(&filter.finish());
        assert_eq!(shown, "héllo世界");
        assert_eq!(filter.push("next"), "next", "finish resets state");
    }

    const ANSWER: &str = "Fixed the wrap bug.\n\n**Details** below.\n[SPEAK]\nFixed the wrap bug;\n  tests pass.\n[/SPEAK]\n[SOURCES]\n- `/etc/hosts:3`\nhttps://example.com/a\n/definitely/missing.rs\n/etc/hosts:3\n[/SOURCES]\n[VERIFY]\nexists=/etc/hosts\n[/VERIFY]\n";

    #[test]
    fn reply_speech_and_sources_are_separated() {
        let answer = build(ANSWER, Path::new("/"), json!({"status": "passed", "claims": 1}));
        assert_eq!(answer["reply"], "Fixed the wrap bug.\n\n**Details** below.");
        assert_eq!(answer["speech"], "Fixed the wrap bug; tests pass.");
        let sources = answer["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 3, "duplicates dropped: {sources:?}");
        assert_eq!(sources[0]["kind"], "file");
        assert_eq!(sources[0]["path"], "/etc/hosts");
        assert_eq!(sources[0]["line"], 3);
        assert_eq!(sources[0]["exists"], true);
        assert_eq!(sources[1], json!({"ref": "https://example.com/a", "kind": "url"}));
        assert_eq!(sources[2]["exists"], false);
        assert_eq!(answer["verify"]["status"], "passed");
    }

    #[test]
    fn markdown_sources_carry_their_title() {
        let dir = std::env::temp_dir().join(format!("lazar-answer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "---\ntitle: \"Flight Sims\"\n---\n# Other\n").unwrap();
        std::fs::write(dir.join("b.md"), "intro\n# Skills Index\nbody").unwrap();
        std::fs::write(dir.join("c.rs"), "# not a title").unwrap();
        let a = source(&dir.join("a.md").display().to_string(), &dir);
        assert_eq!(a["title"], "Flight Sims");
        assert_eq!(a["name"], "a.md");
        assert_eq!(source("b.md:3", &dir)["title"], "Skills Index");
        let c = source("c.rs", &dir);
        assert!(c.get("title").is_none());
        assert_eq!(c["name"], "c.rs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_text_is_all_reply_and_unclosed_blocks_still_strip() {
        let answer = build("just words", Path::new("/"), json!({"status": "none"}));
        assert_eq!(answer["reply"], "just words");
        assert_eq!(answer["speech"], "");
        assert_eq!(answer["sources"], json!([]));
        assert_eq!(display("hi\n[speak]\nsay hi").trim(), "hi");
        assert_eq!(blocks("hi\n[speak]\nsay hi\n[/SPEAK", SPEAK), vec!["say hi"]);
    }
}
