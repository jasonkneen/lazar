#!/usr/bin/env python3
"""Offline regressions for process ownership and outcome scoring."""
from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import signal
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
EVALS = ROOT / 'src/seed-skills/harness-evals'
spec = importlib.util.spec_from_file_location('score_stream', EVALS / 'lib/score_stream.py')
score = importlib.util.module_from_spec(spec)
spec.loader.exec_module(score)


def wait_for(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.02)
    raise AssertionError('condition did not become true before deadline')


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


class ReviewRegressions(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='lazar-review-')
        self.root = Path(self.temporary.name)

    def tearDown(self):
        for directory, dirs, files in os.walk(self.root):
            os.chmod(directory, 0o700)
            for name in files:
                path = Path(directory) / name
                if not path.is_symlink(): path.chmod(0o600)
        self.temporary.cleanup()

    def test_invalid_guardrails_fails_before_build_or_binary_mutation(self):
        (self.root / 'src').mkdir()
        (self.root / 'bin').mkdir()
        binary = self.root / 'bin/lazar'
        binary.write_text('original binary')
        (self.root / '.guardrails').write_text('invalid')
        result = subprocess.run(['bash', str(ROOT / 'scripts/kernel-build.sh')],
                                env=dict(os.environ, LAZAR_HOME=str(self.root)),
                                capture_output=True, text=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('guardrails build mode', result.stderr)
        self.assertEqual(binary.read_text(), 'original binary')

    def test_fresh_source_update_creates_root_and_writable_runtime_skills(self):
        clone = self.root / 'clone'
        install = self.root / 'install'
        for directory in ('src/seed-skills/provider-failover', 'src/seed-hooks', 'scripts', 'tui'):
            (clone / directory).mkdir(parents=True)
        shutil.copy(ROOT / 'setup.sh', clone / 'setup.sh')
        for filename in ('lazar-spawn.sh', 'lazar-failover.sh', 'run-with-timeout.py', 'spawn-supervisor.py'):
            shutil.copy(ROOT / 'scripts' / filename, clone / 'scripts' / filename)
        for filename in ('src/Cargo.toml', 'src/providers.toml', 'src/seed-skills/INDEX.md', 'src/seed-skills/provider-failover/SKILL.md'):
            (clone / filename).write_text('fixture')
        (clone / 'scripts/kernel-build.sh').write_text('#!/bin/bash\nmkdir -p "$LAZAR_HOME/bin"\nprintf fixture > "$LAZAR_HOME/bin/lazar"\nchmod -R a-w "$LAZAR_HOME/src"\n')
        (clone / 'scripts/tui-build.sh').write_text('#!/bin/bash\nprintf fixture > "$LAZAR_HOME/bin/lazartui"\n')
        env = dict(os.environ, LAZAR_HOME=str(install), LAZAR_UPDATE_SOURCE='1')
        result = subprocess.run(['bash', 'setup.sh'], cwd=clone, env=env, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(os.access(install / 'skills/INDEX.md', os.W_OK))
        self.assertFalse(os.access(install / 'src/seed-skills/INDEX.md', os.W_OK))
        self.assertEqual((install / '.guardrails').read_text().strip(), 'on')

    def test_failover_sanitizes_session_model_and_stops_after_hooks(self):
        stub = self.root / 'agent'
        stub.write_text('''#!/bin/bash
[ -z "${LAZAR_SESSION_ID:-}" ] || exit 41
[ -z "${LAZAR_MODEL:-}" ] || exit 42
echo attempt >> "$ATTEMPTS"
printf '{"type":"hook_start"}\\n'
exit 7
''')
        stub.chmod(0o755)
        prompt = self.root / 'prompt'
        prompt.write_text('do a task')
        attempts = self.root / 'attempts'
        env = dict(os.environ, LAZAR_BIN=str(stub), LAZAR_SESSION_ID='old-session',
                   LAZAR_MODEL='old-model', ATTEMPTS=str(attempts))
        result = subprocess.run(['bash', str(ROOT / 'scripts/lazar-failover.sh'), str(prompt), 'one', 'two'],
                                env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertEqual(attempts.read_text().splitlines(), ['attempt'])

    def test_final_invocation_must_complete(self):
        self.assertFalse(score.terminal_state([
            {'type': 'invoke_start'}, {'type': 'invoke_end', 'stop_reason': 'end_turn'},
            {'type': 'invoke_start'}, {'type': 'text_delta', 'text': 'unfinished'},
        ])[0])

    def test_message_stop_requires_success_reason(self):
        for reason in ('tool_use', 'max_tokens', ''):
            with self.subTest(reason=reason):
                self.assertFalse(score.terminal_state([
                    {'type': 'message_start'},
                    {'type': 'message_delta', 'delta': {'stop_reason': reason}},
                    {'type': 'message_stop'},
                ])[1])
        self.assertTrue(score.terminal_state([
            {'type': 'message_start'},
            {'type': 'message_delta', 'delta': {'stop_reason': 'end_turn'}},
            {'type': 'message_stop'},
        ])[1])

    def test_runner_rejects_crashed_adapter_despite_all_stream_checks_passing(self):
        adapter = self.root / 'adapter.py'
        adapter.write_text('import json,sys\nfor e in [{"type":"invoke_start"},{"type":"text_delta","text":sys.argv[1]},{"type":"invoke_end","stop_reason":"end_turn"}]: print(json.dumps(e))\nsys.exit(23)\n')
        results = self.root / 'eval-results'
        command = f'{shlex.quote(sys.executable)} {shlex.quote(str(adapter))} {{{{prompt}}}}'
        result = subprocess.run(['bash', str(EVALS / 'bin/harness-evals'), 'run', '--suite', 'core', '--task', 'core.chat', '--adapter', 'command', '--cmd', command],
                                env=dict(os.environ, HARNESS_EVALS_RESULTS=str(results)), capture_output=True, text=True, timeout=15)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        record = json.loads(next(results.rglob('score.json')).read_text())
        self.assertTrue(all(check['ok'] for check in record['checks']), record)
        self.assertEqual(record['status'], 'error')
        self.assertEqual(record['adapter_exit_code'], 23)

    def test_adapter_exit_overrides_successful_stream(self):
        task = {'checks': [{'type': 'stream_end_ok'}]}
        events = [{'type': 'invoke_end', 'stop_reason': 'end_turn'}]
        for rc in (23, 124):
            result = score.score_task(task, events, {}, adapter_exit_code=rc)
            self.assertFalse(result['passed'])
            self.assertEqual(result['status'], 'error')

    def test_exact_file_checks_use_real_newlines(self):
        for suite in ('core', 'outcomes'):
            for line in (EVALS / 'tasks' / suite / 'tasks.jsonl').read_text().splitlines():
                task = json.loads(line)
                for check in task['checks']:
                    if check['type'] == 'fs_exact':
                        context = {'workdir': str(self.root), 'token': 'TEST'}
                        expanded = score.expanded_check(check, context)
                        self.assertIn('\n', expanded['content'])
                        Path(expanded['path']).write_text(expanded['content'])
                        self.assertTrue(score.run_check(check, [], context)[0])

    def stubborn_command(self, pidfile):
        child = "import os,signal,time,pathlib; signal.signal(signal.SIGTERM,signal.SIG_IGN); pathlib.Path(%r).write_text(str(os.getpid())); time.sleep(60)" % str(pidfile)
        return [sys.executable, '-c', 'import subprocess,time; subprocess.Popen(%r); time.sleep(60)' % [sys.executable, '-c', child]]

    def test_timeout_kills_descendant_after_leader_exits(self):
        for helper in (ROOT / 'scripts/run-with-timeout.py', EVALS / 'lib/run_with_timeout.py'):
            with self.subTest(helper=str(helper)):
                pidfile = self.root / 'pid'
                pidfile.unlink(missing_ok=True)
                argv = [sys.executable, str(helper), '--timeout', '.3']
                if helper.name == 'run_with_timeout.py':
                    argv += ['--stdout', str(self.root / 'out'), '--stderr', str(self.root / 'err')]
                result = subprocess.run(argv + ['--'] + self.stubborn_command(pidfile), timeout=6)
                self.assertEqual(result.returncode, 124)
                pid = int(pidfile.read_text())
                try:
                    wait_for(lambda: not alive(pid))
                finally:
                    if alive(pid):
                        os.kill(pid, signal.SIGKILL)

    def test_runner_sigkill_still_cleans_child_group(self):
        pidfile = self.root / 'pid'
        process = subprocess.Popen([sys.executable, str(ROOT / 'scripts/run-with-timeout.py'),
                                    '--timeout', '60', '--'] + self.stubborn_command(pidfile))
        try:
            wait_for(pidfile.exists)
            pid = int(pidfile.read_text())
            process.kill()
            process.wait(timeout=3)
            wait_for(lambda: not alive(pid))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if pidfile.exists() and alive(int(pidfile.read_text())):
                os.kill(int(pidfile.read_text()), signal.SIGKILL)

    def spawn_env(self):
        stub = self.root / 'fake-lazar'
        stub.write_text('#!/bin/bash\nsleep .2\nprintf done > "$SPAWN_ARTIFACT"\nexit "${SPAWN_RC:-0}"\n')
        stub.chmod(0o755)
        env = dict(os.environ, LAZAR_HOME=str(self.root), LAZAR_BIN=str(stub),
                   SPAWN_ARTIFACT=str(self.root / 'artifact'), LAZAR_SPAWN_TIMEOUT='2')
        return env

    def spawn_prompt(self, claims=True):
        path = self.root / 'prompt'
        path.write_text(f'Create {self.root}/artifact\nSUCCESS: file present\nEXPECT-VERIFY:\n' +
                        (f'  contains={self.root}/artifact:done\n' if claims else ''))
        return path

    def test_parallel_spawns_respect_one_atomic_session_budget(self):
        env = self.spawn_env()
        env.update(LAZAR_SESSION_ID='shared-test', LAZAR_SPAWN_BUDGET='3')
        prompt = self.spawn_prompt()
        children = [subprocess.Popen(['bash', str(ROOT / 'scripts/lazar-spawn.sh'), f'job-{number}', str(prompt)],
                                     env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE) for number in range(8)]
        for child in children:
            child.communicate(timeout=15)
        self.assertEqual(sum(child.returncode == 0 for child in children), 3)
        self.assertEqual((self.root / 'workspace/subagent/.session-shared-test-spawns').read_text(), '3')

    def test_empty_claims_refused_before_detached_execution(self):
        result = subprocess.run(['bash', str(ROOT / 'scripts/lazar-spawn.sh'), '--detach', 'empty', str(self.spawn_prompt(False))],
                                env=self.spawn_env(), capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'artifact').exists())
        self.assertFalse((self.root / 'workspace').exists())

    def test_detached_job_survives_call_group_and_records_result(self):
        env = self.spawn_env()
        wrapper = subprocess.Popen(['bash', str(ROOT / 'scripts/lazar-spawn.sh'), '--detach', 'job', str(self.spawn_prompt())],
                                   env=env, stdout=subprocess.PIPE, start_new_session=True, text=True)
        output, _ = wrapper.communicate(timeout=5)
        self.assertEqual(wrapper.returncode, 0, output)
        envelope = json.loads(output)
        try:
            os.killpg(wrapper.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        job = self.root / 'workspace/subagent/jobs' / (envelope['job_id'] + '.json')
        wait_for(lambda: json.loads(job.read_text())['status'] in ('ok', 'fail', 'error'))
        record = json.loads(job.read_text())
        self.assertEqual(record['status'], 'ok', record)
        self.assertEqual(record['child_rc'], 0)

    def test_detached_nonzero_exit_cannot_pass_matching_claims(self):
        env = self.spawn_env()
        env['SPAWN_RC'] = '23'
        result = subprocess.run(['bash', str(ROOT / 'scripts/lazar-spawn.sh'), '--detach', 'job', str(self.spawn_prompt())],
                                env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        job = self.root / 'workspace/subagent/jobs' / (json.loads(result.stdout)['job_id'] + '.json')
        wait_for(lambda: json.loads(job.read_text())['status'] in ('ok', 'fail', 'error'))
        record = json.loads(job.read_text())
        self.assertEqual(record['status'], 'fail', record)
        self.assertEqual(record['child_rc'], 23)


if __name__ == '__main__':
    unittest.main(verbosity=2)
