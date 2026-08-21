---
name: lazartui
description: Use, build, and troubleshoot Lazar's shipped native Rust terminal client. The source is tui/, the installed binary is bin/lazartui, and the client keeps one lazar acp child alive.
---

# lazartui

`lazartui` is the native Rust client shipped with Lazar. It is a thin ACP
front end: the kernel owns the agent loop, provider requests, tools, sessions,
and access policy.

## Paths

- Source: `$LAZAR_HOME/tui/`
- Build/install script: `$LAZAR_HOME/scripts/tui-build.sh`
- Installed binary: `$LAZAR_HOME/bin/lazartui`
- Kernel transport: one long-lived `$LAZAR_HOME/bin/lazar acp` child

The retired Go client and its launch/watchdog scripts are archived locally
under `$LAZAR_HOME/.archive/legacy-go-tui-2026-09-04/`. Never revive or edit
that archive for current TUI work.

## Usage

```bash
lazartui
lazartui --provider <provider-id>
lazartui --provider <provider-id> --model <model-id>
lazartui --cwd /path/to/project --add-dir /another/write/root
lazartui --session <session-id>
```

Provider ids and model names come from `$LAZAR_HOME/providers.toml`. Inspect
them with `lazar providers`; do not maintain a second catalogue in the TUI.

## Build and test

```bash
cd "$LAZAR_HOME/tui"
cargo test --locked --offline
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
bash "$LAZAR_HOME/scripts/tui-build.sh"
```

When diagnosing transport behavior, test the ACP child and the TUI state
transition separately. A successful build does not prove interactive framing,
restart, cancellation, or session recovery.
