# Proposal checklist: <name>

- [ ] Keep the source diff focused and reviewable.
- [ ] Replace every placeholder in `README.md`.
- [ ] Replace the fail-closed placeholder in `verify.sh` with focused checks.
- [ ] Dry-build the staged tree with its checked-in lockfile.
- [ ] Review `README.md`, `verify.sh`, and the full diff printed by `kernel-apply.sh`.
- [ ] Apply only from an interactive operator terminal.
- [ ] Keep the printed backup ID until the change is proven in normal use.
