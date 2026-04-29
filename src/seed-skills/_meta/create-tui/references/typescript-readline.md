# TypeScript + readline reference

The default lazar TUI stack: TypeScript + Node + readline. Single `src/cli.ts` file, no compile-step gotchas, cross-platform, adapts to terminal theme.

This template is adapted from OpenRouter's `create-agent-tui` skill but stripped of the agent-runner half (the @openrouter/agent SDK pieces) — lazar IS the agent. The TUI just spawns `lazar -p --output-format stream-json` and renders.

## Project layout

```
workspace/<name>/
├── package.json
├── tsconfig.json
├── PROJECT.md                  # see _meta/project-context
├── src/
│   ├── cli.ts                  # entry point, REPL loop
│   ├── lazar.ts                # subprocess wrapper, parses stream-json
│   ├── render.ts               # input style + tool display + loader
│   └── terminal.ts             # ANSI helpers, terminal-bg detection
└── .npmrc                      # NPM_CONFIG_CACHE pointing inside workspace
```

## Setup commands

```bash
cd $LAZAR_HOME/workspace/<name>

# scoped npm cache (sandbox-safe)
mkdir -p .npm
echo "cache=$(pwd)/.npm" > .npmrc

# init
npm init -y
npm pkg set type=module
npm pkg set scripts.start="tsx src/cli.ts"
npm pkg set scripts.dev="tsx watch src/cli.ts"

# deps
npm install marked marked-terminal
npm install -D tsx typescript @types/node
```

## tsconfig.json

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "outDir": "dist",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src"]
}
```

## src/lazar.ts — subprocess wrapper

```typescript
import { spawn, type ChildProcess } from 'child_process';

export type LazarEvent =
  | { type: 'invoke_start'; depth: number; model: string; prompt: string; ts_ms: number }
  | { type: 'text_delta'; index: number; text: string; ts_ms: number }
  | { type: 'text_done'; index: number; ts_ms: number }
  | { type: 'tool_use'; index: number; id: string; name: string; input: Record<string, unknown>; ts_ms: number }
  | { type: 'tool_result'; tool_use_id: string; command: string; content: string; ts_ms: number }
  | { type: 'invoke_end'; stop_reason: string; duration_ms: number; ts_ms: number }
  | { type: 'error'; message: string; ts_ms: number };

export interface LazarRunOptions {
  onEvent: (event: LazarEvent) => void;
  signal?: AbortSignal;
}

