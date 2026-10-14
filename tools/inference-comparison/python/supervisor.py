"""One fresh process per trial; timeout/nonzero exits never pass the execution gate."""
import argparse
import json
import os
import signal
import subprocess
import time
from pathlib import Path


def run_trial(command, output_dir, timeout_seconds, sampler_factory=None):
    if timeout_seconds <= 0:
        raise ValueError('timeout must be positive')
    root = Path(output_dir)
    root.mkdir(parents=True, exist_ok=True)
    report_path = root / 'execution.json'
    if report_path.exists():
        raise FileExistsError('refusing to overwrite trial report')
    started = time.monotonic()
    status, code, memory = 'launch_error', None, None
    error = None
    with (root / 'stdout.log').open('wb') as stdout, (root / 'stderr.log').open('wb') as stderr:
        try:
            child = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True)
        except OSError as exc:
            error = str(exc)
        else:
            sampler = None
            try:
                if sampler_factory:
                    sampler = sampler_factory(child.pid)
                    sampler.start()
                try:
                    code = child.wait(timeout=timeout_seconds)
                    status = 'ok' if code == 0 else 'nonzero'
                except subprocess.TimeoutExpired:
                    status = 'timeout'
                    os.killpg(child.pid, signal.SIGKILL)
                    code = child.wait()
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
                if sampler:
                    memory = sampler.finish()
    report = {'schema_version': 1, 'command': command, 'status': status, 'returncode': code,
              'elapsed_seconds': time.monotonic() - started, 'timeout_seconds': timeout_seconds,
              'execution_gate': status == 'ok', 'memory': memory, 'error': error}
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    return report


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', required=True, type=Path)
    p.add_argument('--timeout', required=True, type=float)
    p.add_argument('command', nargs=argparse.REMAINDER)
    args = p.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        p.error('command required after --')
    result = run_trial(command, args.output, args.timeout)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['execution_gate'] else 1)
