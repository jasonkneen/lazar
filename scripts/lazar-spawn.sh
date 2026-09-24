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
#       any `lazar verify` claim: exists=, not_exists=, size=, size_at_least=,
#       size_at_most=, contains=, mtime_within= (absolute paths recommended)
#
# Emits a JSON envelope on stdout. The wrapper re-checks EXPECT-VERIFY
# against the filesystem — the child's self-report is not trusted.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
HELPER_DIR=$(cd "$(dirname "$0")" && pwd)

emit_err(){ printf '{"status":"error","reason":%s}\n' "$(printf '%s' "$1" | jq -R .)"; exit 1; }

# --- Claim checking: the kernel's checker (`lazar verify`), the same one that
# checks [VERIFY] blocks in every turn, so there is one claim language and one
# implementation. `check_claims` reads one claim per line on stdin and sets
# CLAIMS_MATCHED (true|false), CLAIMS_COUNT and the FAILED_CLAIMS array. The
# child's self-report is never consulted: every claim is re-checked against
# the real filesystem.
# LAZAR_BIN is the child runner (tests stub it); the checker is always a real
# kernel binary.
VERIFY_BIN="${LAZAR_VERIFY_BIN:-$LAZAR_HOME/bin/lazar}"
check_claims(){
  local report
  CLAIMS_MATCHED=false; CLAIMS_COUNT=0; FAILED_CLAIMS=()
  report=$("$VERIFY_BIN" verify 2>/dev/null) || true
  [ -n "$report" ] || { FAILED_CLAIMS+=("lazar verify produced no report"); return; }
  CLAIMS_COUNT=$(printf '%s' "$report" | jq '.claims | length')
  [ "$(printf '%s' "$report" | jq -r '.passed')" = "true" ] && CLAIMS_MATCHED=true
  while IFS= read -r failed; do
    [ -n "$failed" ] && FAILED_CLAIMS+=("$failed")
  done < <(printf '%s' "$report" | jq -r '.claims[] | select(.ok | not) | "\(.claim) — \(.detail)"')
}
failed_claims_json(){
  printf '%s\n' "${FAILED_CLAIMS[@]:-}" | jq -R . | jq -s 'map(select(length>0))'
}

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
  [[ "$AWAIT_ID" =~ ^[A-Za-z0-9._-]+$ ]] || emit_err "job id must be a safe slug"
  STATE_BASE="$LAZAR_HOME/user/workspace/subagent"
  JOB_DIR="$STATE_BASE/jobs"
  JOB="$JOB_DIR/$AWAIT_ID.json"
  [ -f "$JOB" ] || emit_err "unknown job id: $AWAIT_ID"
  # A missing supervisor cannot provide a trustworthy exit status.
  sweep_job(){
    local st pid
    st=$(jq -r '.status // empty' "$JOB")
    [ "$st" = "running" ] || return 0
    pid=$(jq -r '.pid // 0' "$JOB")
    [ "$pid" != "0" ] && kill -0 "$pid" 2>/dev/null && return 0
    jq '. + {status:"error",matched:false,reason:"job supervisor exited without a final record"}' "$JOB" >"$JOB.tmp" && mv "$JOB.tmp" "$JOB"
  }
  sweep_job
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
    sweep_job
    sleep 1
  done
  emit_err "await timed out for job $AWAIT_ID"
fi

TASK_ID="$1"; PROMPT_FILE="$2"
[[ "$TASK_ID" =~ ^[A-Za-z0-9._-]+$ ]] || emit_err "task id must be a safe slug"
[ -f "$PROMPT_FILE" ] || emit_err "prompt file not found: $PROMPT_FILE"
PROMPT_BYTES=$(wc -c <"$PROMPT_FILE" | tr -d '[:space:]')
[ "$PROMPT_BYTES" -le 9000 ] || emit_err "prompt is $PROMPT_BYTES bytes; context limit is 9000 bytes"
PROMPT="$(cat "$PROMPT_FILE")"

# --- Lint ---
printf '%s' "$PROMPT" | rg -q '(^|[[:space:]])/[^[:space:]]+' \
  || emit_err "prompt has no absolute path — name files by full path, not 'the file'"
printf '%s' "$PROMPT" | rg -q '^SUCCESS:' \
  || emit_err "prompt has no SUCCESS: line — write the success criterion"
printf '%s' "$PROMPT" | rg -q '^EXPECT-VERIFY:' \
  || emit_err "prompt has no EXPECT-VERIFY: block — declare the claims the child must satisfy"

