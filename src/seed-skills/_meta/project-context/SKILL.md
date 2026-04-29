# project-context

Before modifying any code in a project directory, read the project's `PROJECT.md` (if present) to learn its conventions, structure, and rules. This prevents the most common drift: the agent inventing a parallel implementation instead of editing the one that exists.

## When to use

- The user references a directory or codebase ("the TUI", "the parser", "the lazartui").
- You're about to write code under `$LAZAR_HOME/workspace/<project>/` or similar.
- You need to know "where does X live?" or "what language/framework does this project use?"

## How to use

1. Identify the project root. Usually it's:
   - The deepest directory containing `PROJECT.md` (or `README.md` if no PROJECT.md).
   - The deepest directory containing a manifest like `go.mod`, `Cargo.toml`, `package.json`.

2. Read the project pin:

       cat <project-root>/PROJECT.md 2>/dev/null \
         || cat <project-root>/AGENTS.md 2>/dev/null \
         || ls <project-root>          # fall back to listing files

3. Follow the conventions in PROJECT.md verbatim — language, file layout, build commands, "do not" rules.

4. If PROJECT.md says "always read X first", do that next.

5. If there is NO PROJECT.md, infer from the manifest and existing files. Read the entry point (`main.go`, `src/lib.rs`, `index.ts`) before suggesting changes.

## Hard rules

- **Never create parallel implementations.** If `main.go` exists, modify `main.go`. Don't write `main2.go` "to experiment". If you need to experiment, branch with git, not with file proliferation.
- **Don't introduce new languages or frameworks** unless PROJECT.md explicitly invites it. The presence of `go.mod` means Go is the language; don't suddenly add Python.
- **Read before writing.** For any non-trivial edit, read the file you're about to modify in full. Patch in place; don't rewrite.
- **Stay inside the project root.** Don't create new directories at the workspace level "for organization" unless asked.

## Anti-patterns to recognize in yourself

- "Let me scaffold a quick prototype to test this idea" — no, modify the existing code.
- "I'll create a separate utility for X" — usually X belongs in the existing entry point.
- "Let me set up a build system for this" — the build system already exists; use it.
- "I'll write a Python script to drive the Go program" — no, do it in the language the project is in.

## Authoring a PROJECT.md

If a project doesn't have one and the user is iterating on it often, propose creating one. A good PROJECT.md is short — under 50 lines — and answers:

- What the project IS (one paragraph).
- Where the entry point lives.
- Language, framework, build/run commands.
- Conventions: "this is the ONLY X; don't create parallel ones."
- "Read these files first" when working here.
- Any external tool dependencies.

## Pairing with create-skill

If you find yourself repeatedly fighting the same drift on the same project, that's a sign the PROJECT.md isn't strong enough. Edit the PROJECT.md (it lives in workspace/, you can write there) to encode the lesson.
