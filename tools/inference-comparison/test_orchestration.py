"""Unit tests for inference-comparison orchestration without running inference."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import memory
import run_trials


class TrialTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.child = self.root / 'fake_child.py'
        self.child.write_text(
            "import json, os, pathlib, sys, time\n"
            "mode = os.environ['FAKE_CHILD_MODE']\n"
            "if mode == 'timeout':\n"
            "    time.sleep(30)\n"
            "elif mode == 'nonzero':\n"
            "    print('fake child failure', file=sys.stderr)\n"
            "    raise SystemExit(17)\n"
            "elif mode == 'json':\n"
            "    report = {'schema_version': 1, 'real_samples': 16000, "
            "'duration_seconds': 1.0, 'compute_seconds': 0.5, 'rtf': 0.5, "
            "'transcript': 'hello', 'segments': [], 'ticks': [], "
            "'runner': 'fake-runner', 'provider': 'cpu', 'mode': 'asr', "
            "'warmup_runs': 1, 'load_seconds': 0.1, 'warmup_seconds': 0.2}\n"
            "    pathlib.Path(os.environ['FAKE_RESULT_PATH']).write_text(json.dumps(report))\n"
            "elif mode == 'bad-schema':\n"
            "    pathlib.Path(os.environ['FAKE_RESULT_PATH']).write_text("
            "json.dumps({'schema_version': 1, 'real_samples': 16000}))\n"
            "elif mode == 'malformed-json':\n"
            "    pathlib.Path(os.environ['FAKE_RESULT_PATH']).write_text('{not valid json')\n"
            "elif mode != 'missing-json':\n"
            "    raise SystemExit('unknown test mode')\n"
        )

    def run_trial(self, mode, timeout=5):
        trial_dir = self.root / f'trial-{mode}'
        env = dict(os.environ)
        env.update(FAKE_CHILD_MODE=mode,
                   FAKE_RESULT_PATH=str(trial_dir / 'result.json'))
        with mock.patch.object(run_trials, 'MemorySampler', _NoopSampler):
            record = run_trials.trial(
                [sys.executable, str(self.child)], trial_dir, timeout, env)
        return record, trial_dir

    def test_timeout_is_recorded(self):
        record, _ = self.run_trial('timeout', timeout=0.2)
        self.assertEqual(record['status'], 'timeout')
        self.assertNotEqual(record['exit_code'], 0)

    def test_nonzero_exit_is_failure(self):
        record, trial_dir = self.run_trial('nonzero')
        self.assertEqual(record['status'], 'failed')
        self.assertEqual(record['exit_code'], 17)
        self.assertIn('fake child failure', (trial_dir / 'stderr.log').read_text())

    def test_successful_json_report_is_loaded(self):
        record, _ = self.run_trial('json')
        self.assertEqual(record['status'], 'pass')
        self.assertEqual(record['result'], {
            'schema_version': 1, 'real_samples': 16000,
            'duration_seconds': 1.0, 'compute_seconds': 0.5, 'rtf': 0.5,
            'transcript': 'hello', 'segments': [], 'ticks': [],
            'runner': 'fake-runner', 'provider': 'cpu', 'mode': 'asr',
            'warmup_runs': 1, 'load_seconds': 0.1, 'warmup_seconds': 0.2,
        })

    def test_malformed_json_report_turns_success_into_failure(self):
        record, _ = self.run_trial('malformed-json')
        self.assertEqual(record['exit_code'], 0)
        self.assertEqual(record['status'], 'failed')
        self.assertIn('missing/invalid runner report', record['error'])

    def test_schema_mismatch_turns_success_into_failure(self):
        record, _ = self.run_trial('bad-schema')
        self.assertEqual(record['exit_code'], 0)
        self.assertEqual(record['status'], 'failed')
        self.assertIn('runner report schema mismatch', record['error'])

    def test_missing_json_report_turns_success_into_failure(self):
        record, _ = self.run_trial('missing-json')
        self.assertEqual(record['exit_code'], 0)
        self.assertEqual(record['status'], 'failed')
        self.assertIn('missing/invalid runner report', record['error'])


class _NoopSampler:
    """Keep trial tests independent of host memory/GPU utilities."""

    def __init__(self, pid):
        self.pid = pid

    def start(self):
        return self

    def finish(self):
        return {'peak_rss_kib': None, 'peak_child_gpu_mib': None,
                'samples': [], 'errors': [], 'limitations': 'test sampler'}


class MemorySamplerTests(unittest.TestCase):
    def test_unavailable_pid_is_reported_without_claiming_zero_memory(self):
        sampler = memory.MemorySampler(2**31 - 1, interval=0.01)
        sampled = threading.Event()

        def unavailable_gpu_query(*args, **kwargs):
            sampled.set()
            raise FileNotFoundError('nvidia-smi unavailable')

        with mock.patch.object(memory.subprocess, 'run', side_effect=unavailable_gpu_query):
            sampler.start()
            self.assertTrue(sampled.wait(timeout=2), 'sampler did not attempt GPU query')
            result = sampler.finish()

        self.assertIsNone(result['peak_rss_kib'])
        self.assertIsNone(result['peak_child_gpu_mib'])
        self.assertTrue(result['samples'])
        self.assertIn('RSS unavailable: FileNotFoundError', result['errors'])
        self.assertIn('GPU allocation unavailable: FileNotFoundError', result['errors'])
        self.assertIn('absent rows are unknown, not zero', result['limitations'])


class ScriptSyntaxTests(unittest.TestCase):
    def test_build_and_isolation_scripts_parse(self):
        for name in ('build.py', 'check_isolation.py'):
            path = HERE / name
            with self.subTest(script=name):
                compile(path.read_text(), str(path), 'exec')


if __name__ == '__main__':
    unittest.main()
