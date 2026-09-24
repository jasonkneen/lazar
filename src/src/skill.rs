//! Deterministic skill invocation.
//!
//! A client can ask for a skill with a directive at the start of the prompt:
//!
//!   [skill:<name>] <request>     the lazartui `/name` expansion
//!   /<name> <request>            an ACP slash command (Zed and friends)
//!   $<name> <request>            a skill mention (the lazartui `$` picker)
//!
//! The kernel resolves `$LAZAR_HOME/skills/<name>/SKILL.md` itself and puts
//! its contents into the user message. The model never has to know a naming
//! convention, guess a path, or spend a tool call to find out what the
//! operator asked for — the instructions are simply there. A misspelled
//! `[skill:…]` is an error back to the client, not a prompt to the model.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{core::context::MAX_SKILL_BODY_BYTES, text::head_bytes};

/// Larger skills remain available on disk; only a bounded prefix is injected
/// into one model-visible item, with an instruction to read the rest on demand.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocation {
    pub name: String,
    pub path: PathBuf,
    pub body: String,
    pub truncated: bool,
    /// What the operator wants done with the skill; defaults to a plain
    /// "use it" when the directive stands alone.
    pub request: String,
}

impl SkillInvocation {
    /// The user message the model actually receives.
    pub fn prompt(&self) -> String {
        let mut out = format!(
            "The operator invoked the skill `{}`. Its SKILL.md is included below; follow it now using the bash tool.\n\n--- {} ---\n{}",
            self.name,
            self.path.display(),
            self.body.trim_end(),
        );
        if self.truncated {
            out.push_str(&format!(
                "\n[truncated at {MAX_SKILL_BODY_BYTES} bytes — read the file for the rest]"
            ));
        }
        out.push_str(&format!(
            "\n--- end of SKILL.md ---\n\nOperator request: {}",
            self.request
        ));
        out
    }
}

/// A skill the kernel can advertise (ACP `available_commands_update`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
}

