//! Persisted TUI preferences: last provider, model, session, composer
//! history. Lives under `$LAZAR_HOME/memory/lazartui/` (the Go TUI kept its
//! state in `memory/` too). Nothing here is security-relevant, which is why
//! an agent-writable directory is acceptable; access grants live elsewhere
//! (see `access.rs`).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PersistedState {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub theme: Option<String>,
    /// "nerd" | "unicode" | "ascii" (`/icons`). `None` defers to
    /// `LAZARTUI_ICONS` or auto-detection.
    pub icons: Option<String>,
    pub thin: bool,
    /// Debug/verbose view (Ctrl+A / `/debug`): hooks, internal kernel
    /// events, and full detail. Off by default — see `LAZARTUI_DEBUG`.
    pub debug: bool,
    pub composer_history: Vec<String>,
    /// Agents-screen rows the operator removed (Backspace/Delete); the
    /// jobs and session logs stay on disk.
    pub hidden_agents: Vec<String>,
}

pub fn state_path(lazar_home: &Path) -> PathBuf {
    lazar_home
        .join("user")
        .join("memory")
        .join("lazartui")
        .join("state.json")
}

impl PersistedState {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Atomic write (temp file + rename) so a crash mid-save cannot leave a
    /// truncated file behind.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_or_partial_files() {
        let dir = std::env::temp_dir().join(format!("lazartui-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = state_path(&dir);
        assert_eq!(PersistedState::load(&path), PersistedState::default());

        let state = PersistedState {
            provider: Some("kimi".into()),
            model: Some("kimi-k3".into()),
            session_id: Some("sess_1".into()),
            theme: Some("groknight".into()),
            icons: Some("ascii".into()),
            thin: true,
            debug: true,
            composer_history: vec!["hi".into()],
            hidden_agents: vec!["session:old".into()],
        };
        state.save(&path).unwrap();
        assert_eq!(PersistedState::load(&path), state);

        fs::write(&path, r#"{"provider":"minimax"}"#).unwrap();
        let partial = PersistedState::load(&path);
        assert_eq!(partial.provider.as_deref(), Some("minimax"));
        assert!(partial.composer_history.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
