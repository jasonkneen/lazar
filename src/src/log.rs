//! stream-log machinery: append, rotation, pruning.

use serde_json::{json, Value};
use std::{
    cell::Cell,
    env, fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};

use crate::hooks::{self, HookEvent};
use crate::session;
use crate::{lazar_home, now_millis};

thread_local! {
    static IN_LOG_ROTATION_HOOK: Cell<bool> = const { Cell::new(false) };
    static SESSION_WRITE_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The first session-log failure in this turn, if a mirrored write did not commit.
pub(crate) fn take_session_write_error() -> Option<String> {
    SESSION_WRITE_ERROR.with(|slot| slot.borrow_mut().take())
}

fn note_session_write_error(message: String) {
    SESSION_WRITE_ERROR.with(|slot| {
        let mut current = slot.borrow_mut();
        if current.is_none() {
            *current = Some(message);
        }
    });
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn unique_log_archive_path(logs: &Path) -> PathBuf {
    let pid = std::process::id();
    for attempt in 0..1000u32 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!(".{attempt}")
        };
        let candidate = logs.join(format!("stream.jsonl.{}.{pid}{suffix}.bak", now_nanos()));
        if !candidate.exists() {
            return candidate;
        }
    }

    logs.join(format!("stream.jsonl.{}.{pid}.fallback.bak", now_nanos()))
}

#[cfg(unix)]
fn lock_file_exclusive(file: &File) {
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_EX);
    }
}

#[cfg(not(unix))]
fn lock_file_exclusive(_file: &File) {}

fn ensure_dir_not_symlink(path: &Path, label: &str) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to use symlinked {label}"),
            ));
        }
    }
    fs::create_dir_all(path)?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to use symlinked {label}"),
        ));
    }
    Ok(())
}

fn open_append_no_follow(path: &Path, label: &str) -> std::io::Result<File> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to append to symlinked {label}"),
            ));
        }
    }

    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

fn write_new_no_follow(path: &Path, label: &str, contents: &str) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing to write symlinked {label}"),
            ));
        }
    }

    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())
}

/// Auto-rotate the stream log when it exceeds LAZAR_LOG_MAX_BYTES (default 10MB).
/// The current log is moved to a unique stream.jsonl.<unix-nanos>.<pid>.bak
/// archive and a minimal summary is written into memory/log-summaries/ so the
/// agent has a navigable index of past archives. Skills like _meta/log-rotation
/// can layer richer summaries on top. After rotation hooks run, the kernel prunes
/// old *harvested* archives according to LAZAR_LOG_ARCHIVE_KEEP (default 5,
/// 0 disables). Unharvested archives are never deleted — harvest first
/// (memory/.harvested), then prune keeps disk bounded.
/// Result of a successful log rotation. Returned to the caller so it can
/// fire the `log-rotation` hook AFTER releasing the stream lock — firing
/// inside the lock would deadlock because hooks themselves write hook_start
/// / hook_end events through append_stream.
struct RotationInfo {
    archive: PathBuf,
    size_bytes: u64,
    summary: Option<PathBuf>,
}