CLAIMS=$(printf '%s\n' "$PROMPT" | awk '/^EXPECT-VERIFY:/{f=1;next} f{ if($0 ~ /^[[:space:]]/) print; else f=0 }')
[ -n "$(printf '%s' "$CLAIMS" | tr -d '[:space:]')" ] || emit_err "EXPECT-VERIFY block parsed to zero claims"
LINT=$(printf '%s\n' "$CLAIMS" | "$VERIFY_BIN" verify --lint 2>/dev/null) \
  || emit_err "invalid verification claim(s): $(printf '%s' "$LINT" | jq -r '[.claims[]? | select(.ok | not) | "\(.claim) (\(.detail))"] | join("; ")' 2>/dev/null)"

if [ "$LINT_ONLY" = "1" ]; then printf '{"status":"ok","lint":"passed"}\n'; exit 0; fi

# --- Budget state: workspace/subagent (logs/ is read-only under sandbox) ---
# Tests may set LAZAR_SPAWN_TEST=1 + LAZAR_SUBAGENT_STATE=<tmpdir>.
if [ "${LAZAR_SPAWN_TEST:-0}" = "1" ] && [ -n "${LAZAR_SUBAGENT_STATE:-}" ]; then
  STATE="$LAZAR_SUBAGENT_STATE"
else
  STATE="$LAZAR_HOME/user/workspace/subagent"
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

# POSIX flock via Python works on macOS too, and is released before execution.
python3 - "$LOCK" "$SESS" "$BUDGET" <<'PYBUDGET'
import fcntl, pathlib, sys
lock, state, budget = sys.argv[1:]
with open(lock, 'a') as stream:
    fcntl.flock(stream, fcntl.LOCK_EX)
    path = pathlib.Path(state)
    count = int(path.read_text()) if path.exists() else 0
    if count >= int(budget):
        raise SystemExit(1)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(str(count + 1))
    temporary.replace(path)
PYBUDGET
[ "$?" -eq 0 ] || emit_err "session spawn budget ($BUDGET) exhausted — stop and reassess"

# --- Per-task/prompt ceiling ---
KEY=$(printf '%s' "$TASK_ID"$'\x00'"$PROMPT" | shasum -a 256 | cut -d' ' -f1)
CFILE="$STATE/ceil-$KEY"
FAILS=$(cat "$CFILE" 2>/dev/null || echo 0)
[ "$FAILS" -ge "$CEILING" ] && emit_err "task '$TASK_ID' hit failure ceiling ($CEILING) — fix the task, don't respawn"

# --- Test hook: simulate a failed attempt without a real child ---
if [ "${SPAWN_TEST_NOOP:-}" = "fail" ]; then python3 "$HELPER_DIR/spawn-supervisor.py" --failure "$CFILE"; emit_err "noop-fail (test)"; fi

# --- Bounded child spawn ---
LAZAR_BIN="${LAZAR_BIN:-$LAZAR_HOME/bin/lazar}"
HELPER_DIR=$(cd "$(dirname "$0")" && pwd)
TIMEOUT_RUNNER="$HELPER_DIR/run-with-timeout.py"
SUPERVISOR="$HELPER_DIR/spawn-supervisor.py"
[ -f "$TIMEOUT_RUNNER" ] || emit_err "missing bounded runner: $TIMEOUT_RUNNER"
KILL_AFTER="${LAZAR_SPAWN_TIMEOUT:-300}"
[[ "$KILL_AFTER" =~ ^[0-9]+$ && "$KILL_AFTER" -gt 0 ]] || emit_err "spawn timeout must be a positive integer"
CHILD_DEPTH=$(( ${LAZAR_DEPTH:-0} + 1 ))
OUTFILE="$STATE/out-$KEY.log"

JOB_DIR="$STATE/jobs"
mkdir -p "$JOB_DIR"
JOB_ID="${TASK_ID}-$(date +%s)-$$"
JOB_FILE="$JOB_DIR/$JOB_ID.json"

run_child_fg(){
  python3 "$TIMEOUT_RUNNER" --timeout "$KILL_AFTER" -- env LAZAR_DEPTH="$CHILD_DEPTH" LAZAR_SPAWNED=1 "$LAZAR_BIN" -p "$PROMPT" --output-format stream-json
}

# Write the final job record. $1 = matched (true|false), $2 = child rc,
# $3 = failed-claims JSON array (only read when matched=false).
finish_job(){
  local matched="$1" rc="$2" failed_json="$3" started
  # Keep the start time from the running record so `lazar jobs` can report
  # how long a finished job took.
  started=$(jq '.started_ms // null' "$JOB_FILE" 2>/dev/null || echo null)
  [ -n "$started" ] || started=null
  if [ "$matched" = "true" ] && [ "$rc" -eq 0 ]; then
    jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$rc" --arg out "$OUTFILE" --argjson started "$started" \
      '{status:"ok",job_id:$id,task:$task,matched:true,child_rc:$rc,output:$out,started_ms:$started}' >"$JOB_FILE"
  else
    jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson rc "$rc" --arg out "$OUTFILE" --argjson failed "$failed_json" --argjson started "$started" \
      '{status:"fail",job_id:$id,task:$task,matched:false,child_rc:$rc,failed_claims:$failed,output:$out,started_ms:$started}' >"$JOB_FILE"
  fi
}

