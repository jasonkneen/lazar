---
name: health-check
description: Verify lazar is installed correctly and all load-bearing behaviors work end-to-end. Runs static checks (kernel locked, env vars, skills present), boundary tests (sandbox enforcement), live behavior tests (log auto-rotation, memory persistence, recursion), and short end-to-end builds in TypeScript and Python. Runs in a temp directory under workspace/, cleans up after itself, prints PASS/FAIL with a summary at the end. Use after install, after kernel rebuild, after --reset-all, or anytime something feels off.
---

# health-check

Verify lazar is installed correctly and all the load-bearing behaviors work end-to-end. Runs in a temp directory so it doesn't pollute real state.

## When to use

- Right after install or `setup.sh`, to confirm everything works.
- After a kernel rebuild, to verify the patch didn't break anything.
- After `--reset-all`, to validate fresh state.
- When something feels off and you want a baseline.
- Periodically as a smoke test before a long session.

## How to use

Run the recipes in order. Each check prints `[PASS]`, `[FAIL: <reason>]`, or `[SKIP: <reason>]` for things that aren't applicable on this system. Print a summary at the end with the PASS/FAIL count.

If any FAIL appears, the user should investigate before doing real work — most failures point at a misconfiguration that will silently bite later.

## Setup

```bash
HC="$LAZAR_HOME/workspace/.health-check-$(date +%s)"
mkdir -p "$HC"
echo "[health-check] starting in $HC"
PASS=0; FAIL=0; SKIP=0
pass() { echo "  [PASS] $1"; PASS=$((PASS+1)); }
fail() { echo "  [FAIL] $1"; FAIL=$((FAIL+1)); }
skip() { echo "  [SKIP] $1"; SKIP=$((SKIP+1)); }
```

## 1. Static checks (kernel + filesystem)

```bash
echo "── 1. static checks ────────────────────────────────"

# kernel binary
[ -f "$LAZAR_HOME/bin/lazar" ] && pass "kernel binary at bin/lazar" || fail "no kernel binary"

# kernel locked (macOS chflags uchg)
ls -lO "$LAZAR_HOME/bin/lazar" 2>/dev/null | grep -q uchg \
    && pass "kernel binary is uchg-locked (immutable)" \
    || fail "kernel binary is NOT immutable — chflags uchg should be set"

# source not user-writable
TESTSRC="$LAZAR_HOME/src/.health-write-probe"
touch "$TESTSRC" 2>/dev/null && { rm -f "$TESTSRC"; fail "src/ is writable — should be chmod a-w"; } \
    || pass "src/ is read-only"

# env vars from kernel
[ -n "$LAZAR_HOME" ]      && pass "LAZAR_HOME=$LAZAR_HOME"           || fail "LAZAR_HOME unset"
[ -n "$LAZAR_SKILLS" ]    && pass "LAZAR_SKILLS=$LAZAR_SKILLS"       || fail "LAZAR_SKILLS unset"
[ -n "$LAZAR_MEMORY" ]    && pass "LAZAR_MEMORY=$LAZAR_MEMORY"       || fail "LAZAR_MEMORY unset"
[ -n "$LAZAR_WORKSPACE" ] && pass "LAZAR_WORKSPACE=$LAZAR_WORKSPACE" || fail "LAZAR_WORKSPACE unset"
[ -n "$LAZAR_LOGS" ]      && pass "LAZAR_LOGS=$LAZAR_LOGS"           || fail "LAZAR_LOGS unset"

# api key
[ -n "$ANTHROPIC_API_KEY" ] && pass "ANTHROPIC_API_KEY is set" || fail "ANTHROPIC_API_KEY not set"

# seed skills present in live skills/
for s in create-skill find-capability shell-handoff load-context log-rotation archive-search distill propose-kernel-patch project-context create-tui health-check; do
    [ -f "$LAZAR_SKILLS/_meta/$s/SKILL.md" ] && pass "skill _meta/$s" || fail "skill _meta/$s missing"
done
[ -f "$LAZAR_SKILLS/memory/SKILL.md" ] && pass "skill memory" || fail "skill memory missing"
[ -f "$LAZAR_SKILLS/INDEX.md" ]        && pass "skills/INDEX.md present" || fail "INDEX.md missing"
```

