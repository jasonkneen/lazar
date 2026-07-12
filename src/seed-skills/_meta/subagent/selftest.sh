#!/usr/bin/env bash
# subagent selftest — trusted bin path, logs state, veto hook.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

# Resolve trusted spawn (never rely on agent-writable skills logic).
if [ -x "$LAZAR_HOME/bin/lazar-spawn" ]; then
  SPAWN="$LAZAR_HOME/bin/lazar-spawn"
elif [ -x "$LAZAR_HOME/scripts/lazar-spawn.sh" ]; then
  SPAWN="$LAZAR_HOME/scripts/lazar-spawn.sh"
elif [ -x "$HERE/../../../../scripts/lazar-spawn.sh" ]; then
  SPAWN="$(cd "$HERE/../../../../scripts" && pwd)/lazar-spawn.sh"
else
  echo "[FAIL] no trusted lazar-spawn found"; exit 1
fi

PASS=0; FAIL=0
ok(){ echo "[PASS] $1"; PASS=$((PASS+1)); }
no(){ echo "[FAIL] $1"; FAIL=$((FAIL+1)); }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# Isolate runtime state so tests never touch production counters.
export LAZAR_SPAWN_TEST=1
export LAZAR_SUBAGENT_STATE="$TMP/state"

# --- Task 1: lint ---
printf 'add a line to the file\n' > "$TMP/vague.txt"
if "$SPAWN" t1 "$TMP/vague.txt" >/dev/null 2>&1; then no "vague prompt should be refused"; else ok "vague prompt refused"; fi

printf 'Write DONE to /tmp/x.\nSUCCESS: file has DONE\nEXPECT-VERIFY:\n  contains=/tmp/x:DONE\n' > "$TMP/good.txt"
OUT=$("$SPAWN" --lint-only t1 "$TMP/good.txt" 2>&1) && ok "well-formed prompt passes lint" || no "well-formed prompt should pass lint: $OUT"

# --- Task 2: ceiling + budget ---
export LAZAR_SPAWN_CEILING=2 LAZAR_SPAWN_BUDGET=3
printf 'touch /tmp/noop\nSUCCESS: noop\nEXPECT-VERIFY:\n  exists=/tmp/noop\n' > "$TMP/noop.txt"
SPAWN_TEST_NOOP=fail "$SPAWN" tceil "$TMP/noop.txt" >/dev/null 2>&1 || true
SPAWN_TEST_NOOP=fail "$SPAWN" tceil "$TMP/noop.txt" >/dev/null 2>&1 || true
CEIL_OUT=$(SPAWN_TEST_NOOP=fail "$SPAWN" tceil "$TMP/noop.txt" 2>&1 || true)
echo "$CEIL_OUT" | rg -q 'ceiling' && ok "per-task ceiling stops respawn" || no "ceiling should stop 3rd attempt: $CEIL_OUT"

# --- Task 3: spawn + claim re-check via stub lazar ---
mkdir -p "$TMP/bin"
CHILD_OUT="$TMP/spawn_selftest_out"
cat > "$TMP/bin/lazar" <<STUB
#!/usr/bin/env bash
case "\$*" in
  *GOOD*) echo DONE > "$CHILD_OUT" ;;
esac
echo '{"type":"verify_report","report":{"all_passed":true}}'
STUB
chmod +x "$TMP/bin/lazar"
export LAZAR_SPAWN_CEILING=2 LAZAR_SPAWN_BUDGET=20
rm -f "$CHILD_OUT"
printf 'GOOD write DONE to %s\nSUCCESS: file present\nEXPECT-VERIFY:\n  contains=%s:DONE\n' "$CHILD_OUT" "$CHILD_OUT" > "$TMP/win.txt"
OUT=$(LAZAR_BIN="$TMP/bin/lazar" "$SPAWN" twin "$TMP/win.txt" 2>&1)
echo "$OUT" | rg -q '"matched":true' && echo "$OUT" | rg -q '"child_rc"' && ok "matched claim -> ok + child_rc" || no "expected matched:true with child_rc, got: $OUT"

