use std::{
    cell::RefCell,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::RwLock,
};

pub(crate) const INHERITED_CWD_ENV: &str = "LAZAR_INHERITED_CWD";
pub(crate) const INHERITED_DIRS_ENV: &str = "LAZAR_INHERITED_ADD_DIRS";

static RUNTIME_ACCESS: RwLock<Option<AccessPolicy>> = RwLock::new(None);

thread_local! {
    /// A prompt-turn override for long-lived clients such as ACP. Keeping the
    /// override thread-local prevents one session from changing another
    /// session's in-flight cwd or write roots.
    static SCOPED_ACCESS: RefCell<Vec<AccessPolicy>> = const { RefCell::new(Vec::new()) };
}

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
                .unwrap_or_else(|| crate::user_dir("workspace")),
            None => crate::user_dir("workspace"),
        };
        let mut canonical_dirs = Vec::new();
        for path in allowed_write_dirs {
            let canonical = canonical_directory(path, "allowed directory")?;
            validate_scoped_root(&canonical, lazar_home)?;
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
                "ACCESS POLICY\n- Current working directory: {}\n- Sandbox: OFF.\n- Filesystem: normal operator access, including kernel and TUI source.\n- Durable learning belongs in user/memory and user/skills.",
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
    let mut slot = RUNTIME_ACCESS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.is_some() {
        return Err("runtime access policy was initialized twice".to_string());
    }
    *slot = Some(policy);
    Ok(())
}

pub(crate) struct ScopedAccess;

impl Drop for ScopedAccess {
    fn drop(&mut self) {
        SCOPED_ACCESS.with(|slot| {
            slot.borrow_mut()
                .pop()
                .expect("scoped access guards must be dropped in stack order");
        });
    }
}

/// Apply an access policy to the current prompt worker only. Scopes may nest;
/// dropping the guard restores the preceding policy.
pub(crate) fn scope(policy: AccessPolicy) -> ScopedAccess {
    SCOPED_ACCESS.with(|slot| slot.borrow_mut().push(policy));
    ScopedAccess
}

pub(crate) fn current() -> AccessPolicy {
    if let Some(policy) = SCOPED_ACCESS.with(|slot| slot.borrow().last().cloned()) {
        return policy;
    }
    if let Some(policy) = RUNTIME_ACCESS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return policy;
    }
    let lazar_home = std::env::var("LAZAR_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".to_string())).join("lazar")
        });
    AccessPolicy::new(false, None, &[], &lazar_home).unwrap_or(AccessPolicy {
        unsandboxed: false,
        tool_cwd: crate::user_dir("workspace"),
        allowed_write_dirs: Vec::new(),
    })
}

pub(crate) fn inherited() -> (Option<PathBuf>, Vec<PathBuf>) {
    decode_inherited(
        matches!(
            std::env::var("LAZAR_TOOL_ENV").ok().as_deref(),
            Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
        ),
        std::env::var_os(INHERITED_CWD_ENV),
        std::env::var(INHERITED_DIRS_ENV).ok().as_deref(),
    )
}

fn decode_inherited(
    from_lazar_tool: bool,
    cwd: Option<OsString>,
    encoded_dirs: Option<&str>,
) -> (Option<PathBuf>, Vec<PathBuf>) {
    if !from_lazar_tool {
        return (None, Vec::new());
    }
    let cwd = cwd.filter(|value| !value.is_empty()).map(PathBuf::from);
    let dirs = encoded_dirs
        .and_then(|value| serde_json::from_str::<Vec<PathBuf>>(value).ok())
        .unwrap_or_default();
    (cwd, dirs)
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

fn validate_scoped_root(path: &Path, lazar_home: &Path) -> Result<(), String> {
    let canonical_home = lazar_home
        .canonicalize()
        .unwrap_or_else(|_| lazar_home.to_path_buf());

    // A scoped project grant must never weaken Lazar's protected kernel state.
    // Existing agent-writable zones are harmless (and useful for workspace
    // projects); every other path inside Lazar remains protected. Ancestors of
    // LAZAR_HOME are rejected because their recursive grant would contain it.
    if canonical_home.starts_with(path) {
        return Err(format!(
            "allowed directory '{}' contains protected Lazar state; use --no-sandbox for explicit full access",
            path.display()
        ));
    }
    if path.starts_with(&canonical_home) {
        let in_existing_writable_zone = ["skills", "memory", "workspace"]
            .iter()
            .any(|zone| path.starts_with(canonical_home.join(zone)));
        if !in_existing_writable_zone {
            return Err(format!(
                "allowed directory '{}' overlaps protected Lazar state; use --no-sandbox for explicit full access",
                path.display()
            ));
        }
    }
    Ok(())
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

    #[test]
    fn scoped_roots_cannot_contain_or_enter_protected_lazar_state() {
        let temp = std::env::temp_dir().join(format!(
            "lazar-protected-root-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let lazar = temp.join("lazar");
        let workspace_project = lazar.join("workspace/project");
        let protected_src = lazar.join("src");
        std::fs::create_dir_all(&workspace_project).expect("workspace project");
        std::fs::create_dir_all(&protected_src).expect("protected src");

        let ancestor = AccessPolicy::new(
            false,
            None,
            std::slice::from_ref(&temp),
            &lazar,
        );
        assert!(ancestor.is_err(), "an ancestor grant would reopen all Lazar state");

        let protected = AccessPolicy::new(false, None, &[protected_src], &lazar);
        assert!(protected.is_err(), "src must remain protected from scoped grants");

        let workspace = AccessPolicy::new(
            false,
            None,
            std::slice::from_ref(&workspace_project),
            &lazar,
        )
        .expect("workspace projects remain valid scoped roots");
        assert_eq!(
            workspace.allowed_write_dirs(),
            &[workspace_project.canonicalize().expect("canonical workspace")]
        );
        std::fs::remove_dir_all(temp).expect("remove test tree");
    }

    #[test]
    fn nested_lazar_inherits_the_parent_cwd_and_scoped_roots() {
        let (cwd, dirs) = decode_inherited(
            true,
            Some(OsString::from("/tmp")),
            Some(r#"["/tmp"]"#),
        );
        let policy = AccessPolicy::new(
            false,
            cwd.as_deref(),
            &dirs,
            Path::new("/Users/Shared/lazar-test-home"),
        )
        .expect("inherited policy");

        let expected = Path::new("/tmp").canonicalize().expect("canonical /tmp");
        assert_eq!(policy.tool_cwd(), expected);
        assert_eq!(policy.allowed_write_dirs(), &[expected]);

        let ignored = decode_inherited(false, Some(OsString::from("/tmp")), Some(r#"["/tmp"]"#));
        assert_eq!(ignored, (None, Vec::new()));
    }

    #[test]
    fn prompt_access_scope_is_nested_and_thread_local() {
        let baseline = current();
        let outer = AccessPolicy {
            unsandboxed: false,
            tool_cwd: PathBuf::from("/outer"),
            allowed_write_dirs: vec![PathBuf::from("/outer/write")],
        };
        let inner = AccessPolicy {
            unsandboxed: true,
            tool_cwd: PathBuf::from("/inner"),
            allowed_write_dirs: Vec::new(),
        };

        let outer_guard = scope(outer.clone());
        assert_eq!(current(), outer);
        assert_eq!(std::thread::spawn(current).join().unwrap(), baseline);

        {
            let _inner_guard = scope(inner.clone());
            assert_eq!(current(), inner);
        }
        assert_eq!(current(), outer);
        drop(outer_guard);
        assert_eq!(current(), baseline);
    }
}
