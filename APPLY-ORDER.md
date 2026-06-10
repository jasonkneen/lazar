# Apply order — full session work

Everything you decided to ship rolled up. Run these blocks in order. If
any block fails, stop and inspect — don't barrel through.

---

## 0. Sanity check (10 seconds)

```bash
ls ~/lazar/workspace/proposals/
# expect ONE active proposal: no-sandbox-flag
# (everything else under .archive/)

ls ~/lazar/workspace/hook-staging/heartbeat-v2/
# expect: README.md, tick.sh, tick.d/01-pulse.sh, 02-tombstone.sh, 03-memory-hygiene.sh

ls -la ~/lazar/workspace/tui/main.go.bak.subagent-fix
# expect: backup exists (means TUI subagent fix was applied)
```

---

## 1. Apply the no-sandbox-flag bundle (kernel)

This is the big one. Includes:
- `--no-sandbox` flag + `LAZAR_NO_SANDBOX=1` env (with inheritance through nested `lazar -p`)
- Tool renamed `execute` → `bash` (model priors)
- Verifier with unicode normalization (closes the U+2011 fabrication vector)
- Bash-fence fallback (recovers from non-Anthropic providers emitting markdown shells)
- Session module (multi-turn continuity, cross-provider safe)
- Hooks module
- Sandbox patch with `~/.codex` / `~/.claude` writes

```bash
cd ~/lazar
bash scripts/kernel-apply.sh no-sandbox-flag
```

It prints a diff, asks `Apply this proposal? [y/N]`, backs up your
current `src/` to `workspace/kernel-backups/src.<timestamp>/`,
rebuilds, and re-locks the binary. Auto-rollback if the build fails.

After it succeeds:

```bash
# confirm bash tool name is in effect
lazar -p "what tool do you have?" --verbose 2>&1 | head -10

# confirm no-sandbox banner fires
lazar --no-sandbox -p "echo hello" --verbose 2>&1 | head -20

# confirm sandbox is restored when flag absent
lazar -p "touch /etc/probe" 2>&1 | tail -5
# expect: Operation not permitted
```

---

## 2. Install heartbeat-v2 (hooks/tick.d/ migration)

Puts pulse/tombstone/memory-hygiene into pure-bash hooks.
Routine ticks stop costing API tokens.

```bash
# Install the three pulse hooks
mkdir -p ~/lazar/hooks/tick.d
cp ~/lazar/workspace/hook-staging/heartbeat-v2/tick.d/*.sh \
   ~/lazar/hooks/tick.d/
chmod +x ~/lazar/hooks/tick.d/*.sh

# Back up old tick.sh and swap in the new one
cp ~/lazar/skills/_meta/heartbeat/tick.sh \
   ~/lazar/skills/_meta/heartbeat/tick.sh.bak.v1
cp ~/lazar/workspace/hook-staging/heartbeat-v2/tick.sh \
   ~/lazar/skills/_meta/heartbeat/tick.sh
chmod +x ~/lazar/skills/_meta/heartbeat/tick.sh

# Test once manually
bash ~/lazar/skills/_meta/heartbeat/tick.sh

# Verify a journal line appeared and no LLM was called
tail -10 ~/lazar/memory/heartbeat/journal.md
# expect: a "tick X — <ts>" entry with `mode: bash-only-tick`
```

Your existing launchd plist still calls `tick.sh`, so no plist changes
needed. Future ticks fire the bash hooks instead of an LLM loop.

To re-add LLM-using phases (distill, gap-fill, discover) selectively,
see the README at `workspace/hook-staging/heartbeat-v2/README.md` — it
shows the pattern.

Rollback if anything breaks:

```bash
cp ~/lazar/skills/_meta/heartbeat/tick.sh.bak.v1 \
   ~/lazar/skills/_meta/heartbeat/tick.sh
```

---

## 3. Rebuild the TUI (subagent fix + session support)

```bash
cd ~/lazar/workspace/tui
go build -o lazartui
```

If it builds clean, the subagent feature is working again AND the
`--session` continuity work is in. If you get errors, paste them — but
the `bash -n` syntax checks passed and the three known-broken refs
(subagentStatusMtime, tickSubagents-as-Cmd, return signature) were
all addressed.

After building, test one end-to-end TUI session:

```bash
./lazartui
# in TUI: ask "remember 88"
# next turn: ask "what number?"
# expect: 88, with sessionID visible somewhere in your TUI status
```

