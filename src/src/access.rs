use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

static RUNTIME_ACCESS: OnceLock<AccessPolicy> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessPolicy {
    unsandboxed: bool,
    tool_cwd: PathBuf,
    allowed_write_dirs: Vec<PathBuf>,
}

impl AccessPolicy {
    pub(crate) fn new(
        unsandboxed: bool,
        explicit_cwd: Option<&Path>,
        allowed_write_dirs: &[PathBuf],
        lazar_home: &Path,
    ) -> Result<Self, String> {
        let tool_cwd = match explicit_cwd {
            Some(path) => canonical_directory(path, "working directory")?,
            None if unsandboxed => std::env::var("PWD")
                .ok()
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| lazar_home.join("workspace")),
            None => lazar_home.join("workspace"),
        };
        let mut canonical_dirs = Vec::new();
        for path in allowed_write_dirs {
            let canonical = canonical_directory(path, "allowed directory")?;
            let display = canonical.to_str().ok_or_else(|| {
                format!(
                    "allowed directory is not valid UTF-8: {}",
                    canonical.display()
                )
            })?;
            if display.contains(['\n', '\r']) {
                return Err(format!(
                    "allowed directory contains a line break: {}",
                    canonical.display()
                ));
            }
            if !canonical_dirs.contains(&canonical) {
                canonical_dirs.push(canonical);
            }
        }
        Ok(Self {
            unsandboxed,
            tool_cwd,
            allowed_write_dirs: canonical_dirs,
        })
    }

    pub(crate) fn unsandboxed(&self) -> bool {
        self.unsandboxed
    }

    pub(crate) fn tool_cwd(&self) -> &Path {
        &self.tool_cwd
    }

    pub(crate) fn allowed_write_dirs(&self) -> &[PathBuf] {
        &self.allowed_write_dirs
    }

    pub(crate) fn prompt_context(&self) -> String {
        if self.unsandboxed {
            return format!(
                "ACCESS POLICY\n- Current working directory: {}\n- Sandbox: OFF; operator-level filesystem access is active.",
                self.tool_cwd.display()
            );
        }
        let mut lines = vec![
            "ACCESS POLICY".to_string(),
            format!("- Current working directory: {}", self.tool_cwd.display()),
            "- Sandbox: ON.".to_string(),
        ];
        if self.allowed_write_dirs.is_empty() {
            lines.push("- Additional operator-approved write roots: none.".to_string());
        } else {
            lines.push("- Additional operator-approved write roots:".to_string());
            lines.extend(
                self.allowed_write_dirs
                    .iter()
                    .map(|path| format!("  - {}", path.display())),
            );
        }
        lines.join("\n")
    }
}

pub(crate) fn init(policy: AccessPolicy) -> Result<(), String> {
    RUNTIME_ACCESS
        .set(policy)
        .map_err(|_| "runtime access policy was initialized twice".to_string())
}

pub(crate) fn current() -> &'static AccessPolicy {
    RUNTIME_ACCESS.get_or_init(|| {
        let lazar_home = std::env::var("LAZAR_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".to_string()))
                    .join("lazar")
            });
        AccessPolicy::new(false, None, &[], &lazar_home).unwrap_or(AccessPolicy {
            unsandboxed: false,
            tool_cwd: lazar_home.join("workspace"),
            allowed_write_dirs: Vec::new(),
        })
    })
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|err| format!("invalid {label} '{}': {err}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "invalid {label} '{}': not a directory",
            path.display()
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_cwd_is_canonicalized_and_used_for_tools() {
        let expected = Path::new("/tmp").canonicalize().expect("canonical /tmp");
        let policy = AccessPolicy::new(false, Some(Path::new("/tmp")), &[], Path::new("/lazar"))
            .expect("valid policy");

        assert_eq!(policy.tool_cwd(), expected);
    }

    #[test]
    fn allowed_write_dirs_are_canonicalized_and_deduplicated() {
        let expected = Path::new("/tmp").canonicalize().expect("canonical /tmp");
        let policy = AccessPolicy::new(
            false,
            None,
            &[PathBuf::from("/tmp"), expected.clone()],
            Path::new("/lazar"),
        )
        .expect("valid policy");

        assert_eq!(policy.allowed_write_dirs(), &[expected]);
    }
}
