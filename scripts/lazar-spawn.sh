#!/usr/bin/env bash
# lazar-spawn — the ONLY trusted sub-agent spawn surface.
#
# Installed to $LAZAR_HOME/bin/lazar-spawn by setup.sh (chmod 555; bin/ is not agent-writable under the sandbox).
# Lives outside agent-writable skills/; budget state lives under workspace/
# (sandbox cannot write logs/, but CAN write workspace/). Model-agnostic — no provider-specific logic.
#
# Usage: lazar-spawn [--lint-only] [--detach] <task-id> <prompt-file>
#        lazar-spawn --status|--await <job-id>
#
# Prompt file requirements (context-blind child):
#   - at least one absolute path
#   - a SUCCESS: line
#   - an EXPECT-VERIFY: block with indented claims:
#       exists=/abs | contains=/abs:needle | size_at_least=/abs:bytes
#
# Emits a JSON envelope on stdout. The wrapper re-checks EXPECT-VERIFY
# against the filesystem — the child's self-report is not trusted.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

emit_err(){ printf '{"status":"error","reason":%s}\n' "$(printf '%s' "$1" | jq -R .)"; exit 1; }

LINT_ONLY=0
DETACH=0
STATUS_ONLY=0
AWAIT_ID=""
while [ $# -gt 0 ]; do
  case "${1:-}" in
    --lint-only) LINT_ONLY=1; shift ;;
    --detach) DETACH=1; shift ;;
    --status) STATUS_ONLY=1; shift; AWAIT_ID="${1:-}"; shift || true ;;
    --await) shift; AWAIT_ID="${1:-}"; shift || true; STATUS_ONLY=0; WAIT_MODE=1 ;;
    -*) emit_err "unknown flag: $1" ;;
    *) break ;;
  esac
done
WAIT_MODE="${WAIT_MODE:-0}"
if [ "$STATUS_ONLY" = "1" ] || [ "$WAIT_MODE" = "1" ]; then
  :
elif [ $# -ne 2 ]; then
  emit_err "usage: lazar-spawn [--lint-only] [--detach] <task-id> <prompt-file> | --status|--await <job-id>"
fi
# Status / await path (job files under workspace/subagent/jobs/)
if [ -n "$AWAIT_ID" ]; then
  STATE_BASE="$LAZAR_HOME/workspace/subagent"
  JOB_DIR="$STATE_BASE/jobs"
  JOB="$JOB_DIR/$AWAIT_ID.json"
  [ -f "$JOB" ] || emit_err "unknown job id: $AWAIT_ID"
  if [ "$STATUS_ONLY" = "1" ]; then
    cat "$JOB"
    exit 0
  fi
  # --await: poll until done or timeout
  DEADLINE=$(( $(date +%s) + ${LAZAR_SPAWN_TIMEOUT:-300} ))
  while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    ST=$(jq -r '.status // empty' "$JOB" 2>/dev/null || true)
    case "$ST" in
      ok|fail|error) cat "$JOB"; [ "$ST" = "ok" ] && exit 0 || exit 2 ;;
    esac
    sleep 1
  done
  emit_err "await timed out for job $AWAIT_ID"
fi

TASK_ID="$1"; PROMPT_FILE="$2"
[ -f "$PROMPT_FILE" ] || emit_err "prompt file not found: $PROMPT_FILE"
PROMPT="$(cat "$PROMPT_FILE")"

# --- Lint ---
printf '%s' "$PROMPT" | rg -q '(^|[[:space:]])/[^[:space:]]+' \
  || emit_err "prompt has no absolute path — name files by full path, not 'the file'"
printf '%s' "$PROMPT" | rg -q '^SUCCESS:' \
  || emit_err "prompt has no SUCCESS: line — write the success criterion"
printf '%s' "$PROMPT" | rg -q '^EXPECT-VERIFY:' \
  || emit_err "prompt has no EXPECT-VERIFY: block — declare the claims the child must satisfy"

if [ "$LINT_ONLY" = "1" ]; then printf '{"status":"ok","lint":"passed"}\n'; exit 0; fi

# --- Budget state: workspace/subagent (logs/ is read-only under sandbox) ---
# Tests may set LAZAR_SPAWN_TEST=1 + LAZAR_SUBAGENT_STATE=<tmpdir>.
if [ "${LAZAR_SPAWN_TEST:-0}" = "1" ] && [ -n "${LAZAR_SUBAGENT_STATE:-}" ]; then
  STATE="$LAZAR_SUBAGENT_STATE"
