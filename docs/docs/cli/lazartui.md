# lazartui

`lazartui` is the native Rust terminal client for the Lazar kernel. It keeps a
single `lazar acp` subprocess alive, so prompts stream without spawning a new
kernel for every turn and sessions survive provider/model switches.

```bash
lazartui
lazartui --provider kimi --model 'kimi-k3[1m]'
lazartui --cwd /path/to/project --add-dir /another/write/root
lazartui --session sess_123
lazartui --icons nerd
lazartui --thin
```

Provider ids and model lists come from `$LAZAR_HOME/providers.toml`; keys stay
in their named environment variables or the macOS Keychain. The retired Go
client and compatibility launchers are kept only in the local `.archive/`
directory and are not part of the active install.

## Core controls

| Key | Action |
|---|---|
| Enter | Send, or queue while a turn runs |
| Shift+Enter / Alt+Enter / Ctrl+J | New line |
| `/` / `@` | Open live command/skill or workspace-path completion |
| Tab | Reopen completion for the current token |
| Mouse wheel | Scroll the message transcript |
| Ctrl+O / Ctrl+P | Model / provider picker |
| Ctrl+C | Cancel a running turn; quit while idle |
| Ctrl+S / Ctrl+X | Steer with the composer / drop queued prompt |
| Ctrl+T | Fold or unfold tool output |
| Ctrl+H / Ctrl+E | Hook trace / raw Lazar event feed |
| Ctrl+G | Sub-agent log panel |
| F1 | Full in-app help |

The default icon mode uses portable Unicode. Use `--icons nerd` (or
`--nerd-font`) when the terminal is configured with a Nerd Font, and
`--icons ascii` on restricted terminals. `LAZARTUI_ICONS` accepts the same
`nerd`, `unicode`, and `ascii` values.

Slash commands include `/provider`, `/model`, `/theme`, `/session`, `/add-dir`,
`/weight`, `/clear`, and `/help`. `/theme` opens a live-preview picker for Auto, NERV,
Grok Night, Grok Day, and Tokyo Night; Escape restores the previous theme and
Enter persists the selection. `LAZARTUI_THEME` overrides the saved theme at
launch. Lazar detects truecolor, 256-color, and basic terminals; `NO_COLOR`
disables palette colors and `LAZARTUI_COLOR=truecolor|256|basic|none` provides
an explicit override. A skill name such as `/todo ...` expands to the kernel's
`[skill:todo]` prompt convention.

Use `--thin`, `--font-weight thin`, or `/weight thin` to stop Lazar from
requesting bold terminal faces. The choice is persisted by the slash command;
`LAZARTUI_FONT_WEIGHT=thin|regular` overrides it at launch. The terminal owns
the actual font family and face, so select a light or thin face in the terminal
profile for genuinely thin glyphs. Lazar deliberately does not use ANSI faint
for this mode because faint reduces contrast instead of selecting a font
weight.

The mouse wheel scrolls a picker when the pointer is over it and scrolls the
message transcript elsewhere. A bright transcript scrollbar means the view is
scrolled away from live output; it becomes subdued again at the bottom.

## Build

```bash
bash scripts/tui-build.sh
```

The script uses `workspace/.cargo-target-tui` and atomically installs the
stripped release binary as `bin/lazartui`.
