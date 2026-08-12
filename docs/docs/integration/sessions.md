# Sessions

By default every `lazar -p` invocation starts cold: no prior messages, just
your prompt plus the system prompt. Sessions are the opt-in continuity
mechanism.

## Resuming

```bash
lazar -p --session <id> "what did we decide about X?"
lazar -p --resume "continue"
```

With `--session <id>` or `--resume`, prior turns from the session log are
prepended to the message array (bounded by
`LAZAR_SESSION_HISTORY_MAX_BYTES`). This is the only mode in which
referential prompts ("yes", "do that") resolve against real prior turns.

## The event stream

Everything — prompts, responses, tool calls, tool results — is appended as
JSONL to `~/lazar/logs/stream.jsonl`, one JSON object per line with fields
like `{kind, ts_ms, content, command, ...}`. This is the "infinite memory"
log: the kernel records it but does **not** auto-load it.

!!! warning "Never `cat` the log"
    `stream.jsonl` can be megabytes. Reading it whole will exceed the model
    context window and the API will reject the request. Use the bounded
    recipes from the `load-context` skill:

    ```bash
    tail -n 200 ~/lazar/logs/stream.jsonl | tail -c 50000
    ```

    `jq` is your friend for field extraction.

## Rotation

The kernel auto-rotates at `LAZAR_LOG_MAX_BYTES` (default ~10MB) and keeps
`LAZAR_LOG_ARCHIVE_KEEP` archives. A `log-rotation` hook fires after each
rotation. The `_meta/log-rotation` skill exists for richer archive summaries
but is optional.

## Referential prompts without a session

If the operator forgot `--session` and sends "yes" / "continue", do **not**
infer the referent from recent tool output — that is the documented #1
source of "executes the wrong thing confidently" errors. Either load context
from the log with the bounded recipe, or ask a one-line clarifying question.

## Session env

- `LAZAR_SESSION_ID` — current session id, set by the kernel.
- `LAZAR_SESSION_HISTORY_MAX_BYTES` — cap on replayed history.
