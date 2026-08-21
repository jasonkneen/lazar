//! Shared subprocess scaffolding for agent bash and lifecycle hooks.
//! Both run through the same sandbox profile — keep them identical by
//! construction: any confinement change happens HERE, once.

use std::env;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::access::AccessPolicy;
use crate::SANDBOX_PROFILE;

fn sandbox_profile(policy: &AccessPolicy) -> String {
    let mut profile = SANDBOX_PROFILE.to_string();
    if policy.allowed_write_dirs().is_empty() {
        return profile;
    }
    profile.push_str("\n;; operator-approved runtime project roots\n(allow file-write*\n");
    for (i, _path) in policy.allowed_write_dirs().iter().enumerate() {
        profile.push_str(&format!("  (subpath (param \"ALLOWED_WRITE_DIR_{i}\"))\n"));
    }
    profile.push_str(")\n");
    profile
}

/// Credentials and config passed through to the child process when the
/// operator has explicitly opted out of the sandbox (`LAZAR_NO_SANDBOX=1`).
/// SSH agent, GPG, GitHub/GitLab tokens, common LLM API keys, editor/pager
/// preferences, locale, color settings.
pub(crate) const PASSTHROUGH_VARS: &[&str] = &[
    "SSH_AUTH_SOCK", "SSH_AGENT_PID",
    "GPG_TTY", "GNUPGHOME",
    "GH_TOKEN", "GITHUB_TOKEN", "GITLAB_TOKEN",
    "OPENAI_API_KEY", "GROQ_API_KEY", "GEMINI_API_KEY",
    "GOOGLE_API_KEY", "MISTRAL_API_KEY", "TOGETHER_API_KEY",
    "PERPLEXITY_API_KEY", "DEEPSEEK_API_KEY", "XAI_API_KEY",
    "AWS_PROFILE", "AWS_REGION", "AWS_DEFAULT_REGION",
    "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN",
    "EDITOR", "VISUAL", "PAGER",
    "LC_ALL", "LC_CTYPE",
    "COLORTERM", "CLICOLOR", "FORCE_COLOR", "NO_COLOR",
    "USER", "LOGNAME",
    "TMPDIR",
];

