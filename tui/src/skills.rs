//! Skill discovery for `/<skill>` and `$<skill>` autocomplete.
//!
//! The kernel (`<lazar> skills --json`) is the source of truth: it knows
//! which skills are actually installed, the same set it resolves a leading
//! `$name` against. Parsing `$LAZAR_HOME/skills/INDEX.md` (the file the
//! kernel tells the model to read first) is kept only as a fallback for when
//! the kernel command isn't available yet or fails.

use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub summary: String,
}

#[derive(Debug, Deserialize)]
struct SkillRow {
    name: String,
    #[serde(default)]
    description: String,
}

/// `load_from_kernel`, falling back to `load_index` if the kernel command
/// fails (not built yet, older kernel without `skills --json`, and so on).
pub fn load(kernel: &Path, lazar_home: &Path) -> Vec<SkillEntry> {
    load_from_kernel(kernel, lazar_home).unwrap_or_else(|_| load_index(lazar_home))
}

pub fn load_from_kernel(kernel: &Path, lazar_home: &Path) -> Result<Vec<SkillEntry>, String> {
    let output = Command::new(kernel)
        .args(["skills", "--json"])
        .env("LAZAR_HOME", lazar_home)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", kernel.display()))?;
    if !output.status.success() {
        return Err(format!(
            "`lazar skills --json` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_kernel_skills(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_kernel_skills(json: &str) -> Result<Vec<SkillEntry>, String> {
    let rows: Vec<SkillRow> =
        serde_json::from_str(json).map_err(|e| format!("bad skills JSON: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|row| SkillEntry {
            name: row.name,
            summary: row.description,
        })
        .collect())
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
    fn parses_kernel_skill_rows_and_rejects_bad_json() {
        let rows = parse_kernel_skills(
            r#"[{"name":"_meta/harness","description":"harness notes"},{"name":"todo"}]"#,
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                SkillEntry { name: "_meta/harness".into(), summary: "harness notes".into() },
                SkillEntry { name: "todo".into(), summary: String::new() },
            ]
        );
        assert!(parse_kernel_skills("nope").is_err());
    }

    #[test]
    fn load_falls_back_to_the_index_when_the_kernel_command_fails() {
        let root = std::env::temp_dir().join(format!(
            "lazartui-skills-load-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("user").join("skills")).unwrap();
        fs::write(root.join("user/skills/INDEX.md"), "- todo — task list\n").unwrap();
        let entries = load(Path::new("/usr/bin/false"), &root);
        assert_eq!(entries, vec![SkillEntry { name: "todo".into(), summary: "task list".into() }]);
        fs::remove_dir_all(&root).unwrap();
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
