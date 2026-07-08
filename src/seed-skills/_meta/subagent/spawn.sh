#!/usr/bin/env bash
# spawn.sh — the ONLY sanctioned way to spawn a lazar sub-agent.
# Usage: spawn.sh [--lint-only] <task-id> <prompt-file>
#
# The prompt file must be a self-contained task a context-blind agent can do:
#   - at least one absolute path (name files by full path, never "the file")
#   - a `SUCCESS:` line (the criterion, one sentence)
#   - an `EXPECT-VERIFY:` block: claims that must become true, one per indented
#     line, in `exists=/abs`, `contains=/abs:needle`, or `size_at_least=/abs:bytes`.
#
# spawn.sh lints the prompt, bounds retries (per-task ceiling + session budget),
# wall-clock-kills a wedged child, then RE-CHECKS the EXPECT-VERIFY claims itself
# against the filesystem (the wrapper is the judge — the child's self-report is
# not trusted). Emits a result envelope JSON on stdout.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

emit_err(){ printf '{"status":"error","reason":%s}\n' "$(printf '%s' "$1" | jq -R .)"; exit 1; }

LINT_ONLY=0
if [ "${1:-}" = "--lint-only" ]; then LINT_ONLY=1; shift; fi
[ $# -eq 2 ] || emit_err "usage: spawn.sh [--lint-only] <task-id> <prompt-file>"
TASK_ID="$1"; PROMPT_FILE="$2"
[ -f "$PROMPT_FILE" ] || emit_err "prompt file not found: $PROMPT_FILE"
PROMPT="$(cat "$PROMPT_FILE")"

# --- Lint: a context-blind child needs concrete, verifiable instructions ---
printf '%s' "$PROMPT" | rg -q '(^|[[:space:]])/[^[:space:]]+' \
  || emit_err "prompt has no absolute path — name files by full path, not 'the file'"
printf '%s' "$PROMPT" | rg -q '^SUCCESS:' \
  || emit_err "prompt has no SUCCESS: line — write the success criterion"
printf '%s' "$PROMPT" | rg -q '^EXPECT-VERIFY:' \
  || emit_err "prompt has no EXPECT-VERIFY: block — declare the claims the child must satisfy"

if [ "$LINT_ONLY" = "1" ]; then printf '{"status":"ok","lint":"passed"}\n'; exit 0; fi

# --- Runtime state (overridable for tests) ---
STATE="${LAZAR_SUBAGENT_STATE:-$LAZAR_HOME/skills/_meta/subagent/state}"
mkdir -p "$STATE"
CEILING="${LAZAR_SPAWN_CEILING:-2}"
BUDGET="${LAZAR_SPAWN_BUDGET:-10}"

# --- Session budget (absolute backstop; prefer session id when set) ---
SESS_KEY="${LAZAR_SESSION_ID:-ppid-$PPID}"
# sanitize for filename
SESS_KEY=$(printf '%s' "$SESS_KEY" | tr -c 'A-Za-z0-9._-' '_')
SESS="$STATE/.session-${SESS_KEY}-spawns"
COUNT=$(cat "$SESS" 2>/dev/null || echo 0)
[ "$COUNT" -ge "$BUDGET" ] && emit_err "session spawn budget ($BUDGET) exhausted — stop and reassess"

# --- Per-task/prompt ceiling (retry bound) ---
KEY=$(printf '%s' "$TASK_ID"$'\x00'"$PROMPT" | shasum -a 256 | cut -d' ' -f1)
CFILE="$STATE/ceil-$KEY"
FAILS=$(cat "$CFILE" 2>/dev/null || echo 0)
[ "$FAILS" -ge "$CEILING" ] && emit_err "task '$TASK_ID' hit failure ceiling ($CEILING) — fix the task, don't respawn"

echo $((COUNT+1)) > "$SESS"

# --- Test hook: simulate a failed attempt without a real child ---
if [ "${SPAWN_TEST_NOOP:-}" = "fail" ]; then echo $((FAILS+1)) > "$CFILE"; emit_err "noop-fail (test)"; fi

# --- Bounded child spawn (stream-json so the kernel's verify_report is captured) ---
LAZAR_BIN="${LAZAR_BIN:-$LAZAR_HOME/bin/lazar}"
TIMEOUT_CMD="$(command -v timeout || command -v gtimeout || true)"
KILL_AFTER="${LAZAR_SPAWN_TIMEOUT:-300}"
CHILD_DEPTH=$(( ${LAZAR_DEPTH:-0} + 1 ))
OUTFILE="$STATE/out-$KEY.log"

run_child(){
  if [ -n "$TIMEOUT_CMD" ]; then
    "$TIMEOUT_CMD" "$KILL_AFTER" env LAZAR_DEPTH="$CHILD_DEPTH" "$LAZAR_BIN" -p "$PROMPT" --output-format stream-json
  else
    env LAZAR_DEPTH="$CHILD_DEPTH" "$LAZAR_BIN" -p "$PROMPT" --output-format stream-json
  fi
}
run_child >"$OUTFILE" 2>/dev/null; RC=$?
if [ "$RC" -eq 124 ]; then echo $((FAILS+1)) > "$CFILE"; emit_err "child timed out after ${KILL_AFTER}s (wedged) — killed"; fi

# --- Independently re-check the parent's declared claims (the wrapper is judge) ---
check_one(){
  local c="$1" p n min pn
  case "$c" in
    exists=*)        [ -e "${c#exists=}" ] ;;
    contains=*)      pn="${c#contains=}"; p="${pn%%:*}"; n="${pn#*:}"; [ -f "$p" ] && rg -qF "$n" "$p" ;;
    size_at_least=*) pn="${c#size_at_least=}"; p="${pn%%:*}"; min="${pn#*:}"; [ -f "$p" ] && [ "$(wc -c <"$p")" -ge "$min" ] ;;
    *) return 1 ;;
  esac
}
MATCHED=true
NCLAIMS=0
FAILED=()
# Portable extraction (BSD/GNU awk): indented lines after EXPECT-VERIFY: up to
# the next non-indented line.
while IFS= read -r claim; do
  claim="$(printf '%s' "$claim" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
  [ -z "$claim" ] && continue
  NCLAIMS=$((NCLAIMS+1))
  if ! check_one "$claim"; then MATCHED=false; FAILED+=("$claim"); fi