fn maybe_rotate_log(logs: &Path, path: &Path) -> Option<RotationInfo> {
    let max_bytes: u64 = env::var("LAZAR_LOG_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_485_760); // 10 MB

    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return None, // file doesn't exist yet — nothing to rotate
    };
    if meta.file_type().is_symlink() {
        eprintln!("[lazar] WARN: refusing to rotate symlinked stream.jsonl");
        return None;
    }

    let size = meta.len();
    if size < max_bytes {
        return None;
    }

    let archive = unique_log_archive_path(logs);
    if fs::rename(path, &archive).is_err() {
        eprintln!("[lazar] WARN: log rotation rename failed; log will keep growing");
        return None;
    }
    eprintln!(
        "[lazar] auto-rotated stream.jsonl → {} ({size} bytes)",
        archive
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archive")
    );

    // Write a minimal summary into memory/log-summaries/ so the agent has
    // an index of past archives without re-reading them. _meta/log-rotation
    // and _meta/distill can layer richer per-archive summaries on top.
    let memory_root = logs
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("memory");
    let memory = memory_root.join("log-summaries");
    let mut summary_path: Option<PathBuf> = None;
    if let Err(e) = ensure_dir_not_symlink(&memory_root, "memory directory") {
        eprintln!("[lazar] WARN: not writing rotation summary: {e}");
    } else if let Err(e) = ensure_dir_not_symlink(&memory, "log-summaries directory") {
        eprintln!("[lazar] WARN: not writing rotation summary: {e}");
    } else {
        let summary_name = archive
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("stream.jsonl.unknown.bak")
            .trim_start_matches("stream.jsonl.")
            .trim_end_matches(".bak")
            .to_string();
        let sum_path = memory.join(format!("{summary_name}.md"));
        let header = format!(
            "# log-summary {summary_name}\n\
             \n\
             archive: {}\n\
             size_bytes: {size}\n\
             rotated_at_unix_ms: {}\n\
             \n\
             _Auto-rotated by the lazar kernel when stream.jsonl exceeded \
             LAZAR_LOG_MAX_BYTES. The kernel may prune old full archives according \
             to LAZAR_LOG_ARCHIVE_KEEP; this summary is the durable navigation \
             record. For richer per-archive summaries (top user prompts, top tool \
             commands), run the `_meta/log-rotation` skill against this archive. \
             For LLM-extracted learnings, run `_meta/distill`._\n",
            archive.display(),
            now_millis()
        );
        if let Err(e) = write_new_no_follow(&sum_path, "rotation summary", &header) {
            eprintln!("[lazar] WARN: failed to write rotation summary: {e}");
        } else {
            summary_path = Some(sum_path);
        }
    }

    Some(RotationInfo {
        archive,
        size_bytes: size,
        summary: summary_path,
    })
}

fn log_archive_timestamp(path: &Path) -> Option<u128> {
    let name = path.file_name()?.to_str()?;
    let stem = name
        .strip_prefix("stream.jsonl.")?
        .strip_suffix(".bak.gz")
        .or_else(|| name.strip_prefix("stream.jsonl.")?.strip_suffix(".bak"))?;
    stem.split('.').next()?.parse().ok()
}

/// Load archive basenames recorded by `_meta/log-harvest` in
/// `memory/.harvested`. Only harvested archives are eligible for prune —
/// unharvested history is never deleted solely because keep was exceeded.
fn harvested_archive_names(_home: &Path) -> std::collections::HashSet<String> {
    let path = crate::user_dir("memory").join(".harvested");
    match fs::read_to_string(path) {
        Ok(s) => s
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        Err(_) => std::collections::HashSet::new(),
    }
}

fn archives_to_prune(
    logs: &Path,
    keep: usize,
    harvested: &std::collections::HashSet<String>,
) -> Vec<PathBuf> {
    if keep == 0 {
        return Vec::new();
    }

    let mut archives: Vec<(u128, String, PathBuf)> = match fs::read_dir(logs) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_file() {
                    return None;
                }
                let ts = log_archive_timestamp(&path)?;
                let name = path.file_name()?.to_str()?.to_string();
                // Never prune unharvested archives — durable journal first.
                if !harvested.contains(&name) {
                    return None;
                }
                Some((ts, name, path))
            })
            .collect(),
        Err(_) => return Vec::new(),
    };

    archives.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    archives
        .into_iter()
        .skip(keep)
        .map(|(_, _, path)| path)
        .collect()
}

fn log_archive_keep_limit() -> usize {
    match env::var("LAZAR_LOG_ARCHIVE_KEEP") {
        Ok(raw) => match raw.parse() {
            Ok(keep) => keep,
            Err(_) => {
                eprintln!(
                    "[lazar] WARN: invalid LAZAR_LOG_ARCHIVE_KEEP={raw:?}; using default 5"
                );
                5
            }
        },
        Err(_) => 5,
    }
}

fn prune_log_archives(logs: &Path) {
    let harvested = harvested_archive_names(&lazar_home());
    for archive in archives_to_prune(logs, log_archive_keep_limit(), &harvested) {
        if let Err(e) = fs::remove_file(&archive) {
            eprintln!(
                "[lazar] WARN: failed to prune old log archive {}: {e}",
                archive.display()
            );
        }
    }
}