MISS_OUT="$TMP/spawn_selftest_miss"
rm -f "$MISS_OUT"
printf 'BAD do nothing for %s\nSUCCESS: file present\nEXPECT-VERIFY:\n  exists=%s\n' "$MISS_OUT" "$MISS_OUT" > "$TMP/miss.txt"
OUT=$(LAZAR_BIN="$TMP/bin/lazar" "$SPAWN" tmiss "$TMP/miss.txt" 2>&1)
echo "$OUT" | rg -q '"matched":false' && ok "unmet claim -> not matched" || no "expected matched:false, got: $OUT"

# --- Task 4: production state defaults to logs/ (not skills/) ---
if rg -q 'logs/subagent' "$SPAWN" 2>/dev/null \
  || rg -q 'logs/subagent' "$LAZAR_HOME/scripts/lazar-spawn.sh" 2>/dev/null; then
  ok "trusted spawn defaults state to logs/subagent"
else
  no "trusted spawn must default budget state under logs/subagent"
fi

# Thin skills re-exec must not contain claim/budget logic.
THIN="$HERE/spawn.sh"
if [ -f "$THIN" ]; then
  if rg -q 'EXPECT-VERIFY|LAZAR_SPAWN_CEILING|ceil-' "$THIN" 2>/dev/null; then
    no "skills spawn.sh must be thin re-exec only (no control-plane logic)"
  else
    ok "skills spawn.sh is thin re-exec only"
  fi
fi

# --- Task 5: veto hook ---
HOOK="$LAZAR_HOME/hooks/pre-tool.d/10-spawn-discipline.strict.sh"
[ -f "$HOOK" ] || HOOK="$LAZAR_HOME/src/seed-hooks/pre-tool.d/10-spawn-discipline.strict.sh"
if [ -f "$HOOK" ]; then
  printf '{"command":"LAZAR_DEPTH=1 lazar -p \\"do a thing\\""}' | bash "$HOOK" | rg -q '"action":"veto"' && ok "raw lazar -p vetoed" || no "raw spawn should be vetoed"
  printf '{"command":"bash $LAZAR_HOME/bin/lazar-spawn t1 /tmp/p.txt"}' | bash "$HOOK" | rg -q '"action":"continue"' && ok "bin/lazar-spawn allowed" || no "bin/lazar-spawn should be allowed"
  printf '{"command":"bash skills/_meta/subagent/spawn.sh t1 /tmp/p.txt"}' | bash "$HOOK" | rg -q '"action":"continue"' && ok "thin skills re-exec allowed" || no "thin re-exec should be allowed"
  printf '{"command":"echo skills/_meta/subagent/spawn.sh; lazar -p evil"}' | bash "$HOOK" | rg -q '"action":"veto"' && ok "decoy spawn path + lazar -p vetoed" || no "decoy bypass should be vetoed"
  printf '{"command":"echo bin/lazar-spawn; lazar -p evil"}' | bash "$HOOK" | rg -q '"action":"veto"' && ok "decoy bin name + lazar -p vetoed" || no "decoy bin bypass should be vetoed"
  printf '{"command":"bin/lazar --prompt hi"}' | bash "$HOOK" | rg -q '"action":"veto"' && ok "long-form --prompt vetoed" || no "--prompt should be vetoed"
  printf 'not-json-at-all' | bash "$HOOK" | rg -q '"action":"veto"' && ok "unparseable payload fail-closed" || no "bad JSON should veto"
else
  echo "[SKIP] veto hook not present yet"
fi

# --- Task 6: SKILL.md ---
if [ -f "$HERE/SKILL.md" ]; then
  rg -q 'bin/lazar-spawn' "$HERE/SKILL.md" && rg -q 'EXPECT-VERIFY' "$HERE/SKILL.md" && ok "SKILL.md documents locked bin + contract" || no "SKILL.md missing bin/lazar-spawn or contract"
else
  echo "[SKIP] SKILL.md not present yet"
fi

echo "---- $PASS passed, $FAIL failed ----"
[ "$FAIL" -eq 0 ]