else
  STATE="$LAZAR_HOME/workspace/subagent"
  # Refuse redirecting production state into agent-writable trees.
  case "${LAZAR_SUBAGENT_STATE:-}" in
    */skills/*)
      emit_err "refusing LAZAR_SUBAGENT_STATE under skills/; use workspace/subagent or LAZAR_SPAWN_TEST=1"
      ;;
  esac
fi
mkdir -p "$STATE"
CEILING="${LAZAR_SPAWN_CEILING:-2}"
BUDGET="${LAZAR_SPAWN_BUDGET:-10}"

# --- Session budget (flock when available) ---
SESS_KEY="${LAZAR_SESSION_ID:-ppid-$PPID}"
SESS_KEY=$(printf '%s' "$SESS_KEY" | tr -c 'A-Za-z0-9._-' '_')
SESS="$STATE/.session-${SESS_KEY}-spawns"
LOCK="$STATE/.session-${SESS_KEY}.lock"

bump_session(){
  local count
  count=$(cat "$SESS" 2>/dev/null || echo 0)
  [ "$count" -ge "$BUDGET" ] && return 1
  echo $((count+1)) > "$SESS"
  return 0
}

if command -v flock >/dev/null 2>&1; then
  exec 9>"$LOCK"
  flock 9 || emit_err "could not lock session spawn budget"
  bump_session || emit_err "session spawn budget ($BUDGET) exhausted — stop and reassess"
else
  bump_session || emit_err "session spawn budget ($BUDGET) exhausted — stop and reassess"
fi

# --- Per-task/prompt ceiling ---
KEY=$(printf '%s' "$TASK_ID"$'\x00'"$PROMPT" | shasum -a 256 | cut -d' ' -f1)
CFILE="$STATE/ceil-$KEY"
FAILS=$(cat "$CFILE" 2>/dev/null || echo 0)
[ "$FAILS" -ge "$CEILING" ] && emit_err "task '$TASK_ID' hit failure ceiling ($CEILING) — fix the task, don't respawn"

# --- Test hook: simulate a failed attempt without a real child ---
if [ "${SPAWN_TEST_NOOP:-}" = "fail" ]; then echo $((FAILS+1)) > "$CFILE"; emit_err "noop-fail (test)"; fi

# --- Bounded child spawn ---
LAZAR_BIN="${LAZAR_BIN:-$LAZAR_HOME/bin/lazar}"
TIMEOUT_CMD="$(command -v timeout || command -v gtimeout || true)"
KILL_AFTER="${LAZAR_SPAWN_TIMEOUT:-300}"
CHILD_DEPTH=$(( ${LAZAR_DEPTH:-0} + 1 ))
OUTFILE="$STATE/out-$KEY.log"

JOB_DIR="$STATE/jobs"
mkdir -p "$JOB_DIR"
JOB_ID="${TASK_ID}-$(date +%s)-$$"
JOB_FILE="$JOB_DIR/$JOB_ID.json"

run_child_fg(){
  if [ -n "$TIMEOUT_CMD" ]; then
    "$TIMEOUT_CMD" "$KILL_AFTER" env LAZAR_DEPTH="$CHILD_DEPTH" "$LAZAR_BIN" -p "$PROMPT" --output-format stream-json
  else
    env LAZAR_DEPTH="$CHILD_DEPTH" "$LAZAR_BIN" -p "$PROMPT" --output-format stream-json
  fi
}

finish_job(){
  local matched="$1" rc="$2" failed_json="$3"
  if [ "$matched" = "true" ]; then
    jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$rc" --arg out "$OUTFILE"       '{status:"ok",job_id:$id,task:$task,matched:true,child_rc:$rc,output:$out}' >"$JOB_FILE"
  else
    jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$rc" --arg out "$OUTFILE" --argjson failed "$failed_json"       '{status:"fail",job_id:$id,task:$task,matched:false,child_rc:$rc,failed_claims:$failed,output:$out}' >"$JOB_FILE"
  fi
}

if [ "$DETACH" = "1" ]; then
  jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --arg out "$OUTFILE"     '{status:"running",job_id:$id,task:$task,output:$out,pid:0}' >"$JOB_FILE"
  (
    run_child_fg >"$OUTFILE" 2>/dev/null
    RC=$?
    # claim check in subshell
    MATCHED=true
    FAILED=()
    while IFS= read -r claim; do
      claim="$(printf '%s' "$claim" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
      [ -z "$claim" ] && continue
      case "$claim" in
        exists=*) [ -e "${claim#exists=}" ] || { MATCHED=false; FAILED+=("$claim"); } ;;
        contains=*) pn="${claim#contains=}"; p="${pn%%:*}"; n="${pn#*:}"; [ -f "$p" ] && rg -qF "$n" "$p" || { MATCHED=false; FAILED+=("$claim"); } ;;
        size_at_least=*) pn="${claim#size_at_least=}"; p="${pn%%:*}"; min="${pn#*:}"; [ -f "$p" ] && [ "$(wc -c <"$p")" -ge "$min" ] || { MATCHED=false; FAILED+=("$claim"); } ;;
        *) MATCHED=false; FAILED+=("$claim") ;;
      esac
    done < <(printf '%s' "$PROMPT" | awk '/^EXPECT-VERIFY:/{f=1;next} f{ if($0 ~ /^[[:space:]]/) print; else f=0 }')
    FAILED_JSON=$(printf '%s
' "${FAILED[@]:-}" | jq -R . | jq -s 'map(select(length>0))')
    if [ "$MATCHED" = "true" ]; then
      jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$RC" --arg out "$OUTFILE"         '{status:"ok",job_id:$id,task:$task,matched:true,child_rc:$rc,output:$out}' >"$JOB_FILE"
    else
      echo $((FAILS+1)) > "$CFILE"
      jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$RC" --arg out "$OUTFILE" --argjson failed "$FAILED_JSON"         '{status:"fail",job_id:$id,task:$task,matched:false,child_rc:$rc,failed_claims:$failed,output:$out}' >"$JOB_FILE"
    fi
  ) &
  BG_PID=$!
  jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --arg out "$OUTFILE" --argjson pid "$BG_PID"     '{status:"running",job_id:$id,task:$task,output:$out,pid:$pid}' >"$JOB_FILE"
  printf '{"status":"running","job_id":%s,"task":%s,"pid":%s,"output":%s}
'     "$(printf '%s' "$JOB_ID"|jq -R .)" "$(printf '%s' "$TASK_ID"|jq -R .)" "$BG_PID" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 0
fi

run_child_fg >"$OUTFILE" 2>/dev/null; RC=$?
if [ "$RC" -eq 124 ]; then echo $((FAILS+1)) > "$CFILE"; emit_err "child timed out after ${KILL_AFTER}s (wedged) — killed"; fi

# --- Independently re-check parent's claims (wrapper is the judge) ---
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
while IFS= read -r claim; do
  claim="$(printf '%s' "$claim" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
  [ -z "$claim" ] && continue
  NCLAIMS=$((NCLAIMS+1))
  if ! check_one "$claim"; then MATCHED=false; FAILED+=("$claim"); fi
done < <(printf '%s' "$PROMPT" | awk '/^EXPECT-VERIFY:/{f=1;next} f{ if($0 ~ /^[[:space:]]/) print; else f=0 }')

if [ "$NCLAIMS" -eq 0 ]; then
  echo $((FAILS+1)) > "$CFILE"
  emit_err "EXPECT-VERIFY block parsed to zero claims — malformed prompt, refusing to declare success"
fi

FAILED_JSON=$(printf '%s\n' "${FAILED[@]:-}" | jq -R . | jq -s 'map(select(length>0))')

if [ "$MATCHED" = "true" ]; then
  jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$RC" --arg out "$OUTFILE"     '{status:"ok",job_id:$id,task:$task,matched:true,child_rc:$rc,output:$out}' >"$JOB_FILE" 2>/dev/null || true
  printf '{"status":"ok","job_id":%s,"task":%s,"matched":true,"child_rc":%s,"output":%s}\n'     "$(printf '%s' "$JOB_ID"|jq -R .)" "$(printf '%s' "$TASK_ID"|jq -R .)" "$RC" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 0
else
  echo $((FAILS+1)) > "$CFILE"
  jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$RC" --arg out "$OUTFILE" --argjson failed "$FAILED_JSON"     '{status:"fail",job_id:$id,task:$task,matched:false,child_rc:$rc,failed_claims:$failed,output:$out}' >"$JOB_FILE" 2>/dev/null || true
  printf '{"status":"fail","job_id":%s,"task":%s,"matched":false,"child_rc":%s,"failed_claims":%s,"output":%s}\n'     "$(printf '%s' "$JOB_ID"|jq -R .)" "$(printf '%s' "$TASK_ID"|jq -R .)" "$RC" "$FAILED_JSON" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 2
fi
