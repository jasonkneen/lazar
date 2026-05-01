# lazar hooks

Drop-in lifecycle scripts. The kernel fires hooks at deterministic moments.
You drop bash scripts into `<event>.d/`, the kernel runs them in lex order,
and they can either observe or influence behavior.

This directory was seeded by the kernel on first install. You may edit any
file here, add new ones, or remove the examples. Re-seeding only happens on
`lazar --reset-all`.

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

Each event has a `.d/` directory. Drop bash scripts in. They must be
**executable** (`chmod +x`) to fire. Files ending in `.disabled` are skipped
(the kernel ships examples this way so they don't run by default).

```
hooks/
├── README.md                           # this file
├── session-start.d/
├── user-prompt.d/
├── pre-tool.d/
│   └── 00-example-deny-rm-rf.sh.disabled
├── post-tool.d/
├── session-end.d/
│   └── 00-example-distill.sh.disabled
├── log-rotation.d/
│   └── 00-example-compress-archives.sh.disabled
├── agent-stop.d/
└── tick.d/
    └── 00-example-daily-distill.sh.disabled
```

Naming: prefix with a number for ordering (`01-`, `99-`). All matching
scripts in a `.d/` fire on every event in lex order.

## Protocol

Each hook receives a JSON payload on **stdin**:

```json
{
  "event": "pre-tool",
  "ts_ms": 1714555000000,
  "lazar_home": "/Users/you/lazar",
  "depth": 0,
  ...event-specific fields...
}
```

The hook may emit a JSON action on **stdout** to influence behavior. The
default (no stdout, or stdout that doesn't parse as JSON) is `continue`.

| Action | Where | Effect |
|---|---|---|
| `{"action":"continue"}` | anywhere | default; do nothing |
| `{"action":"veto","reason":"..."}` | `pre-tool` only | block this tool call; reason returned to model |
| `{"action":"transform","command":"..."}` | `pre-tool` only | rewrite the bash command before exec |
| `{"action":"transform","output":"..."}` | `post-tool` only | rewrite captured output before posting to model |
| `{"action":"inject","context":"..."}` | `session-start` / `user-prompt` | append to system prompt as RUNTIME CONTEXT |

When multiple scripts in the same `.d/` return transforms, **last wins** (lex order).
When any script returns `veto` on `pre-tool`, the call is blocked — earlier
transforms still register but don't matter.

## Environment

Every hook gets these env vars:

- `LAZAR_HOME`, `LAZAR_SKILLS`, `LAZAR_MEMORY`, `LAZAR_WORKSPACE`, `LAZAR_LOGS`
- `LAZAR_HOOK_EVENT` — the event name (`pre-tool`, etc)
- `LAZAR_HOOK_PAYLOAD_LEN` — bytes of payload coming on stdin (in case you
  want to refuse oversized payloads without reading)
- `HOME` — set to `LAZAR_HOME` so `~` works as expected
- `PATH` — homebrew + system paths

## Sandbox

Hooks run through the same `sandbox-exec` profile as agent bash. They can
**read** anywhere but can only **write** to:

- `$LAZAR_SKILLS`
- `$LAZAR_MEMORY`
- `$LAZAR_WORKSPACE`
- `$LAZAR_LOGS`
- `/tmp`

This means a buggy or malicious hook cannot modify `bin/`, `src/`, or
anything in your home directory outside `~/lazar`. Same boundary as the
agent's own code execution.

## Timeouts

Each hook gets up to **5 seconds** by default. Override with
`LAZAR_HOOK_TIMEOUT_SECS=N`. A timed-out hook is killed and treated as
`continue`.

## Errors

If a hook crashes, returns non-zero, emits invalid JSON, or hangs:
- A `WARN` is printed to stderr.
- The kernel treats the result as `continue` (no behavior change).
- The session does **not** abort.

Hooks are advisory infrastructure — a bug in your hook should not break
your agent.

## Observability

Every hook fire emits two events to `logs/stream.jsonl`:

```json
{"kind":"hook_start","event":"pre-tool","script":"01-deny.sh","ts_ms":...}
{"kind":"hook_end","event":"pre-tool","script":"01-deny.sh","action":"veto",
 "exit_code":0,"duration_ms":12,"timed_out":false,"ts_ms":...}
```

Stream-json consumers (TUIs, dashboards) can render these as a hook activity
feed.

## Heartbeat — wire `--tick` to your scheduler

The kernel does not run a daemon. To get scheduled background work, wire
`lazar --tick` to your OS scheduler:

**macOS** — write `~/Library/LaunchAgents/com.lazar.tick.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.lazar.tick</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/lazar</string>
    <string>--tick</string>
  </array>
  <key>StartInterval</key><integer>300</integer>
  <key>StandardOutPath</key><string>/Users/YOU/lazar/logs/tick.log</string>
  <key>StandardErrorPath</key><string>/Users/YOU/lazar/logs/tick.log</string>
</dict>
</plist>
```

Then: `launchctl load ~/Library/LaunchAgents/com.lazar.tick.plist`

**Linux** — add to user crontab (`crontab -e`):

```
*/5 * * * * /usr/local/bin/lazar --tick >> ~/lazar/logs/tick.log 2>&1
```

Each `tick.d/` hook owns its own scheduling via touch files in
`$LAZAR_MEMORY/.heartbeat-*`. The OS triggers every 5 minutes; hooks decide
"is this my turn?"

## Examples

The seed includes one disabled example per use case. To activate one,
remove `.disabled`:

```bash
mv ~/lazar/hooks/pre-tool.d/00-example-deny-rm-rf.sh.disabled \
   ~/lazar/hooks/pre-tool.d/00-deny-rm-rf.sh
```

## Hard rules

- **Hooks are infrastructure, not skills.** Skills are agent-controlled
  capabilities. Hooks are user-controlled lifecycle scripts. The agent
  may read hooks (to study itself) but should not modify hooks the user
  installed without being asked.
- **Don't make hooks slow.** A 5s timeout exists, but `pre-tool` and
  `post-tool` fire on every bash call — even 100ms adds up.
- **Don't make hooks recursive.** A hook can shell out to lazar, but
  remember that nested `lazar -p` calls don't fire `session-start` /
  `session-end` (only top-level does).
- **Test before activating.** Manual fire: `echo '{}' | bash my-hook.sh`.
