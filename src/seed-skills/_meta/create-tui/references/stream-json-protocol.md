# stream-json protocol

Lazar emits one JSON object per line on stdout when invoked as:

    lazar -p "..." --output-format stream-json

Every event has a `type` field and a `ts_ms` field (Unix milliseconds, added by the kernel). All events are also written to `$LAZAR_HOME/logs/stream.jsonl` for offline replay.

## Event types

### `invoke_start`

```json
{"type":"invoke_start","depth":0,"model":"claude-sonnet-4-6","prompt":"...","ts_ms":1745928000000}
```

Emitted exactly once at the start of the agent run.

- `depth` — recursion depth (0 for top-level, increments when lazar calls itself via `lazar -p` from a tool).
- `model` — the model name in use.
- `prompt` — the user prompt that started this invocation.

### `text_delta`

```json
{"type":"text_delta","index":0,"text":"Hel","ts_ms":1745928000123}
```

Streamed as the model produces assistant text.

- `index` — the content block index. Multiple text blocks within one assistant turn are rare but possible (one per `content_block_start`). Concat deltas for the same `index` to assemble the full text.
- `text` — the partial text delta.

### `text_done`

```json
{"type":"text_done","index":0,"ts_ms":1745928000456}
```

Marks end of a text block. After this, the block at `index` is complete and will not receive more `text_delta` events. Useful for: "the model is now thinking about the next thing" or "render this block with markdown."

### `tool_use`

```json
{"type":"tool_use","index":1,"id":"toolu_abc","name":"execute","input":{"command":"cat $LAZAR_HOME/skills/INDEX.md"},"ts_ms":1745928000789}
```

Emitted when the model has decided on a tool call (after the input JSON has fully streamed and parsed).

- `index` — the content block index.
- `id` — the tool use ID. Pairs with the next `tool_result`.
- `name` — currently always `execute` (lazar has only one tool).
- `input.command` — the bash command about to run.

### `tool_result`

```json
{"type":"tool_result","tool_use_id":"toolu_abc","command":"cat ...","content":"...stdout...\n[stderr]\n...stderr...\n[exit 0]","ts_ms":1745928001234}
```

Emitted after lazar runs the bash command.

- `tool_use_id` — pairs with the `tool_use` event's `id`.
- `command` — copy of the command that ran (for convenience).
- `content` — full output: stdout, optional `[stderr]` block, and `[exit N]` marker.

### `invoke_end`

```json
{"type":"invoke_end","stop_reason":"end_turn","duration_ms":4823,"ts_ms":1745928004823}
```

Emitted exactly once when the agent loop terminates.

- `stop_reason` — usually `end_turn`. Can be other API stop reasons; if `null` or missing, treat as anomalous.
- `duration_ms` — total wall time from `invoke_start` to here.

### `error`

```json
{"type":"error","message":"API 413: prompt is too long: ... > 1000000 maximum","ts_ms":1745928001000}
```

Emitted on API errors before or during the stream (HTTP-level errors and SSE error events both surface as this).

After an `error` event the lazar process exits non-zero. Your TUI should:

1. Render the message in red (or whatever surfaces failures).
2. Wait for `cmd.Wait()` to return; non-zero exit is expected.
3. Allow the user to retry — don't crash the TUI.

## Consumer pattern

```
spawn:  lazar -p "$prompt" --output-format stream-json
loop on stdout, line-by-line:
  parse JSON
  switch event.type:
    invoke_start: clear or show "thinking..."
    text_delta:   append event.text to current block buffer; redraw
    text_done:    finalize block (e.g. apply markdown rendering)
    tool_use:     show "running: ${event.input.command}" with timer
    tool_result:  show output, collapse if long
    invoke_end:   stop loader, await next user input
    error:        render in red, await cmd.Wait()
```

## Language-specific gotchas

### Go: scanner buffer size

`bufio.Scanner` defaults to 64KB per token. A `tool_result` whose `content` is one long line (e.g. the agent ran `cat` on a big file) will exceed this and the scanner errors with `token too long`. Always set a generous buffer:

```go
scanner := bufio.NewScanner(stdout)
buf := make([]byte, 1024*1024)
scanner.Buffer(buf, 10*1024*1024)  // 10MB max line
```

### Node: line splitting

`readline.createInterface` works fine. If you're using raw streams, split on `\n` and remember to handle trailing partial lines across reads:

```typescript
let leftover = '';
proc.stdout.on('data', (chunk) => {
  const lines = (leftover + chunk.toString()).split('\n');
  leftover = lines.pop() ?? '';
  for (const line of lines) { handle(JSON.parse(line)); }
});
```

### Python: line iteration

```python
for raw in proc.stdout:
    if not raw.strip(): continue
    event = json.loads(raw)
    handle(event)
```

`subprocess.Popen(..., stdout=subprocess.PIPE, bufsize=1, text=True)` gives line-buffered text mode. Don't forget `bufsize=1`.

### All languages: stderr is informational

Lazar writes diagnostics to stderr (`[lazar] tool_use: ...`, `[lazar] invoke_end ...`) when `--verbose` is on. Capture stderr separately and surface it in a status line if you want, or discard.

When lazar exits non-zero WITHOUT having emitted an `error` event on stdout (rare — usually a panic), capture stderr to surface the real reason. Don't `io.Copy(io.Discard, stderr)` blindly.

## Markdown rendering

`text_delta` events stream raw markdown. Two strategies:

1. **Buffer per block, render on `text_done`**. Cleaner. The TUI's viewport gets a finished, properly-rendered chunk.
2. **Stream raw text live, no markdown.** Simpler but boring.

Live incremental markdown rendering (re-rendering as each delta arrives) looks great for short responses but flickers on long ones, especially around list markers (`-`, `1.`) and code fences. Strategy 1 is recommended unless you're optimizing for the wow factor.

## Recursion (depth > 0)

When lazar calls itself via `lazar -p "..."` from a tool, the child process emits its own stream of events with `depth: 1`. If the parent's `tool_result` content is itself JSONL events, the parent's TUI is reading "rendered events" from the child as text — not parsing them as nested events.

Most TUIs ignore this and just show the child's output as opaque tool_result content. That's fine. If you want nested rendering, parse the child JSONL inside the parent's tool_result handler. Almost no one needs this.

## Validation

Quick smoke test from your TUI's repo:

```bash
lazar -p "list 3 colors" --output-format stream-json | head -20
```

You should see `invoke_start`, then text_delta lines, then `text_done`, then `invoke_end`. If you see anything else, lazar's kernel may be older than this protocol — check `lazar --help`.