## 2. Sandbox boundary tests

The sandbox profile should deny writes to `bin/` and `src/` while allowing writes to `skills/`, `memory/`, `workspace/`, `logs/`, `/tmp`. Validate by trying.

```bash
echo "── 2. sandbox boundary tests ───────────────────────"

# DENY: bin/
echo probe > "$LAZAR_HOME/bin/.health-probe" 2>/dev/null \
    && { rm -f "$LAZAR_HOME/bin/.health-probe"; fail "could write to bin/ — sandbox broken"; } \
    || pass "write to bin/ correctly denied"

# DENY: src/
echo probe > "$LAZAR_HOME/src/.health-probe" 2>/dev/null \
    && { rm -f "$LAZAR_HOME/src/.health-probe"; fail "could write to src/ — sandbox broken"; } \
    || pass "write to src/ correctly denied"

# ALLOW: workspace/
echo probe > "$HC/.write-probe" \
    && { rm -f "$HC/.write-probe"; pass "write to workspace/ allowed"; } \
    || fail "could not write to workspace/"

# ALLOW: memory/
PROBE="$LAZAR_MEMORY/.health-probe-$(date +%s).md"
echo "# probe" > "$PROBE" \
    && { pass "write to memory/ allowed"; rm -f "$PROBE"; } \
    || fail "could not write to memory/"
```

## 3. Memory persistence (read-after-write)

```bash
echo "── 3. memory persistence ───────────────────────────"

PROBE="$LAZAR_MEMORY/.health-probe-$(date +%s).md"
MAGIC="lazar-health-check-$(date +%s)-$$"
echo "# $MAGIC" > "$PROBE"
grep -q "$MAGIC" "$PROBE" && pass "memory write+read" || fail "memory readback failed"
rm -f "$PROBE"
```

## 4. Log auto-rotation (kernel safety floor)

Set a tiny threshold and trigger a rotation by running a small lazar invocation. The kernel checks size on every append, so any event after the threshold rotates.

```bash
echo "── 4. log auto-rotation ────────────────────────────"

BEFORE=$(ls "$LAZAR_LOGS"/stream.jsonl.*.bak 2>/dev/null | wc -l | tr -d ' ')

# tiny threshold so the next event triggers rotation
LAZAR_LOG_MAX_BYTES=1 lazar -p "echo health-check-rotation-probe" --output-format text >/dev/null 2>&1

AFTER=$(ls "$LAZAR_LOGS"/stream.jsonl.*.bak 2>/dev/null | wc -l | tr -d ' ')
if [ "$AFTER" -gt "$BEFORE" ]; then
    pass "log auto-rotation fired ($BEFORE -> $AFTER archives)"
    # confirm the kernel wrote a summary too
    NEWEST=$(ls -t "$LAZAR_HOME/memory/log-summaries/"*.md 2>/dev/null | head -n 1)
    [ -f "$NEWEST" ] && pass "rotation summary written: $(basename "$NEWEST")" || fail "no log-summary written"
else
    fail "log did not rotate despite tiny threshold"
fi
```

## 5. Recursion (lazar calling itself)

```bash
echo "── 5. recursion (lazar -p inside a tool call) ──────"

OUT=$(lazar -p "run this exact command and tell me the result: lazar -p 'echo HEALTH_RECURSION_OK_$$'" --verbose 2>&1 || true)
echo "$OUT" | grep -q "HEALTH_RECURSION_OK_$$" \
    && pass "recursion works (depth>0 invocation succeeded)" \
    || fail "recursion did not produce expected output"
```

## 6. Empty-command safeguard

