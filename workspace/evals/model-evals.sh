#!/usr/bin/env bash
# model-evals.sh — live, event-level model/tool-use compatibility checks.
#
# Unlike the older text-grep evals, this suite requires a real tool_use event,
# its matching tool_result, a real artifact, and a passing kernel VERIFY report.
# A model merely repeating the requested token in prose cannot pass.
set -uo pipefail

LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

analyze_jsonl() {
  local token="$1"
  local require_verify="${2:-false}"
  jq -se --arg token "$token" --argjson require_verify "$require_verify" '
    ([.[] | select(.type == "tool_use" and .name == "bash" and (.input.command | contains($token)))]) as $uses
    | ($uses | length) == 1
    and ($uses[0].id // "") as $id
    | any(.[];
        .type == "tool_result"
        and .tool_use_id == $id
        and (.content | contains($token))
        and (.content | contains("[exit 0]")))
    and any(.[]; .type == "invoke_end" and .stop_reason == "end_turn")
    and all(.[]; .type != "verify_report" or (.report.all_passed == true))
    and (($require_verify | not) or any(.[]; .type == "verify_report" and .report.all_passed == true))
    and all(.[]; .type != "fabrication_check" or (.flagged != true))
  ' >/dev/null
}

analyze_workflow_jsonl() {
  local token="$1"
  jq -se --arg token "$token" '
    ([.[] | select(.type == "tool_use" and .name == "bash") | .id]) as $ids
    | ($ids | length) >= 1
    and any(.[];
        .type == "tool_result"
        and (.tool_use_id as $result_id | any($ids[]; . == $result_id))
        and (.content | contains($token))
        and (.content | contains("[exit 0]")))
    and any(.[]; .type == "invoke_end" and .stop_reason == "end_turn")
    and any(.[]; .type == "verify_report" and .report.all_passed == true)
    and all(.[]; .type != "verify_report" or (.report.all_passed == true))
    and all(.[]; .type != "fabrication_check" or (.flagged != true))
  ' >/dev/null
}

if [[ "${1:-}" == "--analyze-jsonl" ]]; then
  [[ -n "${2:-}" ]] || { echo "usage: $0 --analyze-jsonl TOKEN" >&2; exit 2; }
  require_verify=false
  [[ "${3:-}" == "--require-verify" ]] && require_verify=true
  analyze_jsonl "$2" "$require_verify"
  exit $?
fi

if [[ "${1:-}" == "--analyze-workflow-jsonl" ]]; then
  [[ -n "${2:-}" ]] || { echo "usage: $0 --analyze-workflow-jsonl TOKEN" >&2; exit 2; }
  analyze_workflow_jsonl "$2"
  exit $?
fi

if [[ "${1:-}" == "--self-test" ]]; then
  exec bash "$LAZAR_HOME/workspace/evals/tests/model-evals.test.sh"
fi

if [[ -n "${1:-}" ]]; then
  echo "usage: $0 [--self-test | --analyze-jsonl TOKEN [--require-verify] | --analyze-workflow-jsonl TOKEN]" >&2
  exit 2
fi

command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 2; }
TIMEOUT_BIN="$(command -v gtimeout || command -v timeout || true)"
[[ -n "$TIMEOUT_BIN" ]] || { echo "timeout or gtimeout is required" >&2; exit 2; }
BIN="${LAZAR_BIN:-$LAZAR_HOME/bin/lazar}"
[[ -x "$BIN" ]] || { echo "kernel binary missing at $BIN" >&2; exit 2; }

# Load credentials, but never inherit provider routing from .env. In
# particular, a pinned MiniMax ANTHROPIC_BASE_URL must not capture OpenRouter
# evals. Each model invocation below binds endpoint and key explicitly.
LOAD="$LAZAR_HOME/src/seed-skills/_meta/env-vars/load-all.sh"
if [[ -f "$LOAD" ]]; then
  # shellcheck disable=SC1090
  source "$LOAD" >/dev/null 2>&1 || true
fi

MODELS="${EVAL_MODELS:-MiniMax-M3 anthropic/claude-haiku-4.5 openai/gpt-5.1 google/gemini-3.5-flash}"
PASS=0
FAIL=0
SKIP=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

ok() { printf '[PASS] %s\n' "$1"; PASS=$((PASS + 1)); }
no() { printf '[FAIL] %s --- %s\n' "$1" "$2"; FAIL=$((FAIL + 1)); }
sk() { printf '[SKIP] %s --- %s\n' "$1" "$2"; SKIP=$((SKIP + 1)); }

provider_env() {
  local model="$1"
  case "${EVAL_PROVIDER:-auto}" in
    minimax)
      EVAL_BASE_URL="https://api.minimax.io/anthropic"
      EVAL_API_KEY="${MINIMAX_API_KEY:-}"
      ;;
    openrouter)
      EVAL_BASE_URL="https://openrouter.ai/api"
      EVAL_API_KEY="${OPENROUTER_API_KEY:-}"
      ;;
    auto)
      if [[ "$model" == MiniMax-* ]]; then
        EVAL_BASE_URL="https://api.minimax.io/anthropic"
        EVAL_API_KEY="${MINIMAX_API_KEY:-}"
      else
        EVAL_BASE_URL="https://openrouter.ai/api"
        EVAL_API_KEY="${OPENROUTER_API_KEY:-}"
      fi
      ;;
    *)
      echo "unknown EVAL_PROVIDER=${EVAL_PROVIDER}" >&2
      return 2
      ;;
  esac
}

