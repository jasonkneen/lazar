//! First run from a bare binary: everything `setup.sh` seeds, created from
//! copies compiled into the kernel, so a release can be just `bin/lazar` and
//! `bin/lazartui`.
//!
//! The one rule: only ever CREATE what is missing. Nothing existing is
//! overwritten, moved, or deleted — skills, hooks, memory, workspace, logs,
//! guardrails, providers.toml and installed helpers are all left exactly as
//! they are once present.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Helpers setup.sh installs into bin/ (trusted, outside the sandbox's write
/// paths), compiled in from scripts/.
const HELPERS: &[(&str, &str)] = &[
    ("run-with-timeout.py", include_str!("../helpers/run-with-timeout.py")),
    ("spawn-supervisor.py", include_str!("../helpers/spawn-supervisor.py")),
    ("lazar-spawn", include_str!("../helpers/lazar-spawn.sh")),
    ("lazar-failover", include_str!("../helpers/lazar-failover.sh")),
];

/// What a call created, for the one-line startup note.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Seeded {
    pub created: Vec<String>,
}

fn note(seeded: &mut Seeded, what: impl Into<String>) {
    seeded.created.push(what.into());
}

/// Write `contents` to `path` only if nothing is there. `create_new` makes the
/// check and the write one atomic step, so a racing process cannot be clobbered.
fn create_file(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<bool> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path) {
        Ok(mut file) => {
            file.write_all(contents)?;
            Ok(true)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

/// Extract an embedded tree into `dest`, creating only files that are absent.
fn extract_missing(dir: &include_dir::Dir, dest: &Path) -> std::io::Result<usize> {
    let mut created = 0;
    for file in dir.files() {
        let mode = if file.path().extension().is_some_and(|e| e == "sh") { 0o755 } else { 0o644 };
        if create_file(&dest.join(file.path()), file.contents(), mode)? {
            created += 1;
        }
    }
    for sub in dir.dirs() {
        created += extract_missing(sub, dest)?;
    }
    Ok(created)
}

/// Seed whatever is missing under `home` (normally `$LAZAR_HOME`).
pub(crate) fn ensure(
    home: &Path,
    skills: &include_dir::Dir,
    default_catalog: &str,
) -> std::io::Result<Seeded> {
    let mut seeded = Seeded::default();
    let user = home.join("user");
    fs::create_dir_all(&user)?;

    // Skills: a whole tree only for a brand-new install (no index anywhere);
    // an existing, evolved tree is never topped up file by file.
    let has_index = user.join("skills/INDEX.md").is_file() || home.join("skills/INDEX.md").is_file();
    if !has_index {
        let n = extract_missing(skills, &user.join("skills"))?;
        note(&mut seeded, format!("user/skills ({n} files)"));
    }
    // Trusted helpers point at this skill's usage contract.
    if let Some(failover) = skills.get_dir("provider-failover") {
        if !user.join("skills/provider-failover").exists() && !home.join("skills/provider-failover").exists() {
            extract_missing(failover, &user.join("skills"))?;
            note(&mut seeded, "user/skills/provider-failover");
        }
    }

    for name in ["memory", "workspace", "logs"] {
        let dir = user.join(name);
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
            note(&mut seeded, format!("user/{name}/"));
        }
    }

    // Absent guardrails means sandbox off; an existing choice is kept.
    if !home.join(".guardrails").exists() && create_file(&user.join(".guardrails"), b"off\n", 0o600)? {
        note(&mut seeded, "user/.guardrails (off)");
    }

    if !home.join("providers.toml").exists()
        && create_file(&user.join("providers.toml"), default_catalog.as_bytes(), 0o644)?
    {
        note(&mut seeded, "user/providers.toml");
    }

    for (name, source) in HELPERS {
        if create_file(&home.join("bin").join(name), source.as_bytes(), 0o555)? {
            note(&mut seeded, format!("bin/{name}"));
        }
    }
    Ok(seeded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lazar-firstrun-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_bare_home_gets_everything_setup_would_seed() {
        let root = home("bare");
        let seeded = ensure(&root, &crate::SEED_SKILLS, "# catalog\n").unwrap();
        assert!(root.join("user/skills/INDEX.md").is_file());
        assert!(root.join("user/skills/provider-failover").is_dir());
        for dir in ["memory", "workspace", "logs"] {
            assert!(root.join("user").join(dir).is_dir(), "{dir}");
        }
        assert_eq!(fs::read_to_string(root.join("user/.guardrails")).unwrap(), "off\n");
        assert_eq!(fs::read_to_string(root.join("user/providers.toml")).unwrap(), "# catalog\n");
        let spawn = root.join("bin/lazar-spawn");
        assert_eq!(fs::metadata(&spawn).unwrap().permissions().mode() & 0o777, 0o555);
        assert!(!seeded.created.is_empty());
        // Idempotent: a second run creates nothing.
        assert_eq!(ensure(&root, &crate::SEED_SKILLS, "# catalog\n").unwrap(), Seeded::default());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn existing_data_is_never_touched() {
        let root = home("existing");
        let user = root.join("user");
        fs::create_dir_all(user.join("skills/mine")).unwrap();
        fs::write(user.join("skills/INDEX.md"), "my evolved index").unwrap();
        fs::write(user.join("skills/mine/SKILL.md"), "my skill").unwrap();
        fs::create_dir_all(user.join("memory")).unwrap();
        fs::write(user.join("memory/PINNED.md"), "my memory").unwrap();
        fs::write(user.join(".guardrails"), "on\n").unwrap();
        fs::write(user.join("providers.toml"), "my providers").unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/lazar-spawn"), "my patched helper").unwrap();

        ensure(&root, &crate::SEED_SKILLS, "# catalog\n").unwrap();

        assert_eq!(fs::read_to_string(user.join("skills/INDEX.md")).unwrap(), "my evolved index");
        assert_eq!(fs::read_to_string(user.join("skills/mine/SKILL.md")).unwrap(), "my skill");
        assert!(!user.join("skills/memory").exists(), "an evolved skill tree is not topped up");
        assert_eq!(fs::read_to_string(user.join("memory/PINNED.md")).unwrap(), "my memory");
        assert_eq!(fs::read_to_string(user.join(".guardrails")).unwrap(), "on\n");
        assert_eq!(fs::read_to_string(user.join("providers.toml")).unwrap(), "my providers");
        assert_eq!(fs::read_to_string(root.join("bin/lazar-spawn")).unwrap(), "my patched helper");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_legacy_top_level_catalog_or_guardrails_counts_as_present() {
        let root = home("legacy");
        fs::write(root.join("providers.toml"), "legacy").unwrap();
        fs::write(root.join(".guardrails"), "on\n").unwrap();
        ensure(&root, &crate::SEED_SKILLS, "# catalog\n").unwrap();
        assert!(!root.join("user/providers.toml").exists());
        assert!(!root.join("user/.guardrails").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