/// Split a leading directive off the prompt. `Some((name, rest))` for
/// `[skill:name] rest` and `/name rest`; `None` for an ordinary prompt.
/// Which form it was matters to the caller: an explicit `[skill:…]` for an
/// unknown skill is an error, while `/word` that is not a skill is just text.
pub fn parse_directive(prompt: &str) -> Option<(Directive, String, String)> {
    let trimmed = prompt.trim_start();
    if let Some(after) = trimmed.strip_prefix("[skill:") {
        let close = after.find(']')?;
        let name = after[..close].trim();
        let rest = after[close + 1..].trim();
        return Some((Directive::Explicit, name.to_string(), rest.to_string()));
    }
    // `$name` behaves like `/name`: a skill only when one is installed, so
    // `$HOME is…` or `$5` stays ordinary text.
    if let Some(after) = trimmed.strip_prefix('/').or_else(|| trimmed.strip_prefix('$')) {
        let (name, rest) = match after.find(char::is_whitespace) {
            Some(i) => (&after[..i], after[i..].trim()),
            None => (after, ""),
        };
        if !name.is_empty() {
            return Some((Directive::Slash, name.to_string(), rest.to_string()));
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    /// `[skill:name]` — the client asked for this skill by name.
    Explicit,
    /// `/name` or `$name` — only a skill if one by that name exists.
    Slash,
}

/// One or two path segments (`memory`, `_meta/harness`), each starting with
/// an alphanumeric and containing only `[A-Za-z0-9_.-]`. Rejects anything
/// that could escape `skills/`.
pub fn valid_name(name: &str) -> bool {
    let segments: Vec<&str> = name.split('/').collect();
    if segments.is_empty() || segments.len() > 2 {
        return false;
    }
    segments.iter().all(|seg| {
        !seg.is_empty()
            && seg != &"."
            && seg != &".."
            && seg
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    })
}

fn skill_file(skills_root: &Path, name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    let path = skills_root.join(name).join("SKILL.md");
    if !path.is_file() {
        return None;
    }
    // Belt and braces: the resolved file must live under skills/.
    let root = fs::canonicalize(skills_root).ok()?;
    let real = fs::canonicalize(&path).ok()?;
    real.starts_with(&root).then_some(path)
}

/// Resolve a directive against the installed skills.
///
/// - `Ok(None)` — no directive, or `/word` that is not a skill: send the
///   prompt to the model unchanged.
/// - `Ok(Some(inv))` — a skill was found; send `inv.prompt()` instead.
/// - `Err(msg)` — an explicit `[skill:name]` named a skill that does not
///   exist or cannot be read. The caller reports this to the client; the
///   model is never asked to guess.
pub fn resolve(prompt: &str, skills_root: &Path) -> Result<Option<SkillInvocation>, String> {
    let Some((kind, name, rest)) = parse_directive(prompt) else {
        return Ok(None);
    };
    let Some(path) = skill_file(skills_root, &name) else {
        return match kind {
            Directive::Slash => Ok(None),
            Directive::Explicit => {
                let mut installed: Vec<String> = list(skills_root).into_iter().map(|s| s.name).collect();
                installed.sort();
                Err(format!(
                    "unknown skill `{name}` (no {}/{name}/SKILL.md); installed: {}",
                    skills_root.display(),
                    if installed.is_empty() { "none".to_string() } else { installed.join(", ") }
                ))
            }
        };
    };
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read skill `{name}` at {}: {e}", path.display()))?;
    let truncated = raw.len() > MAX_SKILL_BODY_BYTES;
    let body = head_bytes(&raw, MAX_SKILL_BODY_BYTES).to_string();
    let request = if rest.is_empty() {
        "Use this skill now.".to_string()
    } else {
        rest
    };
    Ok(Some(SkillInvocation {
        name,
        path,
        body,
        truncated,
        request,
    }))
}

/// Every installed skill: `skills/<name>/SKILL.md` plus `skills/_meta/<name>/SKILL.md`.
/// Description is the first non-empty, non-heading line of the file.
pub fn list(skills_root: &Path) -> Vec<SkillSummary> {
    let mut out = Vec::new();
    collect(skills_root, "", &mut out);
    collect(&skills_root.join("_meta"), "_meta/", &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<SkillSummary>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else { continue };
        if !valid_name(&file_name) || file_name.starts_with('_') && prefix.is_empty() {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        let Ok(text) = fs::read_to_string(&skill_md) else { continue };
        out.push(SkillSummary {
            name: format!("{prefix}{file_name}"),
            description: first_prose_line(&text),
        });
    }
}

fn first_prose_line(text: &str) -> String {
    let mut in_frontmatter = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if i == 0 && line == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line == "---" {
                in_frontmatter = false;
            } else if let Some(desc) = line.strip_prefix("description:") {
                return short(desc.trim().trim_matches('"'));
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with("```") {
            continue;
        }
        return short(line);
    }
    String::new()
}

fn short(s: &str) -> String {
    const MAX: usize = 120;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(MAX).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dollar_mentions_a_skill_but_leaves_shell_text_alone() {
        assert_eq!(
            parse_directive("$memory note this"),
            Some((Directive::Slash, "memory".into(), "note this".into()))
        );
        let root = std::env::temp_dir().join(format!("lazar-dollar-{}", std::process::id()));
        fs::create_dir_all(root.join("memory")).unwrap();
        fs::write(root.join("memory/SKILL.md"), "# memory\nKeep notes.").unwrap();
        assert_eq!(resolve("$memory note this", &root).unwrap().unwrap().name, "memory");
        assert!(resolve("$HOME is where?", &root).unwrap().is_none());
        assert!(resolve("$5 is cheap", &root).unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    use super::*;

    fn fixture() -> PathBuf {
        // Tests run in parallel; a millisecond timestamp alone collides and
        // one test's cleanup deletes another's files.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "lazar-skill-test-{}-{}-{}",
            std::process::id(),
            crate::now_millis(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("lazar-status")).unwrap();
        fs::write(
            root.join("lazar-status/SKILL.md"),
            "# lazar-status\n\nFast system status snapshot.\n\nRun `bash workspace/lazar-status.sh`.\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("_meta/harness")).unwrap();
        fs::write(
            root.join("_meta/harness/SKILL.md"),
            "---\nname: harness\ndescription: \"Continual harness of ranked lessons.\"\n---\n# harness\n\nbody\n",
        )
        .unwrap();
        // A directory without SKILL.md is not a skill.
        fs::create_dir_all(root.join("not-a-skill")).unwrap();
        root
    }

    #[test]
    fn explicit_directive_inlines_the_skill_body() {
        let root = fixture();
        let inv = resolve("[skill:lazar-status] Use this skill now.", &root)
            .unwrap()
            .expect("skill found");
        assert_eq!(inv.name, "lazar-status");
        assert_eq!(inv.request, "Use this skill now.");
        assert!(!inv.truncated);
        let prompt = inv.prompt();
        assert!(prompt.contains("invoked the skill `lazar-status`"));
        assert!(prompt.contains("Run `bash workspace/lazar-status.sh`."));
        assert!(prompt.ends_with("Operator request: Use this skill now."));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn slash_form_and_meta_skills_resolve() {
        let root = fixture();
        let inv = resolve("/lazar-status what's up", &root).unwrap().unwrap();
        assert_eq!(inv.request, "what's up");
        let inv = resolve("[skill:_meta/harness]", &root).unwrap().unwrap();
        assert_eq!(inv.name, "_meta/harness");
        assert_eq!(inv.request, "Use this skill now.");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_explicit_skill_is_an_error_but_unknown_slash_is_plain_text() {
        let root = fixture();
        let err = resolve("[skill:nope] go", &root).unwrap_err();
        assert!(err.contains("unknown skill `nope`"));
        assert!(err.contains("lazar-status"), "lists installed skills: {err}");
        // `/tmp/foo is broken` is a sentence about a path, not a command.
        assert_eq!(resolve("/tmp/foo is broken", &root).unwrap(), None);
        assert_eq!(resolve("hello", &root).unwrap(), None);
        // A directory with no SKILL.md is not a skill.
        assert!(resolve("[skill:not-a-skill]", &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn names_cannot_escape_the_skills_root() {
        for bad in ["../etc", "a/../../b", "/abs", "a/b/c", "", ".", "..", "a b", "a;rm"] {
            assert!(!valid_name(bad), "{bad:?} must be rejected");
        }
        for good in ["memory", "_meta/harness", "lazar-status", "web.fetch_v2"] {
            assert!(valid_name(good), "{good:?} must be accepted");
        }
        let root = fixture();
        assert!(resolve("[skill:../lazar-status]", &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_reports_installed_skills_with_descriptions() {
        let root = fixture();
        let skills = list(&root);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["_meta/harness", "lazar-status"]);
        assert_eq!(skills[0].description, "Continual harness of ranked lessons.");
        assert_eq!(skills[1].description, "Fast system status snapshot.");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_bodies_are_capped_and_flagged() {
        let root = fixture();
        fs::create_dir_all(root.join("huge")).unwrap();
        fs::write(root.join("huge/SKILL.md"), "x".repeat(MAX_SKILL_BODY_BYTES + 10)).unwrap();
        let inv = resolve("[skill:huge]", &root).unwrap().unwrap();
        assert!(inv.truncated);
        assert_eq!(inv.body.len(), MAX_SKILL_BODY_BYTES);
        assert!(inv.prompt().contains("[truncated at"));
        let _ = fs::remove_dir_all(&root);
    }
}
