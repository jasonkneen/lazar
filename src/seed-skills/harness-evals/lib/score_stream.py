#!/usr/bin/env python3
"""Score observable agent outcomes from a strict stream-json transcript.

The scorer deliberately ignores claims in assistant prose. A task passes only
when its declared filesystem and structured-event checks pass and the stream
has a clean terminal event.
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any, Optional


def load_events(path: str | Path) -> tuple[list[dict[str, Any]], list[str]]:
    events: list[dict[str, Any]] = []
    errors: list[str] = []
    for number, raw in enumerate(Path(path).read_text(errors="replace").splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            errors.append(f"line {number}: invalid JSON ({error.msg})")
            continue
        if not isinstance(value, dict):
            errors.append(f"line {number}: event is not an object")
            continue
        events.append(value)
    if not events and not errors:
        errors.append("stream contains no events")
    return events, errors


def event_type(event: dict[str, Any]) -> str:
    return str(event.get("type") or event.get("kind") or event.get("event") or "")


def assistant_text(events: list[dict[str, Any]]) -> str:
    parts: list[str] = []
    for event in events:
        kind = event_type(event)
        if kind in ("text_delta", "content_block_delta"):
            text = event.get("text")
            if isinstance(text, str):
                parts.append(text)
            delta = event.get("delta")
            if isinstance(delta, dict) and delta.get("type") == "text_delta":
                parts.append(str(delta.get("text") or ""))
        elif kind in ("assistant", "message"):
            content = event.get("content")
            if isinstance(content, str):
                parts.append(content)
            elif isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        parts.append(str(block.get("text") or ""))
    return "".join(parts)


def tool_commands(events: list[dict[str, Any]]) -> list[str]:
    commands: list[str] = []
    for event in events:
        kind = event_type(event)
        if kind in ("tool_use", "tool_call"):
            arguments = event.get("input") or event.get("arguments") or {}
            if isinstance(arguments, str):
                try:
                    arguments = json.loads(arguments)
                except json.JSONDecodeError:
                    arguments = {"command": arguments}
            if isinstance(arguments, dict):
                command = arguments.get("command") or arguments.get("cmd")
                if command:
                    commands.append(str(command))
            if event.get("command"):
                commands.append(str(event["command"]))
        elif kind == "tool_result":
            command = event.get("executed_command") or event.get("command")
            if command:
                commands.append(str(command))
    return commands


def tool_results(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [event for event in events if event_type(event) == "tool_result"]


def tool_result_text(events: list[dict[str, Any]]) -> str:
    chunks: list[str] = []
    for event in tool_results(events):
        content = event.get("content")
        if isinstance(content, str):
            chunks.append(content)
        elif content is not None:
            chunks.append(json.dumps(content, sort_keys=True))
    return "\n".join(chunks)


def tool_exit_code(event: dict[str, Any]) -> Optional[int]:
    value = event.get("exit_code")
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, str) and re.fullmatch(r"-?\d+", value):
        return int(value)
    return None


def has_event(events: list[dict[str, Any]], pattern: str) -> bool:
    expression = re.compile(pattern)
    return any(expression.search(event_type(event)) for event in events)


def terminal_state(events: list[dict[str, Any]]) -> tuple[bool, bool, str]:
    """Return (complete, successful, note) from structured terminal evidence."""
    complete, successful, note = False, False, "missing terminal event"
    reason = ""
    invocation_open = False
    for event in events:
        kind = event_type(event)
        if kind == "error":
            return True, False, "error event present"
        if kind == "invoke_start":
            invocation_open = True
            complete, successful, note = False, False, "unfinished final invocation"
            reason = ""
        elif kind == "message_start":
            complete, successful, note = False, False, "unfinished final message"
            reason = str((event.get("message") or {}).get("stop_reason") or "")
        elif kind == "message_delta":
            reason = str((event.get("delta") or {}).get("stop_reason") or reason)
        elif kind == "invoke_end":
            invocation_open = False
            reason = str(event.get("stop_reason") or "")
            complete, successful, note = True, reason in ("end_turn", "stop_sequence"), f"invoke_end stop_reason={reason}"
        elif kind in ("result", "end"):
            invocation_open = False
            reason = str(event.get("stop_reason") or event.get("status") or "")
            successful = not bool(event.get("is_error") or event.get("error")) and reason in ("end_turn", "stop_sequence", "success", "ok")
            complete, note = True, f"{kind} status={reason or '<missing>'}"
        elif kind == "message_stop" and not invocation_open:
            complete, successful, note = True, reason in ("end_turn", "stop_sequence"), f"message_stop stop_reason={reason or '<missing>'}"
        elif kind in ("tool_use", "tool_call", "tool_result", "text_delta", "content_block_start", "content_block_delta"):
            complete, successful, note = False, False, "activity after last terminal event"
    return complete, successful, note


def expand(value: str, context: dict[str, str]) -> str:
    result = value
    for key, replacement in context.items():
        result = result.replace("{{" + key + "}}", replacement)
    return result


def expanded_check(check: dict[str, Any], context: dict[str, str]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in check.items():
        if isinstance(value, str):
            result[key] = expand(value, context)
        elif isinstance(value, list):
            result[key] = [expand(item, context) if isinstance(item, str) else item for item in value]
        else:
            result[key] = value
    return result


def run_check(
    raw_check: dict[str, Any], events: list[dict[str, Any]], context: dict[str, str]
) -> tuple[bool, str]:
    check = expanded_check(raw_check, context)
    kind = str(check.get("type") or "")

    if kind == "stream_tool_ran":
        token = str(check["token"])
        evidence = "\n".join(tool_commands(events)) + "\n" + tool_result_text(events)
        ok = token in evidence
        return ok, "tool activity contains required token" if ok else f"tool activity lacks {token!r}"

    if kind == "stream_tool_result_contains":
        needle = str(check["needle"])
        ok = needle in tool_result_text(events)
        return ok, "tool result contains required text" if ok else f"tool results lack {needle!r}"

    if kind == "stream_min_tool_calls":
        actual = sum(event_type(event) in ("tool_use", "tool_call") for event in events)
        required = int(check["min"])
        return actual >= required, f"tool_calls={actual}, required>={required}"

    if kind == "stream_max_tool_calls":
        actual = sum(event_type(event) in ("tool_use", "tool_call") for event in events)
        allowed = int(check["max"])
        return actual <= allowed, f"tool_calls={actual}, allowed<={allowed}"

    if kind == "stream_failed_tool_result":
        count = sum((tool_exit_code(event) or 0) != 0 for event in tool_results(events))
        return count > 0, f"failed_tool_results={count}"

    if kind == "stream_successful_tool_result":
        count = sum(tool_exit_code(event) == 0 for event in tool_results(events))
        return count > 0, f"successful_tool_results={count}"

    if kind == "stream_end_ok":
        complete, successful, note = terminal_state(events)
        return complete and successful, note

    if kind == "stream_has_event":
        pattern = str(check["event"])
        ok = has_event(events, pattern)
        return ok, f"event matches {pattern!r}" if ok else f"no event matches {pattern!r}"

    if kind == "text_nonempty":
        ok = bool(assistant_text(events).strip())
        return ok, "assistant text present" if ok else "assistant text absent"

    if kind == "text_contains":
        needle = str(check["needle"])
        ok = needle in assistant_text(events)
        return ok, "assistant text contains required text" if ok else f"assistant text lacks {needle!r}"

    if kind in ("fs_exists", "fs_not_exists", "fs_contains", "fs_exact"):
        path = Path(str(check["path"])).expanduser()
        if kind == "fs_exists":
            return path.exists(), f"exists={path.exists()}: {path}"
        if kind == "fs_not_exists":
            return not path.exists(), f"absent={not path.exists()}: {path}"
        if not path.is_file():
            return False, f"not a file: {path}"
        try:
            body = path.read_text(errors="replace")
        except OSError as error:
            return False, f"could not read {path}: {error}"
        if kind == "fs_contains":
            needle = str(check["needle"])
            return needle in body, f"contains {needle!r}" if needle in body else f"missing {needle!r}"
        expected = str(check["content"])
        return body == expected, "exact file content matches" if body == expected else "file content differs"

    if kind == "any_fs_exists":
        paths = [Path(str(path)).expanduser() for path in check.get("paths") or []]
        ok = any(path.exists() for path in paths)
        return ok, "at least one declared path exists" if ok else "none of the declared paths exist"

    return False, f"unknown check type: {kind}"


def score_task(
    task: dict[str, Any],
    events: list[dict[str, Any]],
    context: dict[str, str],
    parse_errors: Optional[list[str]] = None,
    adapter_exit_code: int = 0,
) -> dict[str, Any]:
    parse_errors = parse_errors or []
    checks: list[dict[str, Any]] = []
    for check in task.get("checks") or []:
        ok, note = run_check(check, events, context)
        checks.append({"type": check.get("type"), "ok": ok, "note": note})

    optional = bool(task.get("optional"))
    complete, terminal_ok, terminal_note = terminal_state(events)
    infrastructure_error = adapter_exit_code != 0 or bool(parse_errors) or not complete or has_event(events, r"^error$")
    passed = (
        bool(checks)
        and all(check["ok"] for check in checks)
        and terminal_ok
        and not infrastructure_error
    )

    if passed:
        status = "pass"
    elif optional:
        status = "skip_optional"
    elif infrastructure_error:
        status = "error"
    else:
        status = "fail"

    results = tool_results(events)
    return {
        "id": task.get("id"),
        "suite": task.get("suite"),
        "title": task.get("title"),
        "status": status,
        "passed": passed,
        "optional": optional,
        "checks": checks,
        "parse_errors": parse_errors,
        "adapter_exit_code": adapter_exit_code,
        "metrics": {
            "events": len(events),
            "parse_errors": len(parse_errors),
            "tool_calls": sum(event_type(event) in ("tool_use", "tool_call") for event in events),
            "tool_results": len(results),
            "failed_tool_results": sum((tool_exit_code(event) or 0) != 0 for event in results),
            "successful_tool_results": sum(tool_exit_code(event) == 0 for event in results),
            "text_chars": len(assistant_text(events)),
            "terminal_complete": complete,
            "terminal_ok": terminal_ok,
            "terminal_note": terminal_note,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Score observable Lazar harness outcomes")
    parser.add_argument("stream", help="path to strict stream JSONL")
    parser.add_argument("--task", required=True, help="path to task JSON")
    parser.add_argument("--ctx", default="{}", help="JSON context for placeholders")
    parser.add_argument("--adapter-exit-code", type=int, default=0)
    parser.add_argument("--out", default="", help="optional score JSON destination")
    args = parser.parse_args()

    task = json.loads(Path(args.task).read_text())
    context = json.loads(args.ctx)
    events, parse_errors = load_events(args.stream)
    result = score_task(task, events, context, parse_errors, args.adapter_exit_code)
    rendered = json.dumps(result, indent=2)
    if args.out:
        Path(args.out).write_text(rendered + "\n")
    print(rendered)
    if result["passed"] or result["optional"]:
        return 0
    return 2 if result["status"] == "error" else 1


if __name__ == "__main__":
    raise SystemExit(main())
