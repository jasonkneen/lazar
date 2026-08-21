//! The provider catalog, read from the kernel (`lazar providers --json`) so
//! the TUI never carries its own table of endpoints or keys.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_env: String,
    pub key_available: bool,
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<String>,
}

/// Resolve the kernel binary: `LAZAR_BIN`, then `$LAZAR_HOME/bin/lazar`,
/// then `lazar` on PATH.
pub fn kernel_binary(lazar_home: &Path) -> PathBuf {
    if let Some(bin) = std::env::var_os("LAZAR_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(bin);
    }
    let local = lazar_home.join("bin").join("lazar");
    if local.is_file() {
        return local;
    }
    PathBuf::from("lazar")
}

pub fn load_catalog(kernel: &Path, lazar_home: &Path) -> Result<Vec<ProviderInfo>, String> {
    let output = Command::new(kernel)
        .args(["providers", "--json"])
        .env("LAZAR_HOME", lazar_home)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", kernel.display()))?;
    if !output.status.success() {
        return Err(format!(
            "`lazar providers --json` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_catalog(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_catalog(json: &str) -> Result<Vec<ProviderInfo>, String> {
    serde_json::from_str(json).map_err(|e| format!("bad provider catalog JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_kernel_shape() {
        let rows = parse_catalog(
            r#"[{"id":"minimax","name":"MiniMax","base_url":"https://api.minimax.io/anthropic","key_env":"MINIMAX_API_KEY","key_available":true,"default_model":"MiniMax-M3","models":["MiniMax-M3"]}]"#,
        )
        .unwrap();
        assert_eq!(rows[0].id, "minimax");
        assert!(rows[0].key_available);
        assert!(parse_catalog("nope").is_err());
    }
}
