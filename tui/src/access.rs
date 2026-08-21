//! Scoped write access for the kernel's sandbox.
//!
//! "Always" grants persist in `$LAZAR_HOME/access-policy.json` (the same file
//! and shape the Go TUI used, so existing approvals carry over). It lives at
//! the LAZAR_HOME root on purpose: model tools can write `memory/` and
//! `workspace/`, so a grant file there could be forged by the agent. Session
//! grants live only in memory. Both are passed to the kernel as additional
//! directories on `session/new`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessPolicy {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub always_dirs: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub always_everywhere: bool,
}

pub fn policy_path(lazar_home: &Path) -> PathBuf {
    lazar_home.join("user").join("access-policy.json")
}

impl AccessPolicy {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Always,
    Session,
}

#[derive(Debug, Default)]
pub struct AccessState {
    pub policy: AccessPolicy,
    pub session_dirs: Vec<PathBuf>,
    /// Directories passed on the command line for this launch.
    pub launch_dirs: Vec<PathBuf>,
}

impl AccessState {
    pub fn grant(&mut self, dir: PathBuf, scope: Scope) {
        let canonical = dir.canonicalize().unwrap_or(dir);
        match scope {
            Scope::Always => {
                let s = canonical.to_string_lossy().to_string();
                if !self.policy.always_dirs.contains(&s) {
                    self.policy.always_dirs.push(s);
                }
            }
            Scope::Session => {
                if !self.session_dirs.contains(&canonical) {
                    self.session_dirs.push(canonical);
                }
            }
        }
    }

    pub fn reset_session(&mut self) {
        self.session_dirs.clear();
    }

    /// Every directory the next kernel session should be allowed to write.
    pub fn additional_directories(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for d in self
            .launch_dirs
            .iter()
            .cloned()
            .chain(self.policy.always_dirs.iter().map(PathBuf::from))
            .chain(self.session_dirs.iter().cloned())
        {
            let canonical = d.canonicalize().unwrap_or(d);
            if !out.contains(&canonical) {
                out.push(canonical);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_file_matches_the_go_tui_shape() {
        let policy: AccessPolicy =
            serde_json::from_str(r#"{"alwaysDirs":["/a"],"alwaysEverywhere":false}"#).unwrap();
        assert_eq!(policy.always_dirs, vec!["/a".to_string()]);
        let text = serde_json::to_string(&policy).unwrap();
        assert_eq!(text, r#"{"alwaysDirs":["/a"]}"#);
    }

    #[test]
    fn grants_dedupe_and_merge_in_order() {
        let mut state = AccessState::default();
        state.launch_dirs.push(PathBuf::from("/tmp"));
        state.grant(PathBuf::from("/tmp"), Scope::Session);
        state.grant(PathBuf::from("/nonexistent-dir-xyz"), Scope::Always);
        state.grant(PathBuf::from("/nonexistent-dir-xyz"), Scope::Always);
        let dirs = state.additional_directories();
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert_eq!(state.policy.always_dirs.len(), 1);
        state.reset_session();
        assert!(state.session_dirs.is_empty());
    }
}