/// What /bin/bash should execute.
pub(crate) enum BashPayload<'a> {
    /// `bash -c <cmd>` — the agent tool path.
    Inline(&'a str),
    /// `bash <script>` — the hook path.
    Script(&'a Path),
}

/// Build the bash Command, wrapped in sandbox-exec unless `unsandboxed`.
/// Mirrors main.rs run_bash / hooks.rs run_hook construction exactly.
/// Does NOT set cwd/stdio/env — those stay at the call site (caller-specific).
pub(crate) fn bash_command(payload: BashPayload, lazar: &str, policy: &AccessPolicy) -> Command {
    if policy.unsandboxed() {
        let mut c = Command::new("/bin/bash");
        match payload {
            BashPayload::Inline(cmd) => {
                c.arg("-c").arg(cmd);
            }
            BashPayload::Script(script) => {
                c.arg(script);
            }
        }
        c
    } else {
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-D")
            .arg(format!("SKILLS_PATH={lazar}/user/skills"))
            .arg("-D")
            .arg(format!("MEMORY_PATH={lazar}/user/memory"))
            .arg("-D")
            .arg(format!("WORKSPACE_PATH={lazar}/user/workspace"))
            .arg("-D")
            .arg(format!("KERNEL_SRC={lazar}/src"))
            .arg("-D")
            .arg(format!("TUI_SRC={lazar}/tui"))
            .arg("-D")
            .arg(format!(
                "USER_HOME={}",
                env::var("HOME").unwrap_or_else(|_| "/".to_string())
            ));
        for (i, dir) in policy.allowed_write_dirs().iter().enumerate() {
            c.arg("-D").arg(format!("ALLOWED_WRITE_DIR_{i}={}", dir.display()));
        }
        c.arg("-p")
            .arg(sandbox_profile(policy))
            .arg("/bin/bash");
        match payload {
            BashPayload::Inline(cmd) => {
                c.arg("-c").arg(cmd);
            }
            BashPayload::Script(script) => {
                c.arg(script);
            }
        }
        c
    }
}

/// Resolve the HOME the child bash process should see: the operator's real
/// HOME when unsandboxed (so ~/.ssh, git config, gh/claude/codex auth work),
/// pinned to LAZAR_HOME when sandboxed (existing behavior).
pub(crate) fn bash_home(lazar: &str, unsandboxed: bool) -> String {
    if unsandboxed {
        env::var("HOME").unwrap_or_else(|_| lazar.to_string())
    } else {
        lazar.to_string()
    }
}

/// env_clear + the base env both callers share (PATH, HOME, LAZAR_HOME,
/// LAZAR_SKILLS/MEMORY/WORKSPACE/LOGS, TERM, LANG), inherited scoped access,
/// plus LAZAR_NO_SANDBOX and PASSTHROUGH_VARS when unsandboxed.
pub(crate) fn apply_base_env(
    command: &mut Command,
    lazar: &str,
    bash_home: &str,
    policy: &AccessPolicy,
) {
    let unsandboxed = policy.unsandboxed();
    command
        .env_clear()
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .env("HOME", bash_home)
        .env("LAZAR_HOME", lazar)
        .env("LAZAR_SKILLS", format!("{lazar}/user/skills"))
        .env("LAZAR_MEMORY", format!("{lazar}/user/memory"))
        .env("LAZAR_WORKSPACE", format!("{lazar}/user/workspace"))
        .env("LAZAR_LOGS", format!("{lazar}/user/logs"))
        .env(crate::access::INHERITED_CWD_ENV, policy.tool_cwd())
        .env(
            crate::access::INHERITED_DIRS_ENV,
            serde_json::to_string(policy.allowed_write_dirs())
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .env(
            "TERM",
            env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
        )
        .env(
            "LANG",
            env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()),
        );

    if unsandboxed {
        command.env("LAZAR_NO_SANDBOX", "1");
        for var in PASSTHROUGH_VARS {
            if let Ok(val) = env::var(var) {
                command.env(var, val);
            }
        }
    }
}

/// setpgid(0,0) pre_exec so the whole process tree the child spawns can be
/// killed as a group later (see `kill_process_group`).
#[cfg(unix)]
pub(crate) fn isolate_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub(crate) fn isolate_process_group(_command: &mut Command) {}

/// Keeps the command group's lifetime tied to the kernel, including SIGKILL.
/// The write end is close-on-exec, so nested commands cannot retain ownership.
pub(crate) struct ProcessOwner {
    #[cfg(unix)]
    _lifetime: std::os::fd::OwnedFd,
}

#[cfg(unix)]
pub(crate) fn spawn_owned(mut command: Command) -> std::io::Result<(std::process::Child, ProcessOwner)> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let reader = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    for descriptor in [&reader, &writer] {
        if unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let descriptor_limit = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    if descriptor_limit <= 0 || descriptor_limit > i32::MAX as libc::c_long {
        return Err(std::io::Error::other("cannot determine guardian descriptor limit"));
    }
    let read_fd = reader.as_raw_fd();
    let write_fd = writer.as_raw_fd();
    isolate_process_group(&mut command);
    unsafe {
        command.pre_exec(move || {
            let group = libc::getpid();
            libc::close(write_fd);
            let guardian = libc::fork();
            if guardian == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if guardian == 0 {
                // Only async-signal-safe operations are allowed after this fork.
                // In particular, close the Command exec-error pipe and captured
                // stdio so the guardian cannot hold spawn or output readers open.
                if libc::setpgid(0, 0) == -1 {
                    libc::kill(-group, libc::SIGKILL);
                    libc::_exit(1);
                }
                for fd in 0..descriptor_limit as i32 {
                    if fd != read_fd { libc::close(fd); }
                }
                let mut byte = 0u8;
                loop {
                    let read = libc::read(read_fd, (&mut byte as *mut u8).cast(), 1);
                    if read == 0 { break; }
                    if read == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                        break;
                    }
                }
                libc::kill(-group, libc::SIGKILL);
                libc::_exit(0);
            }
            libc::close(read_fd);
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok((child, ProcessOwner { _lifetime: writer }))
}

#[cfg(not(unix))]
pub(crate) fn spawn_owned(mut command: Command) -> std::io::Result<(std::process::Child, ProcessOwner)> {
    command.spawn().map(|child| (child, ProcessOwner {}))
}

/// try_wait poll loop with timeout; kills the process group on expiry.
/// Returns `Ok((exit_status, timed_out))` on a normal wait (whether it
/// completed or was killed for timing out), or `Err` if `try_wait` itself
/// errored (child already killed in that case). Callers differ in how they
/// handle the Err path (run_bash short-circuits with an error string,
/// run_hook falls through treating it like a non-timeout, no-status exit) —
/// that decision stays at the call site.
pub(crate) fn wait_with_timeout(
    child: &mut std::process::Child,
    child_pid: u32,
    timeout: Duration,
    poll: Duration,
) -> Result<(Option<std::process::ExitStatus>, bool), std::io::Error> {
    let started = Instant::now();
    let mut timed_out = false;
    loop {
        if crate::core::invocation::is_cancelled() {
            crate::kill_process_group(child_pid);
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok((Some(status), timed_out)),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    timed_out = true;
                    crate::kill_process_group(child_pid);
                    let _ = child.kill();
                    return Ok((child.wait().ok(), timed_out));
                }
                thread::sleep(poll);
            }
            Err(e) => {
                crate::kill_process_group(child_pid);
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    #[test]
    fn dropping_process_ownership_terminates_the_child() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let (mut child, owner) = spawn_owned(command).unwrap();
        drop(owner);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(!status.success());
                break;
            }
            if Instant::now() >= deadline {
                crate::kill_process_group(child.id());
                child.wait().unwrap();
                panic!("child outlived its owner");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn owned_spawn_preserves_exec_errors() {
        let error = spawn_owned(Command::new("/does-not-exist/lazar-test-command")).err().expect("spawn error");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn cancellation_interrupts_a_running_process_group() {
        let id = format!("spawn-cancel-{}", std::process::id());
        let _scope = crate::core::invocation::scope_session(id.clone());
        let mut command = Command::new("/bin/bash");
        command.args(["-c", "sleep 60 & wait"]);
        isolate_process_group(&mut command);
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        let cancel_id = id.clone();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            crate::core::invocation::cancel_session(&cancel_id);
        });
        let started = Instant::now();
        let result = wait_with_timeout(&mut child, pid, Duration::from_secs(60), Duration::from_millis(10));
        cancel.join().unwrap();
        crate::core::invocation::clear_cancel(&id);
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn sandbox_profile_grants_the_canonical_allowed_directory() {
        let requested = std::env::temp_dir().join(format!(
            "lazar-allowed-profile-{}", std::process::id()
        ));
        std::fs::create_dir_all(&requested).expect("create allowed dir");
        let _canonical = requested.canonicalize().expect("canonical allowed dir");
        let policy = AccessPolicy::new(
            false,
            None,
            &[PathBuf::from(&requested)],
            Path::new("/lazar"),
        ).expect("valid policy");
        let profile = sandbox_profile(&policy);

        assert!(profile.contains("(subpath (param \"ALLOWED_WRITE_DIR_0\"))"));
        std::fs::remove_dir_all(requested).expect("remove allowed dir");
    }

    #[test]
    fn sandbox_command_writes_inside_allowed_root_but_not_its_sibling() {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let root = home.join(format!(".lazar-access-test-{}", std::process::id()));
        let allowed = root.join("allowed");
        let denied = root.join("denied");
        std::fs::create_dir_all(&allowed).expect("create allowed dir");
        std::fs::create_dir_all(&denied).expect("create denied dir");
        let policy = AccessPolicy::new(
            false,
            Some(&allowed),
            std::slice::from_ref(&allowed),
            Path::new("/Users/Shared/lazar-test-home"),
        ).expect("valid policy");

        let allowed_file = allowed.join("ok.txt");
        let mut allowed_cmd = bash_command(
            BashPayload::Inline("printf ok > \"$TEST_TARGET\""),
            "/Users/Shared/lazar-test-home",
            &policy,
        );
        let allowed_status = allowed_cmd
            .env("TEST_TARGET", &allowed_file)
            .status()
            .expect("run allowed command");
        assert!(allowed_status.success());
        assert_eq!(std::fs::read_to_string(&allowed_file).expect("allowed output"), "ok");

        let denied_file = denied.join("blocked.txt");
        let mut denied_cmd = bash_command(
            BashPayload::Inline("printf blocked > \"$TEST_TARGET\""),
            "/Users/Shared/lazar-test-home",
            &policy,
        );
        let denied_status = denied_cmd
            .env("TEST_TARGET", &denied_file)
            .status()
            .expect("run denied command");
        assert!(!denied_status.success());
        assert!(!denied_file.exists());

        std::fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn wait_with_timeout_kills_on_expiry() {
        let mut command = Command::new("/bin/bash");
        command
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        isolate_process_group(&mut command);

        let mut child = command.spawn().expect("failed to spawn sleep 5");
        let child_pid = child.id();

        let started = Instant::now();
        let result = wait_with_timeout(
            &mut child,
            child_pid,
            Duration::from_millis(50),
            Duration::from_millis(10),
        );
        let elapsed = started.elapsed();

        let (_, timed_out) = result.expect("wait_with_timeout should not error");
        assert!(timed_out, "expected the 5s sleep to time out at 50ms");
        assert!(
            elapsed < Duration::from_secs(2),
            "wait_with_timeout took too long to return: {elapsed:?}"
        );
    }
}
