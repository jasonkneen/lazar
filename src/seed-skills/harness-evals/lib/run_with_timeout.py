#!/usr/bin/env python3
"""Run one argv with bounded wall time and captured stdout/stderr."""
from __future__ import annotations

import argparse
import os
import signal
import subprocess
import time
from pathlib import Path


class ForwardedSignal(Exception):
    def __init__(self, signum: int):
        self.signum = signum


def stop_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        process.wait()
        return
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        process.poll()
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            process.wait()
            return
        time.sleep(0.02)
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def watch_group(process: subprocess.Popen[bytes]):
    """A separate watchdog owns cleanup if the runner is killed with SIGKILL."""
    reader, writer = os.pipe()
    watcher = os.fork()
    if watcher == 0:
        os.close(writer)
        os.setsid()
        for descriptor in (0, 1, 2):
            try:
                os.close(descriptor)
            except OSError:
                pass
        try:
            while os.read(reader, 1):
                pass
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        finally:
            os._exit(0)
    os.close(reader)
    return watcher, writer


def finish_watch(watch):
    watcher, writer = watch
    os.close(writer)
    os.waitpid(watcher, 0)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--stdout", required=True)
    parser.add_argument("--stderr", required=True)
    parser.add_argument("--cwd", default="")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if args.timeout <= 0 or not command:
        parser.error("positive --timeout and a command after -- are required")

    Path(args.stdout).parent.mkdir(parents=True, exist_ok=True)
    Path(args.stderr).parent.mkdir(parents=True, exist_ok=True)
    with open(args.stdout, "wb") as stdout, open(args.stderr, "wb") as stderr:
        process = subprocess.Popen(
            command,
            cwd=args.cwd or None,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )

        watch = watch_group(process)

        def interrupted(signum: int, _frame: object) -> None:
            raise ForwardedSignal(signum)

        previous_int = signal.signal(signal.SIGINT, interrupted)
        previous_term = signal.signal(signal.SIGTERM, interrupted)
        try:
            return process.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            stop_group(process)
            return 124
        except ForwardedSignal as event:
            stop_group(process)
            return 128 + event.signum
        finally:
            finish_watch(watch)
            signal.signal(signal.SIGINT, previous_int)
            signal.signal(signal.SIGTERM, previous_term)


if __name__ == "__main__":
    raise SystemExit(main())
