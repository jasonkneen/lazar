//! Skill discovery from `$LAZAR_HOME/skills/INDEX.md` for `/<skill>`
//! autocomplete. The kernel tells the model to read INDEX.md first, so the
//! same file is the TUI's source of truth.

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub summary: String,
}

pub fn load_index(lazar_home: &Path) -> Vec<SkillEntry> {
    fs::read_to_string(lazar_home.join("user").join("skills").join("INDEX.md"))
        .map(|s| parse_index(&s))
        .unwrap_or_default()
}

/// Accepts the shapes the index has used over time:
/// `- name — summary`, `- **name** — summary`, `- name: summary`,
/// `- [name](path) summary`, and `- \`name\` summary`.
pub fn parse_index(text: &str) -> Vec<SkillEntry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) else {
            continue;
        };
        let rest = rest.trim();
        let (name_part, summary) = split_name(rest);
        let name = name_part
            .trim()
            .trim_matches(|c| c == '*' || c == '`' || c == '[' || c == ']')
            .split('(')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || name.contains(' ') {
            continue;
        }
        if out.iter().any(|e: &SkillEntry| e.name == name) {
            continue;
        }
        out.push(SkillEntry {
            name,
            summary: summary.trim().to_string(),
        });
    }
    out
}

fn split_name(rest: &str) -> (&str, &str) {
    for sep in [" — ", " – ", ": ", " - ", "](", " "] {
        if let Some(idx) = rest.find(sep) {
            let (a, b) = rest.split_at(idx);
            let b = b.trim_start_matches(sep);
            let b = if sep == "](" {
                b.split_once(')').map(|(_, tail)| tail).unwrap_or("")
            } else {
                b
            };
            return (a, b);
        }
    }
    (rest, "")
}

/// Expand `/name rest` into the kernel's `[skill:name] rest` convention.
pub fn expand_skill_prompt(name: &str, rest: &str) -> String {
    if rest.trim().is_empty() {
        format!("[skill:{name}] Use this skill now.")
    } else {
        format!("[skill:{name}] {}", rest.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_index_shapes() {
        let idx = "# skills\n\n- memory — durable notes\n- **lazar-status**: show status\n- [web-fetch](web-fetch/SKILL.md) fetch pages\n- `todo` task list\n- memory — duplicate ignored\nnot a bullet\n";
        let entries = parse_index(idx);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["memory", "lazar-status", "web-fetch", "todo"]);
        assert_eq!(entries[0].summary, "durable notes");
        assert_eq!(entries[1].summary, "show status");
        assert_eq!(entries[2].summary, "fetch pages");
    }

    #[test]
    fn expands_skill_prompt() {
        assert_eq!(
            expand_skill_prompt("todo", ""),
            "[skill:todo] Use this skill now."
        );
        assert_eq!(
            expand_skill_prompt("todo", " add milk "),
            "[skill:todo] add milk"
        );
    }
}
