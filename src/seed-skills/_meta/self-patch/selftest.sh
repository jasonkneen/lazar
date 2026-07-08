#!/usr/bin/env bash
# selftest for self-patch — exercises the mechanical helpers in a temp LAZAR_HOME.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0; FAIL=0
ok(){ echo "[PASS] $1"; PASS=$((PASS+1)); }
no(){ echo "[FAIL] $1"; FAIL=$((FAIL+1)); }
T="$(mktemp -d)"; trap 'chmod -R u+w "$T" 2>/dev/null; rm -rf "$T"' EXIT
export LAZAR_HOME="$T"
mkdir -p "$T/src/src" "$T/workspace" "$T/memory/distilled"
echo 'fn main(){}' > "$T/src/src/main.rs"
echo 'locked source' > "$T/src/Cargo.toml"
mkdir -p "$T/src/target"; echo junk > "$T/src/target/junk"     # must NOT be copied
chmod -R a-w "$T/src" 2>/dev/null || true                       # simulate locked src

# --- stage-proposal: creates a writable src copy, drops target/, refuses clobber ---
STAGED=$(bash "$HERE/stage-proposal.sh" fix-max-tokens 2>&1)
[ -d "$STAGED" ] && ok "proposal staged" || no "stage failed: $STAGED"
[ -f "$STAGED/src/main.rs" ] && ok "staged src copied" || no "staged src missing main.rs"
[ ! -e "$STAGED/target" ] && ok "target/ excluded from stage" || no "target/ leaked into stage"
# staged copy must be writable even though live src was locked
( echo 'edit' >> "$STAGED/src/main.rs" 2>/dev/null ) && ok "staged copy is writable" || no "staged copy not writable"
CLOBBER=$(bash "$HERE/stage-proposal.sh" fix-max-tokens 2>&1); echo "$CLOBBER" | rg -q 'already staged' && ok "refuses to clobber existing proposal" || no "should refuse clobber"
bash "$HERE/stage-proposal.sh" '../escape' >/dev/null 2>&1 && no "should reject unsafe name" || ok "rejects unsafe proposal name"

# --- ledger: records a dated entry, newest first ---
bash "$HERE/ledger.sh" behavioral "stall spiral" "M3 apology loop" "PINNED anti-stall rule" "contains PINNED anti-stall" >/dev/null
bash "$HERE/ledger.sh" kernel-proposal "max_tokens truncation" "8192 cap" "staged proposal" "build-ok" >/dev/null
L="$T/memory/distilled/self-patch.md"
rg -q 'max_tokens truncation' "$L" 2>/dev/null && ok "ledger recorded entry" || no "ledger entry missing"
[ "$(head -6 "$L" | rg -c 'max_tokens')" -ge 1 ] && ok "newest entry is on top" || no "newest not prepended"

# --- SKILL.md documents the classify procedure + kernel boundary ---
S="$HERE/SKILL.md"
rg -q 'behavioral' "$S" && rg -q 'config' "$S" && rg -q 'kernel' "$S" && ok "SKILL documents the 3 layers" || no "SKILL missing classify layers"
rg -qi 'never .*apply|proposal.only|kernel-apply' "$S" && ok "SKILL documents proposal-only kernel boundary" || no "SKILL missing kernel boundary"

echo "---- $PASS passed, $FAIL failed ----"
[ "$FAIL" -eq 0 ]
