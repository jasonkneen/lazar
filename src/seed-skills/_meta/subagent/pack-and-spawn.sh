#!/usr/bin/env bash
# pack-and-spawn — write a self-contained BRIEF + thin spawn prompt, then
# dispatch via locked bin/lazar-spawn. Prefer this over hand-rolling prompts.
#
# Usage:
#   pack-and-spawn.sh <task-id> --objective "..." --out /abs/result.md \
#     [--path /abs/input]... [--skill name]... [--note "prior decision"]... \
#     [--claim "exists=/abs"]... [--success "one line"] [--no-run]
#
# Layout:
#   $LAZAR_HOME/workspace/tasks/<task-id>/
#     BRIEF.md    full context pack for the child
#     prompt.txt  thin lazar-spawn prompt
#     RESULT.md   child should write this (unless --out overrides)
set -euo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

usage() {
  cat <<'U' >&2
usage: pack-and-spawn.sh <task-id> --objective TEXT --out /abs/path [options]
  --objective TEXT   imperative goal (required)
  --out /abs/path    primary deliverable path (required)
  --path /abs        input path the child may read (repeatable)
  --skill name       skill name to mention (repeatable)
  --note TEXT        prior decision / constraint (repeatable)
  --claim CLAIM      extra EXPECT-VERIFY claim (repeatable)
  --success TEXT     SUCCESS line (default derived from --out)
  --timeout SECS     LAZAR_SPAWN_TIMEOUT override for this run
  --no-run           only write BRIEF+prompt; do not spawn
  --lint-only        write pack then lazar-spawn --lint-only
U
  exit 2
}

