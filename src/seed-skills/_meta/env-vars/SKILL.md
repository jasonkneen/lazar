---
name: env-vars
description: Load, list, verify, and store API keys. Canonical source is macOS Keychain (service=lazar). Use when you need a credential, to check which providers work, or after rotating keys. Never print secret values.
---

# env-vars — Keychain-backed API keys

**Canonical store:** macOS Keychain, service `lazar`, account = env var name
(e.g. account `MINIMAX_API_KEY`).

Plaintext files (`.env`, `provider-env.sh`) are loaders/fallbacks only.
**Never** echo, log, or commit secret values — names and char-lengths only.

## Scripts (this skill)

All under `$LAZAR_HOME/skills/env-vars/` (or seed `src/seed-skills/_meta/env-vars/`).

| Script | Purpose |
|--------|---------|
| `keychain-list.sh` | Names + present/missing + length |
| `keychain-get.sh NAME` | Print one secret to stdout (for capture) |
| `keychain-get.sh NAME --meta` | Length only |
| `keychain-export.sh` | `source` or `eval "$(… --print)"` to export all |
| `keychain-store.sh` | Import from `.env` + `provider-env.sh` into Keychain |
| `keychain-verify.sh` | Live HTTP probe per key (PASS/FAIL, no secrets) |

## Everyday use

```bash
# What's stored?
bash "$LAZAR_HOME/skills/env-vars/keychain-list.sh"

# Load all keys into this shell
source "$LAZAR_HOME/skills/env-vars/keychain-export.sh"
# or: eval "$(bash "$LAZAR_HOME/skills/env-vars/keychain-export.sh" --print)"

# One key for a command (don't log it)
export MINIMAX_API_KEY="$(bash "$LAZAR_HOME/skills/env-vars/keychain-get.sh" MINIMAX_API_KEY)"

# Prove keys still work
bash "$LAZAR_HOME/skills/env-vars/keychain-verify.sh"
```

## First-time / after rotating a key

```bash
# 1. Put the new value in the environment (or temporarily in .env)
export MINIMAX_API_KEY='…'   # paste once; do not commit

# 2. Store into Keychain
bash "$LAZAR_HOME/skills/env-vars/keychain-store.sh" --from-env
# or only one: bash …/keychain-store.sh --one MINIMAX_API_KEY

# 3. Verify
bash "$LAZAR_HOME/skills/env-vars/keychain-verify.sh"
```

Bulk import from existing files (still works if files have values):

```bash
bash "$LAZAR_HOME/skills/env-vars/keychain-store.sh"
```

## Provider proxy / lazar launch

`workspace/provider-switch/provider-env.sh` loads from Keychain.
`start-proxy.sh` sources that file.

```bash
source "$LAZAR_HOME/workspace/provider-switch/provider-env.sh"
# or full export:
source "$LAZAR_HOME/skills/env-vars/keychain-export.sh"

export ANTHROPIC_BASE_URL="https://api.minimax.io/anthropic"
export ANTHROPIC_API_KEY="$MINIMAX_API_KEY"
```

Non-secret config stays in `$LAZAR_HOME/.env` (e.g. `ANTHROPIC_BASE_URL`, `LAZAR_MODEL`).

## Other sources (fallback discovery)

If a key is missing from Keychain, check (names only):

1. `$LAZAR_HOME/.env`
2. `~/.env` (real home)
3. `workspace/provider-switch/provider-env.sh`
4. Shell profiles (`export` lines)
5. 1Password CLI: `op item get …` if installed

## Security rules

- **Never** print full key values in agent replies, logs, or git commits.
- Prefer Keychain over plaintext files for secrets.
- After storing, remove secrets from tracked/plaintext files when possible.
- Run `secrets-scan` before sharing any file that might contain keys.
- Keychain may prompt for unlock once per session.

## Self-test

```bash
bash "$LAZAR_HOME/skills/env-vars/keychain-list.sh"
bash "$LAZAR_HOME/skills/env-vars/keychain-verify.sh"
```
