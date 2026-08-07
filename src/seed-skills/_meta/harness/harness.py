#!/usr/bin/env python3
"""lazar continual harness — ranked, scoped, always-on memory/skill/prompt/subagent store.

Store: $LAZAR_HOME/memory/harness/state.json
History: $LAZAR_HOME/memory/harness/history.jsonl

Kinds: prompt | memory | skill | subagent
Scopes: global (default store) | local (session-scoped file)

Designed so session-start can inject a compact overview without an LLM call.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

KINDS = ("prompt", "memory", "skill", "subagent")
SCOPES = ("global", "local")
SCHEMA = 1
OVERVIEW_MAX_ENTRIES = 16
OVERVIEW_LINE_MAX = 140


def lazar_home() -> Path:
    return Path(os.environ.get("LAZAR_HOME", Path.home() / "lazar")).expanduser()


def global_state_path() -> Path:
    return lazar_home() / "memory" / "harness" / "state.json"


def history_path() -> Path:
    return lazar_home() / "memory" / "harness" / "history.jsonl"


def local_state_path(session_id: str | None) -> Path | None:
    if not session_id:
        session_id = os.environ.get("LAZAR_SESSION_ID") or ""
    session_id = (session_id or "").strip()
    if not session_id:
        return None
    safe = re.sub(r"[^A-Za-z0-9._-]+", "_", session_id)[:64]
    return lazar_home() / "memory" / "sessions" / safe / "harness.json"


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def empty_state() -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "updated_at": now_iso(),
        "entries": {k: {} for k in KINDS},
    }


def load_state(path: Path) -> dict[str, Any]:
    if not path.exists():
        return empty_state()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return empty_state()
    if not isinstance(data, dict):
        return empty_state()
    entries = data.get("entries") or {}
    norm = empty_state()
    norm["schema"] = int(data.get("schema") or SCHEMA)
    norm["updated_at"] = data.get("updated_at") or now_iso()
    for kind in KINDS:
        bucket = entries.get(kind) or {}
        if isinstance(bucket, dict):
            norm["entries"][kind] = {
                eid: e for eid, e in bucket.items() if isinstance(e, dict)
            }
    return norm


def atomic_write_json(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
    fd, tmp = tempfile.mkstemp(prefix=".harness-", suffix=".json", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            try:
                os.unlink(tmp)
            except OSError:
                pass


def save_state(path: Path, state: dict[str, Any]) -> None:
    state["updated_at"] = now_iso()
    atomic_write_json(path, state)


def append_history(event: dict[str, Any]) -> None:
    path = history_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    event = dict(event)
    event.setdefault("ts", now_iso())
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(event, ensure_ascii=False) + "\n")


def slug(text: str, fallback: str = "entry") -> str:
    s = re.sub(r"[^a-z0-9]+", "-", (text or "").lower()).strip("-")
    if not s:
        s = fallback
    return s[:48]


def make_id(kind: str, title: str, content: str) -> str:
    base = slug(title) or slug(content[:40], kind)
    digest = hashlib.md5(f"{kind}:{title}:{content}".encode()).hexdigest()[:6]
    prefix = {"prompt": "P", "memory": "M", "skill": "K", "subagent": "A"}.get(kind, "X")
    return f"{prefix}-{base}-{digest}"


def score(entry: dict[str, Any]) -> float:
    h = float(entry.get("h") or 0)
    m = float(entry.get("m") or 0)
    meta = entry.get("metadata") or {}
    bonus = 0.0
    if meta.get("seed"):
        bonus += 2.0  # curated baseline outranks bulk imports
    src = str(meta.get("imported_from") or "")
    if src.endswith("learnings.md"):
        bonus -= 0.5  # keep available but below explicit seeds
    if src.endswith("playbook.json"):
        bonus += 0.5
    # kind diversity weight applied at overview time; base score here
    return (h - m) + bonus


def all_entries(
    global_state: dict[str, Any], local_state: dict[str, Any] | None
) -> list[tuple[str, str, dict[str, Any]]]:
    """Return list of (scope, kind, entry) with entry including id."""
    out: list[tuple[str, str, dict[str, Any]]] = []
    for scope, state in (("global", global_state), ("local", local_state or empty_state())):
        if state is None:
            continue
        for kind in KINDS:
            for eid, entry in (state.get("entries") or {}).get(kind, {}).items():
                e = dict(entry)
                e["id"] = eid
                e["kind"] = kind
                e["scope"] = scope
                out.append((scope, kind, e))
    return out


def format_overview(
    global_state: dict[str, Any],
    local_state: dict[str, Any] | None = None,
    limit: int = OVERVIEW_MAX_ENTRIES,
) -> str:
    items = all_entries(global_state, local_state)
    items.sort(key=lambda t: (-score(t[2]), t[2].get("id", "")))
    counts = {k: 0 for k in KINDS}
    for _, kind, _ in items:
        counts[kind] += 1

    lines = [
        "CONTINUAL HARNESS (compact — durable lessons ranked by usefulness)",
        f"counts: memory={counts['memory']} skill={counts['skill']} "
        f"prompt={counts['prompt']} subagent={counts['subagent']}",
    ]
    if not items:
        lines.append(
            "(empty) Seed with: python3 $LAZAR_HOME/skills/_meta/harness/harness.py seed"
        )
        lines.append(
            "Write lessons: harness.py add --kind memory --title '...' --content '...'"
        )
        return "\n".join(lines)

    # Prefer kind diversity in the first N lines so prompt/skill/subagent
    # are not drowned by bulk memory imports.
    selected: list[tuple[str, str, dict]] = []
    seen_ids: set[str] = set()
    per_kind_cap = max(2, limit // 4)
    kind_used = {k: 0 for k in KINDS}
    for scope, kind, e in items:
        if len(selected) >= limit:
            break
        eid = e.get("id") or ""
        if eid in seen_ids:
            continue
        if kind_used[kind] >= per_kind_cap and any(
            kind_used[k] < per_kind_cap for k in KINDS if counts[k]
        ):
            continue
        selected.append((scope, kind, e))
        seen_ids.add(eid)
        kind_used[kind] += 1
    if len(selected) < limit:
        for scope, kind, e in items:
            if len(selected) >= limit:
                break
            eid = e.get("id") or ""
            if eid in seen_ids:
                continue
            selected.append((scope, kind, e))
            seen_ids.add(eid)

    shown = 0
    for scope, kind, e in selected:
        if shown >= limit:
            break
        title = (e.get("title") or e.get("id") or "").strip()
        content = (e.get("content") or "").strip().replace("\n", " ")
        body = content if content else title
        if title and content and title.lower() not in content.lower():
            body = f"{title}: {content}"
        body = body[:OVERVIEW_LINE_MAX]
        eid = e.get("id", "?")
        h = int(e.get("h") or 0)
        m = int(e.get("m") or 0)
        lines.append(f"- [{scope}:{kind}:{eid}] (+{h}/-{m}) {body}")
        shown += 1

    remaining = len(items) - shown
    if remaining > 0:
        lines.append(f"... +{remaining} more; harness.py overview --limit 40 or load <id>")
    lines.append(
        "API: python3 $LAZAR_HOME/skills/_meta/harness/harness.py "
        "overview|load|add|helpful|unhelpful|delete|seed"
    )
    lines.append(
        "Delegate isolated file-verifiable work via bin/lazar-spawn + "
        "_meta/subagent pack-and-spawn (not raw lazar -p)."
    )
    return "\n".join(lines)


def cmd_overview(args: argparse.Namespace) -> int:
    g = load_state(global_state_path())
    lp = local_state_path(args.session)
    local = load_state(lp) if lp and lp.exists() else None
    text = format_overview(g, local, limit=args.limit)
    if args.json:
        items = all_entries(g, local)
        items.sort(key=lambda t: (-score(t[2]), t[2].get("id", "")))
        print(
            json.dumps(
                {
                    "overview": text,
                    "entries": [e for _, _, e in items[: args.limit]],
                },
                indent=2,
            )
        )
    else:
        print(text)
    return 0


def find_entry(
    eid: str, global_state: dict[str, Any], local_state: dict[str, Any] | None
) -> tuple[str, str, dict[str, Any], dict[str, Any], Path] | None:
    """Return scope, kind, entry, state, path."""
    candidates: list[tuple[str, dict[str, Any], Path]] = [
        ("global", global_state, global_state_path())
    ]
    lp = local_state_path(None)
    if local_state is not None and lp is not None:
        candidates.insert(0, ("local", local_state, lp))  # prefer local override
    elif lp is not None and lp.exists():
        candidates.insert(0, ("local", load_state(lp), lp))

    for scope, state, path in candidates:
        for kind in KINDS:
            bucket = state.get("entries", {}).get(kind, {})
            if eid in bucket:
                return scope, kind, dict(bucket[eid]), state, path
    # bare id without prefix match
    for scope, state, path in candidates:
        for kind in KINDS:
            for k, v in state.get("entries", {}).get(kind, {}).items():
                if k == eid or k.endswith(eid) or eid in k:
                    return scope, kind, dict(v), state, path
    return None


def cmd_load(args: argparse.Namespace) -> int:
    g = load_state(global_state_path())
    lp = local_state_path(args.session)
    local = load_state(lp) if lp and lp.exists() else None
    found = find_entry(args.id, g, local)
    if not found:
        print(f"not found: {args.id}", file=sys.stderr)
        return 1
    scope, kind, entry, state, path = found
    entry["id"] = args.id if args.id in state["entries"][kind] else entry.get("id")
    # resolve real id
    real_id = None
    for k, v in state["entries"][kind].items():
        if v is entry or k == args.id or args.id in k:
            real_id = k
            entry = dict(v)
            entry["id"] = k
            break
    if real_id is None:
        # find_entry already matched
        for k, v in state["entries"][kind].items():
            if args.id in k or k in args.id:
                real_id = k
                entry = dict(v)
                entry["id"] = k
                break
    entry["kind"] = kind
    entry["scope"] = scope
    if args.helpful:
        state["entries"][kind][entry["id"]]["h"] = int(
            state["entries"][kind][entry["id"]].get("h") or 0
        ) + 1
        state["entries"][kind][entry["id"]]["updated_at"] = now_iso()
        save_state(path, state)
        entry["h"] = state["entries"][kind][entry["id"]]["h"]
    if args.json:
        print(json.dumps(entry, indent=2))
    else:
        print(f"# [{scope}:{kind}:{entry.get('id')}]")
        if entry.get("title"):
            print(f"title: {entry['title']}")
        print(entry.get("content") or "")
        if entry.get("metadata"):
            print(f"metadata: {json.dumps(entry['metadata'])}")
    return 0


def resolve_target(args: argparse.Namespace) -> tuple[Path, dict[str, Any], str]:
    scope = args.scope
    if scope == "local":
        lp = local_state_path(args.session)
        if lp is None:
            raise SystemExit(
                "local scope requires --session or LAZAR_SESSION_ID"
            )
        return lp, load_state(lp), "local"
    path = global_state_path()
    return path, load_state(path), "global"


def cmd_add(args: argparse.Namespace) -> int:
    kind = args.kind
    if kind not in KINDS:
        print(f"invalid kind: {kind}", file=sys.stderr)
        return 2
    content = (args.content or "").strip()
    title = (args.title or "").strip() or content[:60]
    if not content and not title:
        print("need --content or --title", file=sys.stderr)
        return 2
    if not content:
        content = title
    path, state, scope = resolve_target(args)
    # dedupe by similar content
    for eid, e in state["entries"][kind].items():
        if content.lower() in (e.get("content") or "").lower() or (
            e.get("content") or ""
        ).lower() in content.lower():
            if args.force:
                break
            print(json.dumps({"status": "exists", "id": eid, "scope": scope}))
            return 0
    eid = args.id or make_id(kind, title, content)
    entry = {
        "title": title,
        "content": content,
        "h": int(args.h or 0),
        "m": 0,
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "metadata": {},
    }
    if args.evidence:
        entry["metadata"]["evidence"] = args.evidence
    if args.metadata:
        try:
            entry["metadata"].update(json.loads(args.metadata))
        except json.JSONDecodeError as e:
            print(f"bad --metadata JSON: {e}", file=sys.stderr)
            return 2
    state["entries"][kind][eid] = entry
    save_state(path, state)
    append_history(
        {
            "action": "add",
            "scope": scope,
            "kind": kind,
            "id": eid,
            "title": title,
            "evidence": args.evidence or "",
        }
    )
    print(json.dumps({"status": "ok", "id": eid, "scope": scope, "kind": kind}))
    return 0


def cmd_delete(args: argparse.Namespace) -> int:
    g = load_state(global_state_path())
    lp = local_state_path(args.session)
    local = load_state(lp) if lp and lp.exists() else None
    found = find_entry(args.id, g, local)
    if not found:
        print(f"not found: {args.id}", file=sys.stderr)
        return 1
    scope, kind, entry, state, path = found
    # resolve id in bucket
    rid = None
    for k in list(state["entries"][kind].keys()):
        if k == args.id or args.id in k:
            rid = k
            break
    if rid is None:
        print(f"not found: {args.id}", file=sys.stderr)
        return 1
    del state["entries"][kind][rid]
    save_state(path, state)
    append_history({"action": "delete", "scope": scope, "kind": kind, "id": rid})
    print(json.dumps({"status": "ok", "deleted": rid, "scope": scope}))
    return 0


def bump(args: argparse.Namespace, field: str) -> int:
    g = load_state(global_state_path())
    lp = local_state_path(args.session)
    local = load_state(lp) if lp and lp.exists() else None
    found = find_entry(args.id, g, local)
    if not found:
        print(f"not found: {args.id}", file=sys.stderr)
        return 1
    scope, kind, _entry, state, path = found
    rid = None
    for k in state["entries"][kind]:
        if k == args.id or args.id in k:
            rid = k
            break
    if rid is None:
        return 1
    state["entries"][kind][rid][field] = int(state["entries"][kind][rid].get(field) or 0) + 1
    state["entries"][kind][rid]["updated_at"] = now_iso()
    save_state(path, state)
    append_history({"action": field, "scope": scope, "kind": kind, "id": rid})
    print(
        json.dumps(
            {
                "status": "ok",
                "id": rid,
                field: state["entries"][kind][rid][field],
            }
        )
    )
    return 0


def cmd_helpful(args: argparse.Namespace) -> int:
    return bump(args, "h")


def cmd_unhelpful(args: argparse.Namespace) -> int:
    return bump(args, "m")


SEED_BULLETS: list[tuple[str, str, str]] = [
    (
        "memory",
        "never-cat-stream-log",
        "NEVER cat stream.jsonl — can be MB+ and exceed context. Use _meta/load-context bounds (tail -n / jq).",
    ),
    (
        "memory",
        "sandbox-write-zones",
        "Sandbox blocks writes to bin/ and src/. Evolve via skills/, memory/, workspace/. Kernel changes use propose-kernel-patch + operator apply.",
    ),
    (
        "prompt",
        "referential-prompts",
        "Short referential prompts (yes/do that/continue) without session history: do not invent the referent from tool noise; load context or ask one clarifying question.",
    ),
    (
        "prompt",
        "inline-vs-spawn",
        "Do routine bash yourself. Spawn via bin/lazar-spawn only for isolated, file-verifiable tasks (prefer pack-and-spawn BRIEF). Never raw lazar -p.",
    ),
    (
        "skill",
        "subagent-pack",
        "Delegation: write workspace/tasks/<id>/BRIEF.md with objective, absolute paths, SUCCESS, EXPECT-VERIFY, constraints; dispatch with pack-and-spawn.sh or lazar-spawn.",
    ),
    (
        "skill",
        "harness-api",
        "Durable lessons go to continual harness (harness.py add), not only freeform markdown. Overview is auto-injected at session-start.",
    ),
    (
        "memory",
        "verify-contract",
        "Outcome claims require [VERIFY]...[/VERIFY] with exists/contains/size_* claims. Kernel checks the filesystem; truncated [/VERIFY close is accepted.",
    ),
    (
        "memory",
        "nested-api-key",
        "Nested lazar children need API key inheritance only when intentional (LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY=1). Default isolation is deliberate.",
    ),
    (
        "subagent",
        "file-verifiable-units",
        "Child prompts must be stranger-complete: absolute paths, SUCCESS line, EXPECT-VERIFY claims authored by parent. Trust matched envelope, not child prose.",
    ),
]


def cmd_seed(args: argparse.Namespace) -> int:
    path = global_state_path()
    state = load_state(path)
    if any(state["entries"][k] for k in KINDS) and not args.force:
        print(
            f"harness already has entries at {path}. Use --force to re-seed missing ids only.",
            file=sys.stderr,
        )
        # still fill any missing seed ids
    added = []
    for kind, title, content in SEED_BULLETS:
        eid = make_id(kind, title, content)
        # stable seed ids
        eid = f"{kind[0].upper()}-{slug(title)}"
        if eid in state["entries"][kind] and not args.force:
            continue
        if eid in state["entries"][kind] and args.force:
            pass
        state["entries"][kind][eid] = {
            "title": title,
            "content": content,
            "h": 1,
            "m": 0,
            "created_at": now_iso(),
            "updated_at": now_iso(),
            "metadata": {"seed": True},
        }
        added.append(eid)
    # import playbook if present
    playbook = lazar_home() / "memory" / "self-learning" / "playbook.json"
    if playbook.exists() and args.import_playbook:
        try:
            data = json.loads(playbook.read_text(encoding="utf-8"))
            for b in data.get("bullets") or []:
                content = (b.get("content") or "").strip()
                if not content:
                    continue
                cat = b.get("category") or "memory"
                kind = {
                    "pitfall": "memory",
                    "strategy": "memory",
                    "domain": "memory",
                    "code": "skill",
                    "endpoint": "memory",
                }.get(cat, "memory")
                eid = b.get("id") or make_id(kind, content[:40], content)
                if eid in state["entries"][kind] and not args.force:
                    continue
                state["entries"][kind][eid] = {
                    "title": content[:60],
                    "content": content,
                    "h": int(b.get("h") or 0),
                    "m": int(b.get("m") or 0),
                    "created_at": b.get("created") or now_iso(),
                    "updated_at": now_iso(),
                    "metadata": {"imported_from": "playbook.json", "category": cat},
                }
                added.append(eid)
        except (OSError, json.JSONDecodeError) as e:
            print(f"playbook import skipped: {e}", file=sys.stderr)

    # import top learnings lines
    learnings = lazar_home() / "memory" / "self-improving-agent" / "learnings.md"
    if learnings.exists() and args.import_learnings:
        try:
            raw = learnings.read_text(encoding="utf-8")
            # Only high-signal sections; skip long technical dumps.
            allow_heads = (
                "communication preferences",
                "corrections",
                "what works well",
                "ui preferences",
            )
            prefs = []
            active = False
            for line in raw.splitlines():
                stripped = line.strip()
                if stripped.startswith("#"):
                    active = any(h in stripped.lower() for h in allow_heads)
                    continue
                if active and stripped.startswith("- "):
                    prefs.append(stripped[2:].strip())
            for content in prefs[:12]:
                eid = make_id("memory", content[:40], content)
                if any(
                    content.lower() in (e.get("content") or "").lower()
                    for e in state["entries"]["memory"].values()
                ):
                    continue
                state["entries"]["memory"][eid] = {
                    "title": content[:60],
                    "content": content,
                    "h": 0,
                    "m": 0,
                    "created_at": now_iso(),
                    "updated_at": now_iso(),
                    "metadata": {"imported_from": "learnings.md"},
                }
                added.append(eid)
        except OSError as e:
            print(f"learnings import skipped: {e}", file=sys.stderr)

    save_state(path, state)
    append_history({"action": "seed", "added": added})
    print(json.dumps({"status": "ok", "path": str(path), "added": added, "count_added": len(added)}))
    return 0


def cmd_stats(args: argparse.Namespace) -> int:
    g = load_state(global_state_path())
    items = all_entries(g, None)
    print(f"global entries: {len(items)}")
    for kind in KINDS:
        n = len(g["entries"][kind])
        print(f"  {kind}: {n}")
    print(f"state: {global_state_path()}")
    print(f"history: {history_path()}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="harness.py", description="lazar continual harness")
    p.add_argument("--session", default=None, help="session id for local scope")
    sub = p.add_subparsers(dest="cmd", required=True)

    o = sub.add_parser("overview", help="compact ranked overview (for session-start inject)")
    o.add_argument("--limit", type=int, default=OVERVIEW_MAX_ENTRIES)
    o.add_argument("--json", action="store_true")
    o.set_defaults(func=cmd_overview)

    l = sub.add_parser("load", help="load one entry by id")
    l.add_argument("id")
    l.add_argument("--json", action="store_true")
    l.add_argument("--helpful", action="store_true", help="increment helpful score")
    l.set_defaults(func=cmd_load)

    a = sub.add_parser("add", help="add an entry")
    a.add_argument("--kind", required=True, choices=KINDS)
    a.add_argument("--title", default="")
    a.add_argument("--content", default="")
    a.add_argument("--scope", default="global", choices=SCOPES)
    a.add_argument("--id", default=None)
    a.add_argument("--evidence", default="")
    a.add_argument("--metadata", default=None, help="JSON object string")
    a.add_argument("--h", type=int, default=0)
    a.add_argument("--force", action="store_true")
    a.set_defaults(func=cmd_add)

    d = sub.add_parser("delete", help="delete entry by id")
    d.add_argument("id")
    d.set_defaults(func=cmd_delete)

    h = sub.add_parser("helpful", help="upvote entry")
    h.add_argument("id")
    h.set_defaults(func=cmd_helpful)

    u = sub.add_parser("unhelpful", help="downvote entry")
    u.add_argument("id")
    u.set_defaults(func=cmd_unhelpful)

    s = sub.add_parser("seed", help="seed baseline + optional imports")
    s.add_argument("--force", action="store_true")
    s.add_argument("--import-playbook", action="store_true", default=True)
    s.add_argument("--no-import-playbook", action="store_false", dest="import_playbook")
    s.add_argument("--import-learnings", action="store_true", default=True)
    s.add_argument("--no-import-learnings", action="store_false", dest="import_learnings")
    s.set_defaults(func=cmd_seed)

    st = sub.add_parser("stats")
    st.set_defaults(func=cmd_stats)
    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
