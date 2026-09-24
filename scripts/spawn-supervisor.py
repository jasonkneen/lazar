#!/usr/bin/env python3
"""Own a detached job's lifetime, process group, and atomic result record."""
from __future__ import annotations

import fcntl
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def write_record(path, record):
    temporary = path.with_suffix(f'.{os.getpid()}.tmp')
    temporary.write_text(json.dumps(record) + '\n')
    temporary.replace(path)


def check_claims(verifier, claims):
    """Failed claims, checked by the kernel's `lazar verify` (one checker)."""
    try:
        done = subprocess.run([verifier, 'verify'], input='\n'.join(claims) + '\n',
                              capture_output=True, text=True, timeout=60)
        report = json.loads(done.stdout)
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        return [f'lazar verify unavailable: {error}']
    return [f"{c['claim']} — {c['detail']}" for c in report.get('claims', []) if not c.get('ok')]


def bump_failure(filename):
    path = Path(filename)
    with open(str(path) + '.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        count = int(path.read_text()) if path.exists() else 0
        temporary = path.with_suffix(f'.{os.getpid()}.tmp')
        temporary.write_text(str(count + 1))
        temporary.replace(path)


def main():
    mode, filename = sys.argv[1:]
    path = Path(filename)
    if mode == "--failure":
        bump_failure(filename)
        return 0
    if mode == '--detach':
        process = subprocess.Popen([sys.executable, __file__, '--run', filename],
                                   start_new_session=True, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        print(process.pid)
        return 0
    spec = json.loads(path.with_suffix('.spec.json').read_text())
    record = dict(status='running', job_id=spec['id'], task=spec['task'],
                  output=spec['output'], pid=os.getpid(),
                  started_ms=int(time.time() * 1000))
    write_record(path, record)
    module_spec = importlib.util.spec_from_file_location('bounded', spec['runner'])
    bounded = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(bounded)
    process = None
    rc = 1
    try:
        with open(spec['output'], 'wb') as output:
            process = subprocess.Popen(spec['argv'], start_new_session=True,
                                       stdout=output, stderr=subprocess.STDOUT)
            watch = bounded.watch_group(process)
            def interrupted(signum, _frame):
                raise bounded.ForwardedSignal(signum)
            signal.signal(signal.SIGTERM, interrupted)
            signal.signal(signal.SIGINT, interrupted)
            try:
                rc = process.wait(timeout=spec['timeout'])
            except subprocess.TimeoutExpired:
                rc = 124
            except bounded.ForwardedSignal as event:
                rc = 128 + event.signum
            finally:
                # Even successful leaders may leave background descendants.
                bounded.stop_group(process)
                bounded.finish_watch(watch)
        claims = [line.strip() for line in Path(spec['claims']).read_text().splitlines() if line.strip()]
        failed = check_claims(spec['verifier'], claims) if claims else []
        matched = bool(claims) and not failed and rc == 0
        record.update(status='ok' if matched else 'fail', matched=matched,
                      child_rc=rc, failed_claims=failed)
    except Exception as error:
        if process is not None:
            bounded.stop_group(process)
        record.update(status='error', matched=False, reason=str(error), child_rc=rc)
    if record["status"] != "ok":
        bump_failure(spec["failure_file"])
    write_record(path, record)
    return 0 if record['status'] == 'ok' else 1


if __name__ == '__main__':
    raise SystemExit(main())