# Claims are the indented lines that follow EXPECT-VERIFY: in the prompt.
parse_claims(){
  printf '%s' "$PROMPT" | awk '/^EXPECT-VERIFY:/{f=1;next} f{ if($0 ~ /^[[:space:]]/) print; else f=0 }'
}

# Persist the claims beside the job record so --status/--await can finalize a
# wedged job without the original prompt.
CLAIMS_FILE="$JOB_FILE.claims"
parse_claims >"$CLAIMS_FILE"

if [ "$DETACH" = "1" ]; then
  [ -f "$SUPERVISOR" ] || emit_err "missing job supervisor: $SUPERVISOR"
  jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --arg output "$OUTFILE" \
    --arg failure_file "$CFILE" --arg claims "$CLAIMS_FILE" --arg runner "$TIMEOUT_RUNNER" --arg binary "$LAZAR_BIN" --arg verifier "$VERIFY_BIN" \
    --arg prompt "$PROMPT" --arg depth "$CHILD_DEPTH" --argjson timeout "$KILL_AFTER" \
    '{id:$id,task:$task,output:$output,claims:$claims,runner:$runner,verifier:$verifier,timeout:$timeout,failure_file:$failure_file,
      argv:["env",("LAZAR_DEPTH="+$depth),"LAZAR_SPAWNED=1",$binary,"-p",$prompt,"--output-format","stream-json"]}' >"${JOB_FILE%.json}.spec.json"
  jq -n --arg id "$JOB_ID" '{status:"starting",job_id:$id}' >"$JOB_FILE"
  BG_PID=$(python3 "$SUPERVISOR" --detach "$JOB_FILE") || emit_err "cannot start job supervisor"
  jq -cn --arg id "$JOB_ID" --arg task "$TASK_ID" --argjson pid "$BG_PID" --arg output "$OUTFILE" \
    '{status:"running",job_id:$id,task:$task,pid:$pid,output:$output}'
  exit 0
fi

# A running record lets `lazar jobs` (and the TUI footer) show live children.
jq -n --arg id "$JOB_ID" --arg task "$TASK_ID" --arg output "$OUTFILE" --argjson pid "$$" \
  --argjson started "$(python3 -c 'import time; print(int(time.time()*1000))')" \
  '{status:"running",job_id:$id,task:$task,pid:$pid,output:$output,started_ms:$started}' >"$JOB_FILE"
run_child_fg >"$OUTFILE" 2>/dev/null; RC=$?
if [ "$RC" -eq 124 ]; then python3 "$HELPER_DIR/spawn-supervisor.py" --failure "$CFILE"; emit_err "child timed out after ${KILL_AFTER}s (wedged) — killed"; fi

# --- Independently re-check parent's claims (wrapper is the judge) ---
check_claims <"$CLAIMS_FILE"

if [ "$CLAIMS_COUNT" -eq 0 ]; then
  python3 "$HELPER_DIR/spawn-supervisor.py" --failure "$CFILE"
  emit_err "EXPECT-VERIFY block parsed to zero claims — malformed prompt, refusing to declare success"
fi

FAILED_JSON=$(failed_claims_json)

if [ "$CLAIMS_MATCHED" = "true" ] && [ "$RC" -eq 0 ]; then
  finish_job true "$RC" '[]' 2>/dev/null || true
  printf '{"status":"ok","job_id":%s,"task":%s,"matched":true,"child_rc":%s,"output":%s}\n' \
    "$(printf '%s' "$JOB_ID"|jq -R .)" "$(printf '%s' "$TASK_ID"|jq -R .)" "$RC" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 0
else
  python3 "$HELPER_DIR/spawn-supervisor.py" --failure "$CFILE"
  finish_job false "$RC" "$FAILED_JSON" 2>/dev/null || true
  printf '{"status":"fail","job_id":%s,"task":%s,"matched":false,"child_rc":%s,"failed_claims":%s,"output":%s}\n' \
    "$(printf '%s' "$JOB_ID"|jq -R .)" "$(printf '%s' "$TASK_ID"|jq -R .)" "$RC" "$FAILED_JSON" "$(printf '%s' "$OUTFILE"|jq -R .)"
  exit 2
fi
