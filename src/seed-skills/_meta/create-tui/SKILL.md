---
name: create-tui
description: Scaffold a terminal interface for lazar — a small client project under workspace/<name>/ that consumes `lazar -p --output-format stream-json` and renders the events. Default stack: TypeScript + readline. Optional: Go + Bubble Tea, Python + Textual, or anything else the user wants. Use when the user asks for a "TUI", "chat UI", "interface" for lazar that's richer than `lazar-chat.sh`.
---

# create-tui

Scaffold a terminal interface (TUI) that talks to lazar. The output is a small project under `$LAZAR_HOME/workspace/<name>/`. The TUI is a *client* — it spawns `lazar -p --output-format stream-json` as a subprocess and renders the JSONL events. Lazar itself is the agent; the TUI is just the mouth and ears.

This skill is for richer experiences than `scripts/lazar-chat.sh` (a 30-line shell loop with glow piping). If the user just wants to chat with markdown rendering, point them there first and skip this skill.

## When to use

- The user asks for a "TUI", "chat interface", "interactive UI" for lazar.
- They want streaming, markdown, scrollback, keyboard shortcuts, or split panes.
- They mention bubble tea, textual, ink, blessed, ratatui, or any TUI framework.

## Always ask first

Before generating *anything*, ask the user three things:

1. **Stack** — see Stack Choice table below. Default is TypeScript + readline.
2. **Customization** — input style, tool display, loader, banner.
3. **Project name** — for the directory and any banner art.

Don't pre-commit to a stack. Different users have very different terminal preferences.

## Stack choice

| Stack | When to pick | Notes |
|-------|--------------|-------|
| **TypeScript + readline** (default) | Cross-platform, no compile step, easiest to extend | Single `src/cli.ts` file, adapts to terminal theme. See `references/typescript-readline.md`. |
| **Go + Bubble Tea + Glamour** | User wants a polished split-pane TUI with markdown, viewport, textarea, image rendering | Heavier setup. Set `GOMODCACHE=workspace/<name>/.gocache/mod` (see _meta/log-rotation cousin). The user may already have one at `workspace/lazartui/` — read it first. |
| **Python + Textual** | Python users, widget-based design | Use a venv at `workspace/<name>/.venv/`. Textual handles streaming widgets cleanly. |
| **Other** | User has a specific stack | Read `references/stream-json-protocol.md` and adapt. |

## Lazar protocol — read this first

Before writing any TUI code, read `references/stream-json-protocol.md`. It documents the exact JSONL events lazar emits and how to consume them. Skipping it leads to weeks of "why doesn't it stream" debugging.

The short version: `lazar -p "..." --output-format stream-json` emits one JSON object per line on stdout. Event types include `invoke_start`, `text_delta`, `text_done`, `tool_use`, `tool_result`, `invoke_end`, `error`. You parse, dispatch, and render.

## Customization checklist

Present these as multi-select (defaults marked **ON**):

### Input style

| Style | Default | Description |
|-------|---------|-------------|
| `block` | ON | Full-width background box with `▶` prompt, adapts to terminal theme |
| `bordered` |  | Horizontal `─` lines above and below input |
| `plain` |  | Simple `> ` prompt, no escape sequences |

### Tool display

| Style | Default | Description |
|-------|---------|-------------|
| `grouped` | ON | Bold action labels with tree-branch output |
| `emoji` |  | Per-call `⚡`/`✓` markers with command preview and timing |
| `minimal` |  | One-liner summaries |
| `hidden` |  | Show only assistant text; tools silent |

### Loader

| Style | Default | Description |
|-------|---------|-------------|
| `spinner` | ON | Braille dot spinner (⠋⠙⠹) while waiting for first event |
| `dots` |  | Trailing `Working···` |
| `none` |  | No loader |

### Banner

ASCII logo? Y/N. If yes, ask for the project name (1-2 short words, fits in 60 cols).

## Generation workflow

```
- [ ] Confirm stack + customization with the user
- [ ] Read references/stream-json-protocol.md
- [ ] Read references/<stack>.md (default: typescript-readline.md)
- [ ] mkdir -p $LAZAR_HOME/workspace/<name>
- [ ] Generate manifest (package.json / go.mod / pyproject.toml)
- [ ] Generate entry point (src/cli.ts / main.go / __main__.py)
- [ ] Set toolchain cache env (NPM_CONFIG_CACHE / GOMODCACHE / pip cache) inside workspace
- [ ] Compile / build (don't try to run — it's interactive)
- [ ] Add a shell alias via _meta/shell-handoff (e.g. alias <name>='$LAZAR_HOME/workspace/<name>/...')
- [ ] Write a PROJECT.md (see _meta/project-context) so future-you knows what this is
- [ ] Tell the user the alias name and how to source it
```

## Hard rules

- **Output goes under `workspace/<name>/`. Never** under `skills/`, `bin/`, `src/`, or anywhere outside `workspace/`.
- **Never call the Anthropic API directly from the TUI.** Always go through `lazar -p`. Lazar IS the agent; the TUI just renders.
- **Never write to PATH.** Use the shell-handoff aliases.sh pattern.
- **Set toolchain caches inside workspace.** Go: `GOMODCACHE=workspace/<name>/.gocache/mod`. npm: `NPM_CONFIG_CACHE=workspace/<name>/.npm`. Python: venv inside `workspace/<name>/.venv/`. Without this, the sandbox blocks `~/go/`, `~/.npm/`, `~/.cache/pip/` writes.
- **Bound stdout buffers.** Go's `bufio.Scanner` defaults to 64KB per token; raise to 10MB. Other languages are usually fine.
- **Don't try to render text-deltas with glamour incrementally.** Buffer per text block (between `content_block_start` and `text_done`) and render the whole block at once. Live token-by-token markdown looks great until a list opens; then it's a flicker fest.

## Compose with other skills

- `_meta/shell-handoff` — how to expose the TUI as a shell command via `aliases.sh`.
- `_meta/project-context` — write a PROJECT.md inside `workspace/<name>/` so future agent invocations stay focused on this project.
- `_meta/log-rotation` — the TUI runs fine with no rotation, but mention it to the user since stream.jsonl will grow.
- `_meta/create-skill` — if the TUI needs a new lazar-side capability, propose it as a skill, not a TUI feature.

## When NOT to use this skill

- The user just wants to *chat* — point them at `$LAZAR_HOME/scripts/lazar-chat.sh` and stop.
- The user wants the agent to do *one thing* differently — that's a skill, not a UI change.
- The user wants the agent to have a new tool — that's a kernel patch (see `_meta/propose-kernel-patch`), not a TUI.

## Anti-patterns

- Generating a TUI without asking the stack. Always ask.
- Re-implementing what `lazar-chat.sh` already does. If the request fits a 30-line shell loop, point at the loop.
- Building a parallel implementation when one already exists at `workspace/lazartui/` (or wherever). Read first; modify in place if you can.
- Mixing event types: don't try to use `--output-format text` and parse it heuristically. Always use `stream-json` for programmatic consumers.
