# lazar — CodeSurf Workspace Memory

*Generated: 2026-09-24*

---

## Overview

`/Users/jkneen/lazar` is a Rust monorepo containing a CLI daemon binary (`bin/lazar`) and a terminal UI crate (`lazartui`). The binary runs as a **live launchd service** on the host machine; all development must account for that at every step.

---

## Durable Facts

- **Live daemon** — `bin/lazar` is installed and managed by launchd. The production binary is always running.
- **Never run `kernel-build.sh`** and never swap `bin/lazar` while the daemon is live.
- **Build target** — use `cargo` inside the workspace; the shared target directory is `workspace/.cargo-target`.
- **Test isolation** — all tests must override `LAZAR_HOME` to avoid touching the live daemon's state or config.
- **Install immediately** — as soon as tests pass, install the new binary without batching. Always tell the user to restart `lazartui` after install.
- **Crates**
  - `lazartui` — TUI front-end
  - core/engine crate (name to be confirmed) — daemon logic

---

## Active Workflows

- **Build:** `cargo build` in workspace root
- **Test:** `cargo test` with `LAZAR_HOME` overridden to a temp path
- **Deploy:** install binary → restart launchd service → prompt user to restart `lazartui`

---

## Open Threads

- **opencode provider errors** — multiple recent CodeSurf sessions failed immediately with `Unrecognized flag: --dir in command opencode run`; CodeSurf's opencode provider invocation is broken in this workspace. Workaround: use Claude Code or Codex providers directly until the flag issue is resolved upstream.
- **Core crate name** — the exact name of the engine/daemon crate has not been confirmed in session evidence; verify from `Cargo.toml` before referencing it in scripts or documentation.

---

## Notes for Future Sessions

- Memory file `~/.claude/projects/-Users-jkneen-lazar/memory/MEMORY.md` contains linked detail files (`lazar-live-launchd.md`, `shell-interactive-aliases.md`, `install-immediately.md`) — read these before making any install or file-copy decisions.
- Shell has interactive `cp`/`mv` aliases; use `command cp -f` / `command mv -f` in any scripted overwrite to avoid hanging on a prompt.
