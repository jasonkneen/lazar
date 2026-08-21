# provider-failover

Use the installed `bin/lazar-failover` wrapper for a cold one-shot prompt when
provider availability matters:

```bash
printf '%s\n' 'Inspect the checkout and report the failing tests.' > "$LAZAR_WORKSPACE/failover-prompt.txt"
"$LAZAR_HOME/bin/lazar-failover" "$LAZAR_WORKSPACE/failover-prompt.txt" anthropic minimax
```

Omit provider IDs to try every key-available entry from `lazar providers
--json`, in catalog order. Provider names and model identifiers come from the
live catalog; do not invent or validate them from memory.

Safety rules:

- The prompt file must be at most 9000 bytes.
- The wrapper retries only when a failed attempt produced valid stream JSON and
  no `tool_use`, `tool_result`, or `hook_start` event.
- Once any tool or hook event appears, stop. Retrying the whole prompt could repeat a
  filesystem mutation.
- This is cold one-shot failover. It clears inherited session and model state,
  resets invocation depth, and records hook dispatch before allowing a retry.
  It does not continue or migrate a session.
- A provider refusal or other terminal model response is a successful model
  outcome, not an infrastructure failure.

Run `bash "$LAZAR_HOME/user/skills/provider-failover/selftest.sh"` to test the
wrapper with fake providers and no network access.
