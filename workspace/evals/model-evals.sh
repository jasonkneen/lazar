#!/usr/bin/env bash
# model-evals.sh — event-level proof that models used real tools and that the
# kernel accepted their evidence. Narrated commands and failed grounding never
# count as success.
set -uo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

analyze_jsonl() {
  local token="$1"
  local require_verify="${2:-false}"
  jq -se --arg token "$token" --argjson require_verify "$require_verify" '
    . as $events
    | ($events | to_entries | map(select(.value.type == "invoke_end" and (.value.stop_reason == "end_turn" or .value.stop_reason == "stop_sequence"))) | last) as $accepted
    | ([ $events | to_entries[]
         | select(.key < $accepted.key
             and .value.type == "tool_use"
             and .value.name == "bash"
             and (.value.input.command | contains($token))) ]) as $uses
    | ([ $events | to_entries[]
         | select(.key < $accepted.key
             and .key > $uses[0].key
             and .value.type == "tool_result"
             and .value.tool_use_id == $uses[0].value.id
             and .value.exit_code == 0
             and (.value.content | contains($token))
             and (.value.content | test("\\[exit 0\\]\\s*$"))) ]) as $results
    | ($accepted != null)
    and ($uses | length) == 1
    and ($results | length) == 1
    and (($events | to_entries | map(select(.key < $accepted.key and .value.type == "verify_report")) | last) as $last_verify
      | if $last_verify == null then
          ($require_verify | not)
        else
          ($last_verify.value.report.all_passed == true and $results[0].key < $last_verify.key)
        end)
    and all($events | to_entries[] | select(.key < $accepted.key and .value.type == "verify_report" and .value.report.all_passed != true);
      .key as $failed | any($events | to_entries[]; .key > $failed and .key < $accepted.key and .value.type == "grounding_retry"))
    and all($events | to_entries[] | select(.key < $accepted.key and .value.type == "fabrication_check" and .value.flagged == true);
      .key as $flagged | any($events | to_entries[]; .key > $flagged and .key < $accepted.key and .value.type == "grounding_retry"))
    and all($events[]; .type != "grounding_failure")
    and all($events[]; .type != "error")
  ' >/dev/null
}

analyze_workflow_jsonl() {
  local token="$1"
  jq -se --arg token "$token" '
    . as $events
    | ($events | to_entries | map(select(.value.type == "invoke_end" and (.value.stop_reason == "end_turn" or .value.stop_reason == "stop_sequence"))) | last) as $accepted
    | ([ $events | to_entries[]
         | select(.key < $accepted.key and .value.type == "tool_use" and .value.name == "bash") ]) as $uses
    | ([ $events | to_entries[]
         | . as $result
         | select(.key < $accepted.key
             and .value.type == "tool_result"
             and .value.exit_code == 0
             and any($uses[]; .key < $result.key and .value.id == $result.value.tool_use_id)
             and (.value.content | contains($token))
             and (.value.content | test("\\[exit 0\\]\\s*$"))) ]) as $results
    | ($results | last) as $result
    | ($accepted != null)
    and ($uses | length) >= 1
    and ($result != null)
    and (($events | to_entries | map(select(.key < $accepted.key and .value.type == "verify_report")) | last) as $last_verify
      | $last_verify.value.report.all_passed == true and $result.key < $last_verify.key)
    and all($events | to_entries[] | select(.key < $accepted.key and .value.type == "verify_report" and .value.report.all_passed != true);
      .key as $failed | any($events | to_entries[]; .key > $failed and .key < $accepted.key and .value.type == "grounding_retry"))
    and all($events | to_entries[] | select(.key < $accepted.key and .value.type == "fabrication_check" and .value.flagged == true);
      .key as $flagged | any($events | to_entries[]; .key > $flagged and .key < $accepted.key and .value.type == "grounding_retry"))
    and all($events[]; .type != "grounding_failure")
    and all($events[]; .type != "error")
  ' >/dev/null
}