---

## 4. (Optional) Operator-side CLI updates for sub-agents

These don't need lazar, just your shell. Yesterday's tests showed both
were external blockers:

```bash
# codex 0.61 returns 404 on default model — upgrade
brew upgrade codex 2>/dev/null || npm i -g @openai/codex

# claude credentials expired — re-auth
claude /login
```

After both, retest:

```bash
bash ~/lazar/skills/agent-launcher/agents/codex.sh "what is 2+2"
bash ~/lazar/skills/agent-launcher/agents/claude-code.sh "what time is it"
```

Both should now return real answers.

---

## 5. Commit + push the kernel work

```bash
cd ~/lazar
git status --short

# expect (rough):
#  M .gitignore
#  M src/Cargo.lock
#  M src/sandbox.sb
#  M src/src/main.rs
#  M src/src/hooks.rs
# ?? src/src/session.rs
# ?? src/src/verifier.rs
# ?? src/seed-hooks/

git add -A

git commit -m "Hooks, sessions, verifier, --no-sandbox, bash tool

Major architectural pass:

- Hooks: 8 lifecycle events, drop-in .d/ scripts, sandboxed
- Sessions: --session <id> and --resume for multi-turn continuity;
  cross-provider tool-id safe (strips tool blocks on load)
- Verifier: kernel-side fabrication detection. Pattern detector +
  ironclad VERIFY-block engine that runs filesystem checks
  (stat/exists/contains) on agent-declared claims. Unicode
  normalization closes the non-breaking-hyphen substitution vector.
- --no-sandbox / LAZAR_NO_SANDBOX: operator-controlled bypass of
  sandbox-exec for working in non-lazar directories. Loud banner;
  agent cannot self-enable.
- Tool rename: execute() -> bash() for stronger model priors.
  Both names accepted on input (legacy compat).
- Sandbox patch: ~/.codex and ~/.claude writable so sub-agent CLIs
  can update session state from inside the sandbox.
- Bash-fence fallback: synthesizes tool_use from markdown bash
  fences when models degrade after seeing tool_result history.
- SOUL.md: AUTONOMY MANDATE + TRUTH MANDATE (4-tier action rubric,
  anti-fabrication rules).
- Operator inbox channel: ~/lazar/memory/INBOX.md + session-start
  hook for out-of-band operator-to-agent notification."

git push origin main
```

---

## 6. Smoke test the integrated whole

After all the above:

```bash
# Test 1: bash tool name + sandbox + verify all working together
lazar --no-sandbox -p "create /tmp/lazar-final-test with contents 'shipped'. End with a [VERIFY] block."
# expect: ✓ verify: ... claims passed

# Test 2: sessions work in the TUI
./lazartui
# multi-turn with referential prompts; should resolve correctly

# Test 3: heartbeat is now zero-LLM for routine ticks
tail -f ~/lazar/memory/heartbeat/journal.md &
# wait for next launchd-fired tick
# expect: new "tick N — <ts>" line with `mode: bash-only-tick`
# NO model call should fire (check stream.jsonl for hook_start of tick)
```

---

## What happens if something fails

Each step has its own backup:

| Step | Backup |
|---|---|
| 1 | `~/lazar/workspace/kernel-backups/src.<timestamp>/` |
| 2 | `~/lazar/skills/_meta/heartbeat/tick.sh.bak.v1` |
| 3 | `~/lazar/workspace/tui/main.go.bak.subagent-fix` |
| 5 | `git reset --hard HEAD~1` if commit was wrong |

Don't try to fix everything at once. Stop at the first failure and
inspect.

---

## Final state after this completes

```
✓ Hooks live (8 events, multiple active scripts)
✓ Sessions live (multi-turn continuity, cross-provider safe)
✓ Verifier live (filesystem outcomes structurally honest)
✓ --no-sandbox flag (operator escapes sandbox for project work)
✓ bash tool name (stronger model priors)
✓ Heartbeat-v2 (routine ticks cost 0 API tokens)
✓ TUI rebuilt with --session + subagent code re-enabled
✓ codex/claude-code working (after CLI updates)
✓ Repo pushed
✓ INBOX channel for operator handoff
✓ AUTONOMY + TRUTH mandates in SOUL.md
```

That's everything from this session. After step 6 verifies, you can stop.
