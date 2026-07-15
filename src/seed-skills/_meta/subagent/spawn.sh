#!/usr/bin/env bash
# Thin re-exec only — full spawn logic lives in the locked bin/lazar-spawn
# (installed by setup.sh). skills/ is agent-writable; never put ceilings,
# claim checks, or budget state here.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

SPAWN="$LAZAR_HOME/bin/lazar-spawn"
if [ -x "$SPAWN" ]; then
  exec "$SPAWN" "$@"
fi

printf '{"status":"error","reason":"trusted lazar-spawn missing — run setup.sh"}\n' >&2
exit 1