[ $# -ge 1 ] || usage
TASK_ID="$1"; shift
case "$TASK_ID" in
  ""|.*|*/*|*..*|-*) echo "error: unsafe task-id: $TASK_ID" >&2; exit 2 ;;
esac

OBJECTIVE=""
OUT_PATH=""
SUCCESS=""
NO_RUN=0
LINT_ONLY=0
TIMEOUT=""
PATHS=()
SKILLS=()
NOTES=()
CLAIMS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --objective) OBJECTIVE="${2:-}"; shift 2 ;;
    --out) OUT_PATH="${2:-}"; shift 2 ;;
    --path) PATHS+=("${2:-}"); shift 2 ;;
    --skill) SKILLS+=("${2:-}"); shift 2 ;;
    --note) NOTES+=("${2:-}"); shift 2 ;;
    --claim) CLAIMS+=("${2:-}"); shift 2 ;;
    --success) SUCCESS="${2:-}"; shift 2 ;;
    --timeout) TIMEOUT="${2:-}"; shift 2 ;;
    --no-run) NO_RUN=1; shift ;;
    --lint-only) LINT_ONLY=1; shift ;;
    -h|--help) usage ;;
    *) echo "error: unknown arg: $1" >&2; usage ;;
  esac
done

[ -n "$OBJECTIVE" ] || { echo "error: --objective required" >&2; exit 2; }
[ -n "$OUT_PATH" ] || { echo "error: --out required" >&2; exit 2; }
case "$OUT_PATH" in
  /*) ;;
  *) echo "error: --out must be absolute path" >&2; exit 2 ;;
esac
for p in "${PATHS[@]+"${PATHS[@]}"}"; do
  case "$p" in
    /*) ;;
    *) echo "error: --path must be absolute: $p" >&2; exit 2 ;;
  esac
done

TASK_DIR="$LAZAR_HOME/workspace/tasks/$TASK_ID"
mkdir -p "$TASK_DIR"
BRIEF="$TASK_DIR/BRIEF.md"
PROMPT_FILE="$TASK_DIR/prompt.txt"
RESULT_DEFAULT="$TASK_DIR/RESULT.md"

if [ -z "$SUCCESS" ]; then
  SUCCESS="deliverable exists at $OUT_PATH and RESULT.md reports DONE"
fi

# default claims
DEFAULT_CLAIMS=(
  "exists=$OUT_PATH"
  "exists=$RESULT_DEFAULT"
  "contains=$RESULT_DEFAULT:DONE"
)

{
  printf '# Task brief: %s\n\n' "$TASK_ID"
  printf '## Objective\n%s\n\n' "$OBJECTIVE"
  printf '## Success\nSUCCESS: %s\n' "$SUCCESS"
  printf 'EXPECT-VERIFY:\n'
  for c in "${DEFAULT_CLAIMS[@]}"; do printf '  %s\n' "$c"; done
  for c in "${CLAIMS[@]+"${CLAIMS[@]}"}"; do printf '  %s\n' "$c"; done
  printf '\n## Deliverables\n'
  printf -- '- Primary: %s\n' "$OUT_PATH"
  printf -- '- Status: %s (first line DONE or BLOCKED, then short summary)\n\n' "$RESULT_DEFAULT"
  printf '## Inputs (absolute paths)\n'
  if [ ${#PATHS[@]} -eq 0 ]; then
    printf -- '- (none beyond paths in the objective)\n'
  else
    for p in "${PATHS[@]}"; do printf -- '- %s\n' "$p"; done
  fi
  printf '\n## Skills to consider\n'
  if [ ${#SKILLS[@]} -eq 0 ]; then
    printf -- '- (read $LAZAR_HOME/skills/INDEX.md only if needed)\n'
  else
    for s in "${SKILLS[@]}"; do printf -- '- %s\n' "$s"; done
  fi
  printf '\n## Prior decisions / constraints\n'
  if [ ${#NOTES[@]} -eq 0 ]; then
    printf -- '- Report honest failure; do not fabricate.\n'
  else
    for n in "${NOTES[@]}"; do printf -- '- %s\n' "$n"; done
    printf -- '- Report honest failure; do not fabricate.\n'
  fi
  printf '\n## Out of scope\n'
  printf -- '- Do not call raw `lazar -p` (use pack-and-spawn / lazar-spawn only if further split is required).\n'
  printf -- '- Do not modify bin/ or src/.\n'
  printf '\n## Return protocol\n'
  printf '1. Do the work with bash tools.\n'
  printf '2. Write the primary deliverable to the absolute path above.\n'
  printf '3. Write %s starting with DONE or BLOCKED.\n' "$RESULT_DEFAULT"
  printf '4. End with a [VERIFY] block proving the EXPECT-VERIFY claims.\n'
} >"$BRIEF"

{
  printf 'Read %s and execute it fully.\n' "$BRIEF"
  printf 'Write status to %s (first line DONE or BLOCKED).\n' "$RESULT_DEFAULT"
  printf 'Primary deliverable path: %s\n' "$OUT_PATH"
  printf 'SUCCESS: %s\n' "$SUCCESS"
  printf 'EXPECT-VERIFY:\n'
  for c in "${DEFAULT_CLAIMS[@]}"; do printf '  %s\n' "$c"; done
  for c in "${CLAIMS[@]+"${CLAIMS[@]}"}"; do printf '  %s\n' "$c"; done
  printf 'Report honest failure; do not fabricate.\n'
} >"$PROMPT_FILE"

echo "[pack] wrote $BRIEF"
echo "[pack] wrote $PROMPT_FILE"

SPAWN=""
for candidate in "$LAZAR_HOME/bin/lazar-spawn" "$LAZAR_HOME/scripts/lazar-spawn.sh"; do
  if [ -x "$candidate" ]; then SPAWN="$candidate"; break; fi
done
[ -n "$SPAWN" ] || { echo "error: lazar-spawn not found" >&2; exit 1; }

if [ "$NO_RUN" = "1" ]; then
  echo "[pack] --no-run set; not spawning"
  printf '{"status":"packed","task_id":"%s","brief":"%s","prompt":"%s"}\n' \
    "$TASK_ID" "$BRIEF" "$PROMPT_FILE"
  exit 0
fi

export_cmd=()
if [ -n "$TIMEOUT" ]; then
  export LAZAR_SPAWN_TIMEOUT="$TIMEOUT"
fi

if [ "$LINT_ONLY" = "1" ]; then
  exec "$SPAWN" --lint-only "$TASK_ID" "$PROMPT_FILE"
fi

exec "$SPAWN" "$TASK_ID" "$PROMPT_FILE"
