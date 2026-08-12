# `lazar` CLI reference

The complete flag surface of `bin/lazar`, as reported by `bin/lazar --help`
and confirmed in `src/src/main.rs` (clap `Args` struct). Nothing here is
invented.

```text
The smallest self-evolving agent.

Usage: lazar [OPTIONS]
```

## Flags

### `-p, --prompt <PROMPT>`

Prompt for the agent. This is the normal entry point:

```bash
lazar -p "build a skill that prints the current time, then use it"
```

Conflicts with `--reset-all`.

### `--reset-all`

Wipe `skills/`, `hooks/`, `memory/`, `workspace/`, `logs/`, and saved access
approvals, then re-seed from the kernel (seed skills and hooks are embedded in
the binary via `include_dir!`). Asks for confirmation unless `--yes` is also
passed. The kernel binary itself is untouched. After reset, the agent is at
the exact state of a fresh install.

Safety checks in `src/src/main.rs` refuse to reset an empty, relative,
non-canonical, or symlinked `LAZAR_HOME`.

### `--yes`

Skip confirmation for `--reset-all` (destructive, non-interactive).

### `--verbose`

Verbose mode: prints tool calls, depth, and `stop_reason` to stderr.

### `--model <MODEL>`

Override the model. Falls back to `$LAZAR_MODEL`, then to the built-in
default `claude-sonnet-4-6` (or `MiniMax-M3` when running in MiniMax-fallback
mode — see [Configuration](../getting-started/configuration.md)).

### `--output-format <FORMAT>`

Output format. Default: `text`.

| Value | Behavior |
|---|---|
| `text` | Human-readable: streams assistant text to stdout; tool calls silent |
| `json` | One JSON result object emitted at end (no live output) |
| `stream-json` | JSONL events — one JSON object per line on stdout as they happen (for TUIs, log analyzers) |

See [Output formats](../api/output-formats.md) for event shapes.

### `--input-format <FORMAT>`

Input format. Default: `text`.

| Value | Behavior |
|---|---|
| `text` | Prompt comes from `-p` as a plain string (default) |
| `stream-json` | Reserved: prompt comes as JSONL events on stdin. Currently treated as text |

### `--tick`

Heartbeat: fire all hooks under `hooks/tick.d/` and exit. **No model call.**
Wire this to launchd / cron for scheduled work (memory consolidation, summary
enrichment, etc.). Conflicts with `-p` and `--reset-all`. See
[Hooks](../integration/hooks.md).

### `--session <SESSION>`

Session id for multi-turn conversation continuity. When set, prior turns from
`logs/sessions/<id>.jsonl` are prepended to the message array, and new events
are appended to that log. The same flag both creates the log on first turn and
continues it on subsequent turns. Allowed characters: `a-z A-Z 0-9 - _ .`
(max 64 chars, no path traversal). Conflicts with `--resume`.

### `--resume`

Resume the most recently modified session log automatically — equivalent to
`--session <newest>`. Useful for referential prompts ("ok do that", "yes",
"fine") when you don't remember the session id.

### `--no-sandbox`

Disable `sandbox-exec` for this invocation. The agent's bash runs with full
operator privileges instead of the restricted sandbox profile. Use when
working **outside** `~/lazar` (e.g. running lazar from a project directory and
having it edit project files). Also enabled via `LAZAR_NO_SANDBOX=1`.

!!! warning "Security"
    With the sandbox disabled, the agent can read, write, and execute anywhere
    the operator can — including `~/.ssh`, dotfiles, and system paths. The
    kernel emits a loud banner on every invocation when active.

### `--cwd <PATH>`

Set the working directory used by every model-issued bash command. The path
must already exist and is canonicalized before any hook or tool runs.

### `--add-dir <PATH>`

Add an existing directory to the sandbox's write allowlist for this
invocation. Repeat the flag to approve multiple project roots.

### `-h, --help`

Print help (summary with `-h`, full with `--help`).

## Exit behavior

- Depth guard: invocations nested deeper than `MAX_DEPTH` (5) abort with
  `recursion depth <n> exceeds max 5`.
- Budget guards: the loop aborts with `aborted after reaching
  LAZAR_MAX_TURNS=…` or `LAZAR_MAX_TOOL_CALLS=…` when the caps are hit.
- Grounding: a terminal response whose `[VERIFY]` claims fail twice ends in a
  non-zero `grounding_failed` result.

## Examples

```bash
# Normal invocation
lazar -p "what skills do you have?"

# Multi-turn session
lazar --session refactor -p "step 1: inventory the project"
lazar --session refactor -p "ok, do step 2"     # prior turns are visible

# Scoped project access
lazar --cwd ~/code/proj --add-dir ~/code/proj -p "inspect this project"

# Structured output for a TUI
lazar --output-format stream-json -p "hello" | jq -r .kind

# Heartbeat (no model call)
lazar --tick
```
