# Plan 009: Make keychain-verify.sh table-driven (nine pasted probe blocks → one loop)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 2f10eb7..HEAD -- src/seed-skills/_meta/env-vars/keychain-verify.sh`
> On any drift, compare the "Current state" excerpts before proceeding; on a
> mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tech-debt (DRY)
- **Planned at**: commit `2f10eb7`, 2026-07-15

## Why this matters

`src/seed-skills/_meta/env-vars/keychain-verify.sh` (178 lines) probes each
stored API key with a live HTTP call. Nine providers are implemented as nine
copy-pasted blocks with the identical shape — `kc_get` → `http_code` →
`case` on the status. Adding a tenth provider means pasting a tenth block.
A provider table plus one loop handles the seven uniform providers; only
MiniMax (POST probe) and Anthropic (base-URL sniffing + retry) are genuinely
special and keep dedicated functions.

## Current state

- The shared helpers already exist and stay:
  `src/seed-skills/_meta/env-vars/keychain-common.sh` provides `kc_get`,
  `LAZAR_KEY_NAMES`, `LAZAR_KC_SERVICE`; `keychain-verify.sh:16-20` defines
  `http_code()`; lines 10-14 define the `ok/no/sk/ms` counters.
- A representative uniform block (`keychain-verify.sh:82-90`, OPENROUTER):

```bash
if val=$(kc_get OPENROUTER_API_KEY 2>/dev/null); then
  code=$(http_code "https://openrouter.ai/api/v1/models" \
    -H "Authorization: Bearer $val")
  case "$code" in
    200) ok "OPENROUTER_API_KEY (HTTP $code)" ;;
    401|403) no "OPENROUTER_API_KEY" "auth rejected HTTP $code" ;;
    *) no "OPENROUTER_API_KEY" "HTTP $code" ;;
  esac
else ms OPENROUTER_API_KEY; fi
```

- The uniform providers and their probe endpoints/auth styles:

| Key | URL | Auth |
|---|---|---|
| OPENROUTER_API_KEY | https://openrouter.ai/api/v1/models | `Authorization: Bearer` |
| XAI_API_KEY | https://api.x.ai/v1/models | `Authorization: Bearer` |
| FIREWORKS_API_KEY | https://api.fireworks.ai/inference/v1/models | `Authorization: Bearer` |
| NVIDIA_API_KEY | https://integrate.api.nvidia.com/v1/models | `Authorization: Bearer` |
| OPENAI_API_KEY | https://api.openai.com/v1/models | `Authorization: Bearer` |
| GEMINI_API_KEY | https://generativelanguage.googleapis.com/v1beta/models?key=$val | query param (also treats 400 as auth-reject) |
| E2B_API_KEY | https://api.e2b.dev/sandboxes | `X-API-KEY:` header (also accepts 201) |

- The special cases that must keep their current logic verbatim:
  - MINIMAX_API_KEY (lines 25-37): POST to the Anthropic-compatible messages
    endpoint with a JSON body.
  - ANTHROPIC_API_KEY (lines 39-76): base-URL resolution from env/.env,
    MiniMax-model retry on 400/404, soft-pass semantics.
  - ZEN_API_KEY (lines 131-151): unknown-endpoint fallback with SKIP
    semantics and key-length heuristic.
- Note the small per-provider variances the table must express: GEMINI adds
  `400` to the reject set and passes the key as a query param; E2B accepts
  `201` as pass and uses a different header name.
- Conventions: `set -uo pipefail` (deliberately no `-e` — the script counts
  failures), sources keychain-common.sh via `$HERE`, exit status is
  `[ "$FAIL" -eq 0 ]`.
