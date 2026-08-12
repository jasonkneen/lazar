# Install

## Requirements

- **macOS** — the kernel relies on `sandbox-exec` for the bash sandbox and
  `chflags uchg` for the kernel binary lock.
- **Rust toolchain** (`cargo` + `rustc`) — the kernel is a Rust binary built
  from `src/`.
- An API key for an Anthropic-protocol provider (see
  [Configuration](configuration.md)).

## From a git clone

```bash
git clone <repo-url> ~/lazar
cd ~/lazar
bash setup.sh

export ANTHROPIC_API_KEY=sk-...
lazar -p "what skills do you have?"
```

`setup.sh` builds the kernel with an external Cargo target directory
(`$LAZAR_HOME/workspace/.cargo-target` by default), installs the binary at
`~/lazar/bin/lazar`, locks it with `chflags uchg`, and seeds `skills/` and
`hooks/` from the copies embedded in the binary. Re-running setup preserves
existing skills, memory, workspace, logs, and source unless you explicitly opt
in (`LAZAR_UPDATE_SOURCE=1` to refresh source, `LAZAR_RESET_ALL=1` for a factory
reset after rebuild).

## Verify the install

```bash
~/lazar/bin/lazar --help      # enumerate the CLI surface
~/lazar/bin/lazar -p "hello"  # first end-to-end model call
```

## Upgrading the kernel

```bash
bash ~/lazar/scripts/kernel-unlock.sh   # unseal src/ + bin/
# edit src/, then rebuild + relock:
bash ~/lazar/scripts/kernel-build.sh
```

## Migrating from an older `~/clawd/lazar` install

```bash
bash ~/clawd/lazar/scripts/migrate-from-clawd.sh
```

This moves the directory to `~/lazar`, optionally updates the
`/usr/local/bin/lazar` symlink, and shows diffs before patching shell rc files
(with backups). Use `--yes` for non-interactive migration. Skills, memory,
workspace, and logs are preserved; rebuild the kernel after the move.

## Factory reset

```bash
lazar --reset-all          # asks for confirmation
lazar --reset-all --yes    # non-interactive (destructive)
```

Wipes `skills/`, `hooks/`, `memory/`, `workspace/`, `logs/`, and saved access
approvals, then re-seeds the embedded skills and hooks. The kernel itself does
not change.