We can't easily make the model emit an empty tool_use, but we can verify the kernel symbol is in place by checking the binary mentions the safeguard string:

```bash
echo "── 6. kernel safeguards present ────────────────────"

strings "$LAZAR_HOME/bin/lazar" 2>/dev/null | grep -q "empty or missing 'command' field" \
    && pass "empty-command safeguard compiled in" \
    || fail "empty-command safeguard not found in binary (rebuild needed?)"

strings "$LAZAR_HOME/bin/lazar" 2>/dev/null | grep -q "auto-rotated stream.jsonl" \
    && pass "auto-rotation code path compiled in" \
    || fail "auto-rotation not found in binary"
```

## 7. End-to-end: TypeScript

```bash
echo "── 7. e2e: typescript ──────────────────────────────"

if command -v npx >/dev/null 2>&1; then
    mkdir -p "$HC/ts" && cd "$HC/ts"
    mkdir -p .npm && echo "cache=$(pwd)/.npm" > .npmrc
    echo 'console.log("HC_TS_OK")' > test.ts
    if npx -y --quiet tsx test.ts 2>/dev/null | grep -q "HC_TS_OK"; then
        pass "tsx run works (TypeScript toolchain healthy)"
    else
        fail "tsx run failed — check Node + npx + npm cache"
    fi
    cd - >/dev/null
else
    skip "npx not installed — TypeScript stack unavailable"
fi
```

## 8. End-to-end: Python

```bash
echo "── 8. e2e: python venv ─────────────────────────────"

if command -v python3 >/dev/null 2>&1; then
    mkdir -p "$HC/py" && cd "$HC/py"
    python3 -m venv .venv 2>/dev/null
    if [ -x .venv/bin/python ] && .venv/bin/python -c "print('HC_PY_OK')" 2>/dev/null | grep -q "HC_PY_OK"; then
        pass "python3 venv works"
    else
        fail "python3 venv failed"
    fi
    cd - >/dev/null
else
    skip "python3 not installed"
fi
```

## 9. End-to-end: skill discovery via INDEX

```bash
echo "── 9. skill index reachable ────────────────────────"

cat "$LAZAR_SKILLS/INDEX.md" | grep -q "_meta/health-check" \
    && pass "health-check is registered in INDEX.md" \
    || fail "health-check missing from INDEX — agent won't find this skill"
```

## 10. Cleanup + summary

```bash
echo "── cleanup ─────────────────────────────────────────"
rm -rf "$HC"
echo "[health-check] removed temp dir $HC"

echo
echo "════════════════════════════════════════════════════"
echo "  health-check summary"
echo "    PASS: $PASS"
echo "    FAIL: $FAIL"
echo "    SKIP: $SKIP"
echo "════════════════════════════════════════════════════"

if [ "$FAIL" -gt 0 ]; then
    echo "  ⚠  $FAIL check(s) failed. Investigate before doing real work."
    exit 1
else
    echo "  ✓  all checks passed."
    exit 0
fi
```

## Hard rules

- **Run only inside `workspace/`.** All temp state goes under `workspace/.health-check-<ts>/`. Never write to `skills/`, `memory/`, `bin/`, `src/`, or `~/`.
- **Clean up.** Remove the temp dir on success AND on failure. Don't leave health-check probes scattered.
- **Don't run on every prompt.** Once after install, then on demand. The recursion test alone makes a real API call.
- **Skip gracefully.** If python3 or npx aren't installed, mark SKIP not FAIL — toolchain absence isn't a kernel bug.

## Anti-patterns

- Don't add slow tests here (e.g. full TUI build, model-comparison evals). Health-check should run in under a minute.
- Don't write to memory/ outside the temp probe pattern. The user's real memory is sacred.
- Don't fail the whole check on a single SKIP — only FAIL counts against the kernel's health.

## Extending

If you discover a new class of failure that's not caught above, add a section to this skill — that's the lesson encoded so the next health-check catches it. Treat this skill as a living regression suite for the kernel.
