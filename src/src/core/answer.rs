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
const VERIFY: &str = "VERIFY";

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

/// `text` without any of the tagged blocks, trimmed.
pub(crate) fn strip(text: &str, tags: &[&str]) -> String {
    let mut out = Vec::new();
    let mut inside: Option<&str> = None;
    for line in text.lines() {
        match inside {
            Some(tag) => {
                if close_tag(line, tag) {
                    inside = None;
                }
            }
            None => match tags.iter().find(|tag| open_tag(line, tag)) {
                Some(tag) => inside = Some(tag),
                None => out.push(line),
            },
        }
    }
    out.join("\n").trim().to_string()
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
        "reply": strip(text, &[SPEAK, SOURCES, VERIFY]),
        "speech": speech,
        "sources": sources,
        "verify": verify,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(strip("hi\n[speak]\nsay hi", &[SPEAK]), "hi");
        assert_eq!(blocks("hi\n[speak]\nsay hi\n[/SPEAK", SPEAK), vec!["say hi"]);
    }
}