- shellcheck is the lint gate if installed (`command -v shellcheck`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Syntax | `bash -n src/seed-skills/_meta/env-vars/keychain-verify.sh` | exit 0 |
| Lint (if installed) | `shellcheck src/seed-skills/_meta/env-vars/keychain-verify.sh` | no errors (warnings acceptable if pre-existing style) |
| Live run (operator machine only) | `bash src/seed-skills/_meta/env-vars/keychain-verify.sh` | same PASS/FAIL/SKIP/MISS totals as before the change |

## Scope

**In scope**:
- `src/seed-skills/_meta/env-vars/keychain-verify.sh` only.

**Out of scope**:
- `keychain-common.sh` and the other keychain-*.sh scripts.
- The set of probed keys, endpoints, pass/fail semantics, output line
  formats (`[PASS] NAME (HTTP 200)` etc.), and the final exit-status rule.
- The runtime copy under `skills/` (gitignored, agent-owned) — seed only.

## Git workflow

- Branch: `advisor/009-keychain-table` off `main`.
- Suggested commit message: `refactor(seed): table-driven provider probes in keychain-verify`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add the generic probe function and table

Below `http_code()`, add:

```bash
# probe_bearer NAME URL AUTH_STYLE EXTRA_PASS EXTRA_REJECT
#   AUTH_STYLE: bearer | xapikey | querykey
probe_provider() {
  local name="$1" url="$2" auth="$3" extra_pass="${4:-}" extra_reject="${5:-}"
  local val code
  if ! val=$(kc_get "$name" 2>/dev/null); then ms "$name"; return; fi
  case "$auth" in
    bearer)   code=$(http_code "$url" -H "Authorization: Bearer $val") ;;
    xapikey)  code=$(http_code "$url" -H "X-API-KEY: $val") ;;
    querykey) code=$(http_code "${url}?key=${val}") ;;
  esac
  case "$code" in
    200|$extra_pass) ok "$name (HTTP $code)" ;;
    401|403|$extra_reject) no "$name" "auth rejected HTTP $code" ;;
    *) no "$name" "HTTP $code" ;;
  esac
}
```

CAUTION on the `case` pattern trick: an empty `$extra_pass` expands to an
empty pattern alternative — bash treats `200|)` as matching the empty string,
which is harmless for numeric `$code` but sloppy. Use a guard value instead:
default `extra_pass="${4:-NONE}"` and `extra_reject="${5:-NONE}"` so the
pattern is `200|NONE)`. Verify this exact concern in Step 3.

### Step 2: Replace the seven uniform blocks with calls

```bash
probe_provider OPENROUTER_API_KEY "https://openrouter.ai/api/v1/models"              bearer
probe_provider XAI_API_KEY        "https://api.x.ai/v1/models"                        bearer
probe_provider FIREWORKS_API_KEY  "https://api.fireworks.ai/inference/v1/models"      bearer
probe_provider NVIDIA_API_KEY     "https://integrate.api.nvidia.com/v1/models"        bearer
probe_provider OPENAI_API_KEY     "https://api.openai.com/v1/models"                  bearer
probe_provider GEMINI_API_KEY     "https://generativelanguage.googleapis.com/v1beta/models" querykey NONE 400
probe_provider E2B_API_KEY        "https://api.e2b.dev/sandboxes"                     xapikey 201
```

Keep MINIMAX, ANTHROPIC, and ZEN blocks as-is (optionally wrapped in
functions `probe_minimax` / `probe_anthropic` / `probe_zen` for readability —
content unchanged). Preserve the original PROBE ORDER (MiniMax, Anthropic,
OpenRouter, XAI, Fireworks, NVIDIA, Gemini, Zen, E2B, OpenAI) so output
remains comparable across runs.

### Step 3: Verify

- `bash -n src/seed-skills/_meta/env-vars/keychain-verify.sh` → exit 0.
- `shellcheck` (if installed) → no new errors.
- Dry semantic check without keys: run with a bogus service so every key is
  missing — `LAZAR_KC_SERVICE=lazar-noexist bash src/seed-skills/_meta/env-vars/keychain-verify.sh`
  → every probed name reports `[MISS]`, final line `PASS=0 FAIL=0 SKIP=0 MISS=10`,
  exit 0. Count MUST be 10 (the same ten keys as before).
- Operator live run (only if this machine has the keys): totals match a
  pre-change run.

### Step 4: Size check

`wc -l src/seed-skills/_meta/env-vars/keychain-verify.sh` → ≤ 120 (was 178).

## Test plan

- The bogus-service dry run in Step 3 is the behavioral test (all-MISS path
  exercises the loop, counters, and exit status without network).
- No selftest.sh exists for env-vars; do not create one in this plan.

## Done criteria

- [ ] `bash -n` → exit 0
- [ ] Bogus-service dry run → `MISS=10`, exit 0
- [ ] `grep -c "kc_get" src/seed-skills/_meta/env-vars/keychain-verify.sh` → ≤ 5 (one in probe_provider + the three special blocks)
- [ ] `wc -l` ≤ 120
- [ ] Only the one file modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- The dry run reports a MISS count ≠ 10 (a provider was dropped or added).
- Any special-case block (MiniMax/Anthropic/Zen) would need behavioral edits
  to fit the abstraction — leave it verbatim instead; if that's impossible,
  report.

## Maintenance notes

- Adding a provider is now one `probe_provider` line plus an entry in
  `LAZAR_KEY_NAMES` (keychain-common.sh) so store/list/export see it too —
  reviewer should check both were updated together.
- The runtime copy under `skills/_meta/env-vars/` will diverge from the seed
  until the agent re-seeds or the operator copies it; that's the designed
  seed-vs-runtime split, not an error.
