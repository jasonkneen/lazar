# Output formats

`--output-format` controls what `lazar` writes to stdout during a run. Three
modes, from `bin/lazar --help` and `src/src/main.rs`.

## `text` (default)

Human-readable: assistant text streams to stdout as it arrives; tool calls are
silent. This is the normal interactive mode.

```bash
lazar -p "hello"
```

## `json`

One JSON result object emitted at the end of the run — no live output. For
scripts that want the final answer as data.

```bash
lazar --output-format json -p "2+2?" > result.json
```

## `stream-json`

JSONL events — one JSON object per line on stdout **as they happen**. This is
the machine interface: TUIs, log analyzers, and dashboards consume it.

```bash
lazar --output-format stream-json -p "list your skills" | jq -r .type
```

### Event types

The same events are also appended to `logs/stream.jsonl` (with `kind` instead
of `type`). Observed kinds in `src/`:

| Event | Fires when |
|---|---|
| `invoke_start` | Start of an invocation; carries `depth` and `model` |
| `user` | The prompt, as received |
| `assistant` | Each assistant message (content blocks) |
| `tool_result` | Result of a bash tool call |
| `auto_nudge` | Kernel stall-recovery nudge fired (bounded by `LAZAR_STALL_NUDGES`) |
| `invoke_end` | End of invocation; carries `stop_reason` |
| `hook_start` / `hook_end` | A lifecycle hook fired; `hook_end` carries `action`, `exit_code`, `duration_ms`, `timed_out` |
| `tick_start` / `tick_end` | A `--tick` heartbeat run |
| `session_continuation` | Prior session turns were prepended (`--session` / `--resume`) |
| `sandbox_disabled` | The kernel printed its no-sandbox warning banner |
| `access_policy` | The resolved access policy for the invocation |

### Sample event

```json
{"type":"invoke_start","depth":0,"model":"claude-sonnet-4-6","ts_ms":1714555000000}
```

```json
{"type":"hook_end","event":"pre-tool","script":"01-deny.sh","action":"veto",
 "exit_code":0,"duration_ms":12,"timed_out":false,"ts_ms":1714555000123}
```

Consumers should treat unknown `type` values as forward-compatible noise and
keep reading — the kernel may add event kinds without notice.

## `--input-format`

`text` (default) takes the prompt from `-p`. `stream-json` is **reserved**:
stdin JSONL input is currently treated as text.