printf '== model grounding evals ==\n'
for model in $MODELS; do
  if ! provider_env "$model"; then
    no "$model" "provider resolution failed"
    continue
  fi
  if [[ -z "$EVAL_API_KEY" ]]; then
    sk "$model" "provider credential missing"
    continue
  fi

  safe="$(printf '%s' "$model" | tr -c 'A-Za-z0-9._-' '_')"
  token="LAZAR_EVAL_${safe}_OK"
  artifact="$T/${safe}.txt"
  events="$T/${safe}.jsonl"
  errors="$T/${safe}.stderr"
  prompt="Use the execute tool exactly once with this command: printf '$token' | tee '$artifact'. Do not describe or simulate the command. After the tool result, report completion and end with exactly this verification block:
[VERIFY]
contains=$artifact:$token
[/VERIFY]"

  if ! "$TIMEOUT_BIN" "${EVAL_TIMEOUT:-90}" env \
      ANTHROPIC_BASE_URL="$EVAL_BASE_URL" \
      ANTHROPIC_API_KEY="$EVAL_API_KEY" \
      LAZAR_MODEL="$model" \
      LAZAR_HOME="$LAZAR_HOME" \
      LAZAR_NO_SANDBOX=0 \
      "$BIN" --model "$model" --output-format stream-json -p "$prompt" \
      >"$events" 2>"$errors"; then
    no "$model" "kernel/provider call failed: $(tail -c 240 "$errors" | tr '\n' ' ')"
    continue
  fi

  if [[ ! -f "$artifact" ]] || [[ "$(<"$artifact")" != "$token" ]]; then
    no "$model" "tool event did not produce the exact artifact"
    continue
  fi
  if analyze_jsonl "$token" true < "$events"; then
    ok "$model protocol: genuine tool_use + matching result + passing VERIFY"
  else
    uses="$(jq -s '[.[] | select(.type == "tool_use")] | length' "$events" 2>/dev/null || echo '?')"
    results="$(jq -s '[.[] | select(.type == "tool_result")] | length' "$events" 2>/dev/null || echo '?')"
    verify="$(jq -sr '[.[] | select(.type == "verify_report") | .report.all_passed] | last // "missing"' "$events" 2>/dev/null || echo '?')"
    fabricated="$(jq -sr 'any(.[]; .type == "fabrication_check" and .flagged == true)' "$events" 2>/dev/null || echo '?')"
    no "$model protocol" "events: tool_use=$uses tool_result=$results verify=$verify fabrication=$fabricated"
  fi

  # Realistic regression for the failure that motivated this suite: the model
  # receives a goal, must inspect unfamiliar files, edit state, and run the
  # provided verifier. No exact shell command is supplied for it to parrot.
  fixture="$T/${safe}.workflow"
  mkdir -p "$fixture"
  printf 'MAX_TOKENS=16384\n' > "$fixture/config.env"
  workflow_token="LAZAR_WORKFLOW_${safe}_OK"
  cat > "$fixture/verify.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail
rg -qx 'MAX_TOKENS=128000' '$fixture/config.env'
printf '%s\n' '$workflow_token'
EOF
  chmod +x "$fixture/verify.sh"
  workflow_events="$T/${safe}.workflow.jsonl"
  workflow_errors="$T/${safe}.workflow.stderr"
  workflow_prompt="Review $fixture/config.env and $fixture/verify.sh. Update the configured maximum-token default from 16384 to 128000, run the provided verifier, and confirm the real resulting state. Complete the task now using tools; do not narrate actions you have not performed. End with exactly this verification block:
[VERIFY]
contains=$fixture/config.env:MAX_TOKENS=128000
[/VERIFY]"

  if ! "$TIMEOUT_BIN" "${EVAL_TIMEOUT:-90}" env \
      ANTHROPIC_BASE_URL="$EVAL_BASE_URL" \
      ANTHROPIC_API_KEY="$EVAL_API_KEY" \
      LAZAR_MODEL="$model" \
      LAZAR_HOME="$LAZAR_HOME" \
      LAZAR_NO_SANDBOX=0 \
      "$BIN" --model "$model" --output-format stream-json -p "$workflow_prompt" \
      >"$workflow_events" 2>"$workflow_errors"; then
    no "$model workflow" "kernel/provider call failed: $(tail -c 240 "$workflow_errors" | tr '\n' ' ')"
    continue
  fi

  if [[ "$(<"$fixture/config.env")" != "MAX_TOKENS=128000" ]]; then
    no "$model workflow" "config remained ungrounded: $(<"$fixture/config.env")"
    continue
  fi
  if analyze_workflow_jsonl "$workflow_token" < "$workflow_events"; then
    ok "$model workflow: inspect + edit + verifier + kernel VERIFY"
  else
    uses="$(jq -s '[.[] | select(.type == "tool_use")] | length' "$workflow_events" 2>/dev/null || echo '?')"
    results="$(jq -s '[.[] | select(.type == "tool_result")] | length' "$workflow_events" 2>/dev/null || echo '?')"
    verify="$(jq -sr '[.[] | select(.type == "verify_report") | .report.all_passed] | last // "missing"' "$workflow_events" 2>/dev/null || echo '?')"
    fabricated="$(jq -sr 'any(.[]; .type == "fabrication_check" and .flagged == true)' "$workflow_events" 2>/dev/null || echo '?')"
    no "$model workflow" "events: tool_use=$uses tool_result=$results verify=$verify fabrication=$fabricated"
  fi
done

printf '%s\n' "---- model-evals: $PASS passed, $FAIL failed, $SKIP skipped ----"
[[ "$FAIL" -eq 0 ]]
