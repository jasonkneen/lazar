# Kernel proposal: <name>

## Purpose

Replace this paragraph with the observed defect and the intended outcome.

## Files changed

- List each changed source file and why it must change.

## Risk

Describe the failure modes and the checks in `verify.sh` that cover them.

## Rollback

`kernel-apply.sh` restores the trusted pre-apply backup automatically on build
or verification failure. On later regressions, use the backup ID printed by the
apply command:

```bash
bash scripts/kernel-rollback.sh <backup-id>
```