/// Append a single JSON event to the canonical stream at logs/stream.jsonl.
/// The kernel records; the agent decides what (if anything) to read back.
/// This is the "infinite memory log" — a single append-only stream of every
/// prompt, response, tool call, and result across all invocations.
///
/// Auto-rotates when the log exceeds LAZAR_LOG_MAX_BYTES.
pub(crate) fn append_stream(mut event: Value) {
    let logs = crate::user_dir("logs");
    let _ = fs::create_dir_all(&logs);
    let path = logs.join("stream.jsonl");
    let lock_path = logs.join(".stream.lock");

    // Capture rotation info from inside the lock so we can fire the
    // log-rotation hook AFTER the lock is released — firing it inside
    // would deadlock because hooks emit hook_start/hook_end events back
    // through this same function.
    let rotation_info = {
        let session_id = crate::core::invocation::session_id()
            .filter(|id| session::validate_session_id(id).is_ok());
        let kind = event.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let mirror = session_id.is_some()
            && matches!(
                kind,
                "user" | "assistant" | "tool_result" | "invoke_start" | "invoke_end"
            );
        let lock = match open_append_no_follow(&lock_path, "stream lock") {
            Ok(file) => Some(file),
            Err(err) if mirror => {
                note_session_write_error(format!("cannot lock stream log: {err}"));
                return;
            }
            Err(err) => {
                eprintln!("[lazar] WARN: cannot lock stream log: {err}");
                None
            }
        };
        if let Some(lock) = lock.as_ref() {
            lock_file_exclusive(lock);
        }

        // Safety floor: rotate before appending if oversized. A log-rotation
        // hook emits hook_start/hook_end events through this function, so do
        // not recursively rotate while that hook is running.
        let info = if IN_LOG_ROTATION_HOOK.with(|active| active.get()) {
            None
        } else {
            maybe_rotate_log(&logs, &path)
        };

        if let Some(obj) = event.as_object_mut() {
            obj.insert("ts_ms".into(), json!(now_millis()));
        }

        match open_append_no_follow(&path, "stream log") {
            Ok(mut f) => {
                if let Err(err) = writeln!(f, "{event}") {
                    if mirror {
                        note_session_write_error(format!("failed to append stream log: {err}"));
                    } else {
                        eprintln!("[lazar] WARN: failed to append stream log: {err}");
                    }
                }
            }
            Err(err) => {
                if mirror {
                    note_session_write_error(format!("failed to append stream log: {err}"));
                } else {
                    eprintln!("[lazar] WARN: failed to append stream log: {err}");
                }
            }
        }

        // If a session id is set for this invocation, mirror the event
        // into the session log so multi-turn continuity works on the
        // next invocation. Skipped for hook_start/hook_end and rotation
        // bookkeeping events — those are kernel-internal and don't need
        // to be in the session-level history.
        if mirror {
            if let Some(session_id) = session_id {
                if let Err(err) = session::append_session(&session_id, event.clone()) {
                    note_session_write_error(err);
                }
            }
        }
        info
        // lock dropped here
    };

    if let Some(info) = rotation_info {
        // Hook fires post-lock so it can write its own hook_start/hook_end
        // events without deadlocking. The new stream.jsonl is now empty,
        // so a hook calling load-context won't see the old data — but the
        // archive path is in the payload.
        IN_LOG_ROTATION_HOOK.with(|active| active.set(true));
        hooks::fire(
            HookEvent::LogRotation,
            json!({
                "archive": info.archive,
                "size_bytes": info.size_bytes,
                "summary": info.summary,
            }),
        );
        IN_LOG_ROTATION_HOOK.with(|active| active.set(false));
        prune_log_archives(&logs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn append_and_write_helpers_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = env::temp_dir().join(format!(
            "lazar-symlink-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target");
        let append_link = dir.join("append-link");
        let write_link = dir.join("write-link");
        fs::write(&target, "existing").unwrap();
        symlink(&target, &append_link).unwrap();
        symlink(&target, &write_link).unwrap();

        let append_err = open_append_no_follow(&append_link, "test append").unwrap_err();
        let write_err = write_new_no_follow(&write_link, "test write", "new").unwrap_err();

        assert!(append_err.to_string().contains("symlinked"));
        assert!(write_err.to_string().contains("symlinked"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unique_archive_path_does_not_reuse_existing_archive() {
        let dir = env::temp_dir().join(format!(
            "lazar-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();

        let first = unique_log_archive_path(&dir);
        fs::write(&first, "existing").unwrap();
        let second = unique_log_archive_path(&dir);

        assert_ne!(first, second);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archives_to_prune_keeps_newest_requested_count_across_bak_variants() {
        let dir = TempTestDir::new("archive-retention-newest");
        let logs = dir.path();
        let old_bak = logs.join("stream.jsonl.100.1.bak");
        let mid_gz = logs.join("stream.jsonl.200.1.bak.gz");
        let new_bak = logs.join("stream.jsonl.300.1.bak");
        fs::write(&old_bak, "old").unwrap();
        fs::write(&mid_gz, "mid").unwrap();
        fs::write(&new_bak, "new").unwrap();
        let harvested: std::collections::HashSet<String> = [
            "stream.jsonl.100.1.bak".into(),
            "stream.jsonl.200.1.bak.gz".into(),
            "stream.jsonl.300.1.bak".into(),
        ]
        .into_iter()
        .collect();

        let to_prune = archives_to_prune(logs, 2, &harvested);

        assert_eq!(to_prune, vec![old_bak]);
    }

    #[test]
    fn archives_to_prune_keep_zero_disables_pruning() {
        let dir = TempTestDir::new("archive-retention-disabled");
        let logs = dir.path();
        fs::write(logs.join("stream.jsonl.100.1.bak"), "old").unwrap();
        fs::write(logs.join("stream.jsonl.200.1.bak.gz"), "new").unwrap();
        let harvested: std::collections::HashSet<String> = [
            "stream.jsonl.100.1.bak".into(),
            "stream.jsonl.200.1.bak.gz".into(),
        ]
        .into_iter()
        .collect();

        let to_prune = archives_to_prune(logs, 0, &harvested);

        assert!(to_prune.is_empty());
    }

    #[test]
    fn archives_to_prune_ignores_active_lock_and_sessions() {
        let dir = TempTestDir::new("archive-retention-ignores-non-archives");
        let logs = dir.path();
        let old_bak = logs.join("stream.jsonl.100.1.bak");
        let new_bak = logs.join("stream.jsonl.200.1.bak");
        fs::write(&old_bak, "old").unwrap();
        fs::write(&new_bak, "new").unwrap();
        fs::write(logs.join("stream.jsonl"), "active").unwrap();
        fs::write(logs.join(".stream.lock"), "lock").unwrap();
        let sessions = logs.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("stream.jsonl.050.1.bak"), "session").unwrap();
        let harvested: std::collections::HashSet<String> = [
            "stream.jsonl.100.1.bak".into(),
            "stream.jsonl.200.1.bak".into(),
        ]
        .into_iter()
        .collect();

        let to_prune = archives_to_prune(logs, 1, &harvested);

        assert_eq!(to_prune, vec![old_bak]);
    }

    #[test]
    fn archives_to_prune_skips_unharvested() {
        let dir = TempTestDir::new("archive-retention-unharvested");
        let logs = dir.path();
        let old_bak = logs.join("stream.jsonl.100.1.bak");
        let new_bak = logs.join("stream.jsonl.200.1.bak");
        fs::write(&old_bak, "old").unwrap();
        fs::write(&new_bak, "new").unwrap();
        // Only the newest is harvested — old unharvested must not be pruned.
        let harvested: std::collections::HashSet<String> =
            ["stream.jsonl.200.1.bak".into()].into_iter().collect();

        let to_prune = archives_to_prune(logs, 1, &harvested);

        assert!(to_prune.is_empty(), "unharvested old archive must be kept");
    }

    struct TempTestDir {
        path: PathBuf,
    }

    impl TempTestDir {
        fn new(label: &str) -> Self {
            let path = env::temp_dir().join(format!(
                "lazar-test-{label}-{}-{}",
                std::process::id(),
                now_millis()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
