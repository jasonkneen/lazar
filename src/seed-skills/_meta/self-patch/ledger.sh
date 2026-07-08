#!/usr/bin/env bash
# ledger.sh — record a self-patch to the durable fix ledger.
# Every fix (behavioral/config/kernel) gets one dated entry so the lesson
# persists across sessions and future self-evolver/self-patch runs see it.
#
# Usage: ledger.sh <layer> <symptom> <cause> <fix> <verification>
#   layer: behavioral | config | kernel-proposal
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"
LEDGER="$LAZAR_HOME/memory/distilled/self-patch.md"
mkdir -p "$(dirname "$LEDGER")"

[ $# -eq 5 ] || { echo "usage: ledger.sh <layer> <symptom> <cause> <fix> <verification>" >&2; exit 2; }
LAYER="$1"; SYMPTOM="$2"; CAUSE="$3"; FIX="$4"; VERIFY="$5"
DATE=$(date '+%Y-%m-%d %H:%M')

[ -f "$LEDGER" ] || printf '# self-patch ledger — every fix I made and why\n\nSymptom → root-cause → layer → fix → how it was verified. Newest first.\n\n' > "$LEDGER"

ENTRY=$(printf '## %s  [%s]\n- symptom: %s\n- cause: %s\n- fix: %s\n- verified: %s\n\n' \
  "$DATE" "$LAYER" "$SYMPTOM" "$CAUSE" "$FIX" "$VERIFY")

# Prepend under the header (newest first).
{ head -n 4 "$LEDGER"; printf '%s' "$ENTRY"; tail -n +5 "$LEDGER"; } > "$LEDGER.tmp" && mv "$LEDGER.tmp" "$LEDGER"
echo "recorded to $LEDGER"
