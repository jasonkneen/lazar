# memory

Durable notes that survive across `lazar -p` invocations. Memory lives in
`$LAZAR_HOME/memory/` and is plain markdown — no JSON, no encoded blobs.
You and the user should be able to `cat` it and understand instantly.

## When to use

- The user asks you to remember something.
- You learn a fact about the user, the system, or a recurring task that would be useful next session.
- You want to leave breadcrumbs for a future invocation of yourself.
- At the start of a non-trivial task, to load relevant prior context.

## How to use

### Read bounded memory at the start of a task

    cat "$LAZAR_HOME"/memory/*.md 2>/dev/null | head -c 50000 || echo "(no memory yet)"

### Append a timestamped note

    mkdir -p "$LAZAR_HOME/memory"
    cat >> "$LAZAR_HOME/memory/notes.md" <<EOF

    ## $(date -u +%Y-%m-%dT%H:%M:%SZ)
    <the note in one or two sentences>
    EOF

### Write a topic-scoped note

For a topic you'll revisit, give it its own file:

    cat > $LAZAR_HOME/memory/<topic>.md <<EOF
    # <topic>

    <facts and context>
    EOF

### Update an existing topic file

    cat >> $LAZAR_HOME/memory/<topic>.md <<EOF

    ## update $(date -u +%Y-%m-%dT%H:%M:%SZ)
    <new info>
    EOF

## Principles

- Markdown only. Anything else defeats the point.
- Prefer topic files over a single firehose `notes.md` once you have more than ~10 entries on a topic.
- Don't write secrets or API keys to memory. The memory directory is not encrypted.
- Memory survives `lazar --reset-all`? **No** — reset wipes it. If the user wants persistence across resets, they should copy memory/ elsewhere first.