export function runLazar(prompt: string, opts: LazarRunOptions): Promise<{ exitCode: number; stderr: string }> {
  return new Promise((resolve) => {
    const proc: ChildProcess = spawn('lazar', ['-p', prompt, '--output-format', 'stream-json'], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let stderr = '';
    proc.stderr?.setEncoding('utf-8');
    proc.stderr?.on('data', (chunk) => { stderr += chunk; });

    let leftover = '';
    proc.stdout?.setEncoding('utf-8');
    proc.stdout?.on('data', (chunk: string) => {
      const text = leftover + chunk;
      const lines = text.split('\n');
      leftover = lines.pop() ?? '';
      for (const line of lines) {
        if (!line.trim()) continue;
        try {
          const event = JSON.parse(line) as LazarEvent;
          opts.onEvent(event);
        } catch {
          // not JSON — skip
        }
      }
    });

    if (opts.signal) {
      opts.signal.addEventListener('abort', () => proc.kill('SIGTERM'));
    }

    proc.on('close', (code) => {
      if (leftover.trim()) {
        try { opts.onEvent(JSON.parse(leftover) as LazarEvent); } catch {}
      }
      resolve({ exitCode: code ?? 0, stderr });
    });
  });
}
```

## src/render.ts — display

Three input styles, three tool display styles. Pick the user's choice at startup and pass to the renderer.

```typescript
const RESET = '\x1b[0m';
const BOLD = '\x1b[1m';
const DIM = '\x1b[2m';
const CYAN = '\x1b[36m';
const GRAY = '\x1b[90m';
const GREEN = '\x1b[32m';
const YELLOW = '\x1b[33m';
const RED = '\x1b[31m';

export type ToolDisplay = 'grouped' | 'emoji' | 'minimal' | 'hidden';
export type InputStyle = 'block' | 'bordered' | 'plain';

export interface RenderConfig {
  toolDisplay: ToolDisplay;
  inputStyle: InputStyle;
}

const toolStartMs = new Map<string, number>();

export function onToolUse(cfg: RenderConfig, id: string, name: string, input: Record<string, unknown>) {
  if (cfg.toolDisplay === 'hidden') return;
  toolStartMs.set(id, Date.now());
  const cmd = String(input.command ?? '').slice(0, 80);
  switch (cfg.toolDisplay) {
    case 'grouped':
      process.stdout.write(`\n  ${BOLD}${name}${RESET}\n  ${GRAY}└─${RESET} ${DIM}${cmd}${RESET}\n`);
      break;
    case 'emoji':
      process.stdout.write(`\n  ${YELLOW}⚡${RESET} ${DIM}${name}: ${cmd}${RESET}\n`);
      break;
    case 'minimal':
      process.stdout.write(`\n  ${DIM}↳ ${cmd}${RESET}\n`);
      break;
  }
}

export function onToolResult(cfg: RenderConfig, id: string, content: string) {
  if (cfg.toolDisplay === 'hidden') return;
  const ms = Date.now() - (toolStartMs.get(id) ?? Date.now());
  toolStartMs.delete(id);
  const preview = content.split('\n').slice(0, 3).join('\n').slice(0, 200);
  switch (cfg.toolDisplay) {
    case 'grouped':
      process.stdout.write(`  ${GRAY}└─${RESET} ${DIM}${preview}${RESET}\n`);
      break;
    case 'emoji':
      process.stdout.write(`  ${GREEN}✓${RESET} ${DIM}(${(ms/1000).toFixed(1)}s)${RESET}\n`);
      break;
    case 'minimal':
      // already shown via tool_use
      break;
  }
}

export function onError(message: string) {
  process.stdout.write(`\n  ${RED}error:${RESET} ${message}\n`);
}

// — input styles —

export function readPrompt(cfg: RenderConfig, rl: any): Promise<string> {
  return new Promise((resolve) => {
    switch (cfg.inputStyle) {
      case 'block':
        process.stdout.write(`\n${CYAN}▶${RESET} `);
        break;
      case 'bordered':
        process.stdout.write(`\n${GRAY}─────────────${RESET}\n${CYAN}▶${RESET} `);
        break;
      case 'plain':
      default:
        process.stdout.write(`> `);
    }
    rl.once('line', (line: string) => resolve(line));
  });
}
```

## src/cli.ts — entry

```typescript
import { createInterface } from 'readline';
import { runLazar } from './lazar.js';
import { onToolUse, onToolResult, onError, readPrompt, type RenderConfig } from './render.js';
import { marked } from 'marked';
import TerminalRenderer from 'marked-terminal';

marked.setOptions({ renderer: new TerminalRenderer() as any });

const cfg: RenderConfig = {
  toolDisplay: 'grouped',
  inputStyle: 'block',
};

async function main() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });

  console.log('\n  \x1b[1mlazar TUI\x1b[0m  \x1b[2m— Enter to send, Ctrl-C to quit\x1b[0m\n');

  while (true) {
    const prompt = (await readPrompt(cfg, rl)).trim();
    if (!prompt) continue;
    if (prompt === 'exit' || prompt === 'quit' || prompt === ':q') break;

    // collect text per block, render on text_done
    const textBuffers = new Map<number, string>();

    const { exitCode, stderr } = await runLazar(prompt, {
      onEvent: (event) => {
        switch (event.type) {
          case 'invoke_start':
            // could show a loader here
            break;
          case 'text_delta':
            textBuffers.set(event.index, (textBuffers.get(event.index) ?? '') + event.text);
            // optional: write raw delta for liveness, then re-render on text_done
            process.stdout.write(event.text);
            break;
          case 'text_done': {
            const md = textBuffers.get(event.index) ?? '';
            process.stdout.write('\n');
            // re-render the block with markdown
            try {
              const rendered = marked(md) as string;
              // clear lines we wrote during streaming, then print rendered
              // (rough — for a polished TUI use a viewport library)
            } catch {}
            textBuffers.delete(event.index);
            break;
          }
          case 'tool_use':
            onToolUse(cfg, event.id, event.name, event.input);
            break;
          case 'tool_result':
            onToolResult(cfg, event.tool_use_id, event.content);
            break;
          case 'invoke_end':
            process.stdout.write('\n');
            break;
          case 'error':
            onError(event.message);
            break;
        }
      },
    });

    if (exitCode !== 0 && stderr) {
      // surface stderr only when lazar errored AND didn't already emit an `error` event
      onError(stderr.trim());
    }
  }

  rl.close();
  console.log('\n\x1b[2mbye.\x1b[0m');
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
```

## Build + run

```bash
cd $LAZAR_HOME/workspace/<name>
npx tsc --noEmit       # type-check
npm start              # runs via tsx
```

For a packaged launcher, add to `aliases.sh` via `_meta/shell-handoff`:

```bash
# in workspace/aliases.sh
alias <name>='cd $LAZAR_HOME/workspace/<name> && npm start'
```

## Notes

- `process.stdout.write` is unbuffered enough for most cases. If you see batching, set `process.stdout.setNoDelay?.(true)` or use `process.stdout._handle?.setBlocking(true)` (latter is internal).
- `marked-terminal` doesn't handle every markdown edge case perfectly. If you need higher-quality rendering, consider piping each completed text block through `glow` via `child_process.execSync('glow', { input: md })` — slower but prettier.
- For a truly polished TUI (split panes, scrollback, cursor management), TypeScript + readline isn't enough. Switch to **Ink** (`npm i ink`) for React-style components, or move to Go + Bubble Tea. The default template is intentionally minimal.

## Compose with other skills

- `_meta/shell-handoff` — expose the TUI as a shell command.
- `_meta/project-context` — write a `PROJECT.md` so the agent stays focused on this directory.
