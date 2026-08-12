# Hooks

Hooks are user-controlled lifecycle scripts. The kernel fires them at
deterministic moments; you drop executable bash scripts into
`~/lazar/hooks/<event>.d/` and they run in lex order.

## Events

| Event | Fires when | Veto? | Transform? |
|---|---|---|---|
| `session-start` | top of `lazar -p ...`, before first model call (depth 0 only) | — | inject context |
| `user-prompt` | when a top-level prompt is received | — | inject context |
| `pre-tool` | before each bash `execute()` call (every depth) | yes | rewrite command |
| `post-tool` | after each bash `execute()` call returns | — | rewrite output |
| `session-end` | when a top-level invocation ends (success OR error, via Drop) | — | — |
| `log-rotation` | after the kernel auto-rotates `stream.jsonl` | — | — |
| `agent-stop` | when the model emits `stop_reason: end_turn` (depth 0) | — | — |
| `tick` | when invoked as `lazar --tick` (heartbeat path) | — | — |

## Layout

```text
hooks/
├── README.md
├── session-start.d/
├── user-prompt.d/
├── pre-tool.d/
├── post-tool.d/
├── session-end.d/
├── log-rotation.d/
├── agent-stop.d/
└── tick.d/
```

Scripts must be executable (`chmod +x`). Files ending in `.disabled` are
skipped. Prefix with a number for ordering (`01-`, `99-`).

## Protocol

Each hook receives a JSON payload on **stdin**:

```json
{
  "event": "pre-tool",
  "ts_ms": 1714555000000,
  "lazar_home": "/Users/you/lazar",
  "depth": 0
}
```

and may emit one JSON action on **stdout** (default: `continue`):

| Action | Where | Effect |
|---|---|---|
| `{"action":"continue"}` | anywhere | do nothing |
| `{"action":"veto","reason":"..."}` | `pre-tool` | block the call; reason returned to model |
| `{"action":"transform","command":"..."}` | `pre-tool` | rewrite the bash command before exec |
| `{"action":"transform","output":"..."}` | `post-tool` | rewrite captured output before posting to model |
| `{"action":"inject","context":"..."}` | `session-start` / `user-prompt` | append to system prompt as RUNTIME CONTEXT |

Multiple transforms in one `.d/`: **last wins** (lex order). Any `veto` on
`pre-tool` blocks the call.

## Environment

Hooks receive `LAZAR_HOME`, `LAZAR_SKILLS`, `LAZAR_MEMORY`,
`LAZAR_WORKSPACE`, `LAZAR_LOGS`, `LAZAR_HOOK_EVENT`,
`LAZAR_HOOK_PAYLOAD_LEN`, plus `HOME` set to `LAZAR_HOME`. Per-hook timeout:
`LAZAR_HOOK_TIMEOUT_SECS`.

## Examples shipped with the kernel

- `pre-tool.d/00-example-deny-rm-rf.sh.disabled` — veto destructive commands
- `session-end.d/00-example-distill.sh.disabled` — distill the session
- `log-rotation.d/00-example-summarize-archive.sh.disabled` — summarize on rotation
- `tick.d/00-example-daily-distill.sh.disabled` — heartbeat distill

Remove `.disabled` and `chmod +x` to enable. The spawn-discipline pre-tool
hook (the thing that vetoes raw `lazar -p`) is a real installed example of
this mechanism.