case "${1:-}" in
  --analyze-jsonl)
    [[ -n "${2:-}" ]] || { echo "usage: $0 --analyze-jsonl TOKEN [--require-verify]" >&2; exit 2; }
    require_verify=false
    [[ "${3:-}" == "--require-verify" ]] && require_verify=true
    analyze_jsonl "$2" "$require_verify"
    exit $?
    ;;
  --analyze-workflow-jsonl)
    [[ -n "${2:-}" ]] || { echo "usage: $0 --analyze-workflow-jsonl TOKEN" >&2; exit 2; }
    analyze_workflow_jsonl "$2"
    exit $?
    ;;
  --self-test)
    exec bash "$LAZAR_HOME/workspace/evals/tests/model-evals.test.sh"
    ;;
  "") ;;
  *) echo "usage: $0 [--self-test | --analyze-jsonl TOKEN [--require-verify] | --analyze-workflow-jsonl TOKEN]" >&2; exit 2 ;;
esac

command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 2; }
TIMEOUT_BIN="$(command -v gtimeout || command -v timeout || true)"
[[ -n "$TIMEOUT_BIN" ]] || { echo "timeout or gtimeout is required" >&2; exit 2; }
BIN="${LAZAR_BIN:-$LAZAR_HOME/bin/lazar}"
[[ -x "$BIN" ]] || { echo "kernel binary missing at $BIN" >&2; exit 2; }

# These names are sourced from the repo/user's current model configuration.
# Provider routing is explicit so a stale ANTHROPIC_BASE_URL cannot capture a
# run intended for a different backend.
if [[ -n "${GROUNDING_EVAL_MODELS:-}" ]]; then
  MODELS="$GROUNDING_EVAL_MODELS"
elif [[ -n "${MINIMAX_API_KEY:-${ANTHROPIC_API_KEY:-}}" ]]; then
  MODELS="${EVAL_MODELS:-MiniMax-M3}"
else
  MODELS="${EVAL_MODELS:-openai/gpt-oss-120b:free}"
fi
PASS=0; FAIL=0; SKIP=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
ok(){ printf '[PASS] %s\n' "$1"; PASS=$((PASS + 1)); }
no(){ printf '[FAIL] %s --- %s\n' "$1" "$2"; FAIL=$((FAIL + 1)); }
sk(){ printf '[SKIP] %s --- %s\n' "$1" "$2"; SKIP=$((SKIP + 1)); }

provider_env() {
  local model="$1"
  if [[ "${GROUNDING_EVAL_PROVIDER:-auto}" == "minimax" || "$model" == MiniMax-* ]]; then
    EVAL_BASE_URL="https://api.minimax.io/anthropic"
    EVAL_API_KEY="${MINIMAX_API_KEY:-${ANTHROPIC_API_KEY:-}}"
  else
    EVAL_BASE_URL="https://openrouter.ai/api"
    EVAL_API_KEY="${OPENROUTER_API_KEY:-}"
  fi
}

printf '== model grounding evals ==\n'
for model in $MODELS; do
  provider_env "$model"
  if [[ -z "$EVAL_API_KEY" ]]; then sk "$model" "provider credential missing"; continue; fi
  safe="$(printf '%s' "$model" | tr -c 'A-Za-z0-9._-' '_')"
  token="LAZAR_EVAL_${safe}_OK"
  artifact="$T/${safe}.txt"
  events="$T/${safe}.jsonl"
  errors="$T/${safe}.stderr"
  prompt="Use bash exactly once with this command: printf '$token' | tee '$artifact'. Do not simulate it. Then report the real result and end with exactly:
[VERIFY]
contains=$artifact:$token
[/VERIFY]"

  if ! "$TIMEOUT_BIN" "${EVAL_TIMEOUT:-90}" env \
      ANTHROPIC_BASE_URL="$EVAL_BASE_URL" ANTHROPIC_API_KEY="$EVAL_API_KEY" \
      LAZAR_MODEL="$model" LAZAR_HOME="$LAZAR_HOME" LAZAR_GROUNDING_RETRIES=1 \
      "$BIN" --model "$model" --output-format stream-json -p "$prompt" \
      >"$events" 2>"$errors"; then
    no "$model" "kernel/provider call failed: $(tail -c 240 "$errors" | tr '\n' ' ')"
    continue
  fi
  if [[ ! -f "$artifact" ]] || [[ "$(<"$artifact")" != "$token" ]]; then
    no "$model" "real artifact missing or incorrect"
  elif analyze_jsonl "$token" true <"$events"; then
    ok "$model genuine tool + result + accepted VERIFY"
  else
    no "$model" "event stream did not prove grounded completion"
  fi
done

printf '%s\n' "---- model-evals: $PASS passed, $FAIL failed, $SKIP skipped ----"
[[ "$FAIL" -eq 0 ]]
