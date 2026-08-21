---
name: env-vars
description: Load, list, verify, and store API keys. Canonical source is macOS Keychain (service=lazar). Use when you need a credential, to check which providers work, or after rotating keys. Never print secret values.
---

# env-vars — Keychain-backed API keys

**Canonical store:** macOS Keychain, service `lazar`, account = env var name
(e.g. account `MINIMAX_API_KEY`).

Plaintext `.env` is an optional migration source only.
**Never** echo, log, or commit secret values — names and char-lengths only.

## Scripts (this skill)

All under `$LAZAR_HOME/user/skills/_meta/env-vars/` (or seed `src/seed-skills/_meta/env-vars/`).

| Script | Purpose |
|--------|---------|
| `keychain-list.sh` | Names + present/missing + length |
| `keychain-get.sh NAME` | Print one secret to stdout (for capture) |
| `keychain-get.sh NAME --meta` | Length only |
| `keychain-export.sh` | `source` or `eval "$(… --print)"` to export all |
| `keychain-store.sh` | Import the current environment or `.env` into Keychain |
| `keychain-verify.sh` | Live HTTP probe per key (PASS/FAIL, no secrets) |

## Everyday use

```bash
# What's stored?
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-list.sh"

# Load all keys into this shell
source "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-export.sh"
# or: eval "$(bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-export.sh" --print)"

# One key for a command (don't log it)
export MINIMAX_API_KEY="$(bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-get.sh" MINIMAX_API_KEY)"

# Prove keys still work
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-verify.sh"
```

## First-time / after rotating a key

```bash
# 1. Put the new value in the environment (or temporarily in .env)
export MINIMAX_API_KEY='…'   # paste once; do not commit

# 2. Store into Keychain
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-store.sh" --from-env
# or only one: bash …/keychain-store.sh --one MINIMAX_API_KEY

# 3. Verify
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-verify.sh"
```

Bulk import from existing files (still works if files have values):

```bash
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-store.sh"
```

## Selecting a provider

Use the kernel's provider catalog — never alias one provider's key under
another provider's name:

```bash
lazar --provider minimax -p "…"        # or LAZAR_PROVIDER=minimax
lazar --provider anthropic -p "…"
```

Each `[[providers]]` entry in `providers.toml` names its own `key_env`; the
kernel reads that key from Keychain. `ANTHROPIC_API_KEY` must hold a key issued
by Anthropic (`sk-ant-…`). Writing `export ANTHROPIC_API_KEY="$MINIMAX_API_KEY"`
and then running `keychain-store.sh` persisted a MiniMax key in the Anthropic
slot and every Anthropic call 401'd for weeks. `keychain-store.sh` now refuses
to store a value that already belongs to another key name, and
`keychain-verify.sh` flags cross-provider duplicates.

Non-secret config stays in `$LAZAR_HOME/.env` (e.g. `LAZAR_PROVIDER`, `LAZAR_MODEL`).

## Other sources (manual discovery)

If a key is missing from Keychain, check (names only):

1. `$LAZAR_HOME/.env`
2. `~/.env` (real home)
3. Shell profiles (`export` lines)
4. 1Password CLI: `op item get …` if installed

## Security rules

- **Never** print full key values in agent replies, logs, or git commits.
- Prefer Keychain over plaintext files for secrets.
- After storing, remove secrets from tracked/plaintext files when possible.
- Run `secrets-scan` before sharing any file that might contain keys.
- Keychain may prompt for unlock once per session.

## Self-test

```bash
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-list.sh"
bash "$LAZAR_HOME/user/skills/_meta/env-vars/keychain-verify.sh"
```

The CLI and native TUI both read the same provider catalog. Select with
`--provider <id>`, `LAZAR_PROVIDER=<id>`, or the TUI provider picker.
