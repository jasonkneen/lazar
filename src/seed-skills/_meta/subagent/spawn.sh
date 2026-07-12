#!/usr/bin/env bash
# Thin re-exec only — full spawn logic lives in the locked bin/lazar-spawn
# (installed from scripts/lazar-spawn.sh). skills/ is agent-writable; never
# put ceilings, claim checks, or budget state here.
set -uo pipefail
LAZAR_HOME="${LAZAR_HOME:-$HOME/lazar}"

# Prefer locked install; fall back to scripts copy under LAZAR_HOME (not under skills/).
for candidate in \
  "$LAZAR_HOME/bin/lazar-spawn" \
  "$LAZAR_HOME/scripts/lazar-spawn.sh"
do
  if [ -x "$candidate" ]; then
    exec "$candidate" "$@"
  fi
done

printf '{"status":"error","reason":"trusted lazar-spawn missing — run setup.sh or restore scripts/lazar-spawn.sh"}\n' >&2
exit 1