done < <(printf '%s' "$PROMPT" | awk '/^EXPECT-VERIFY:/{f=1;next} f{ if($0 ~ /^[[:space:]]/) print; else f=0 }')

# A parse that yields zero claims must NEVER report success — that is exactly
# the silent-pass this wrapper exists to prevent.
if [ "$NCLAIMS" -eq 0 ]; then
  echo $((FAILS+1)) > "$CFILE"
  emit_err "EXPECT-VERIFY block parsed to zero claims — malformed prompt, refusing to declare success"
fi

FAILED_JSON=$(printf '%s\n' "${FAILED[@]:-}" | jq -R . | jq -s 'map(select(length>0))')

if [ "$MATCHED" = "true" ]; then
  # Surface child_rc even on claim match so a crashed child with pre-existing
  # artifacts is visible (wrapper is still the judge for matched).
  printf '{"status":"ok","task":%s,"matched":true,"child_rc":%s,"output":%s}\n' \
    "$(printf '%s' "$TASK_ID"|jq -R .)" "$RC" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 0
else
  echo $((FAILS+1)) > "$CFILE"
  printf '{"status":"fail","task":%s,"matched":false,"child_rc":%s,"failed_claims":%s,"output":%s}\n' \
    "$(printf '%s' "$TASK_ID"|jq -R .)" "$RC" "$FAILED_JSON" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 2
fi
