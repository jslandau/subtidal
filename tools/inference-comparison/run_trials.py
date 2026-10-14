#!/usr/bin/env python3
"""Run isolated children and archive every failure, output, and memory sample."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time
from memory import MemorySampler

BASE = Path(__file__).resolve().parent

def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()

def trial(command, directory, timeout, env):
    directory.mkdir(parents=True, exist_ok=True)
    record = {'command': command, 'timeout_seconds': timeout, 'status': 'failed',
              'cache_state': 'unknown; user caches preserved', 'started_unix': time.time()}
    start = time.monotonic()
    with (directory / 'stdout.log').open('w') as out, (directory / 'stderr.log').open('w') as err:
        try:
            child = subprocess.Popen(command, stdout=out, stderr=err, env=env, start_new_session=True)
            sampler = MemorySampler(child.pid).start()
            try:
                record['exit_code'] = child.wait(timeout=timeout)
                record['status'] = 'pass' if child.returncode == 0 else 'failed'
            except subprocess.TimeoutExpired:
                import signal
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
                record.update(status='timeout', exit_code=child.returncode)
            record['memory'] = sampler.finish()
        except OSError as error:
            record['error'] = str(error)
    record['wall_seconds'] = time.monotonic() - start
    result_path = directory / 'result.json'
    if record['status'] == 'pass':
        try:
            result = json.loads(result_path.read_text())
            required = {'schema_version', 'real_samples', 'duration_seconds', 'compute_seconds',
                        'rtf', 'transcript', 'segments', 'ticks', 'runner', 'provider', 'mode',
                        'warmup_runs', 'load_seconds', 'warmup_seconds'}
            if not isinstance(result, dict) or not required.issubset(result) or result['schema_version'] != 1:
                raise ValueError('runner report schema mismatch')
            import math
            for key in ('duration_seconds', 'compute_seconds', 'rtf', 'load_seconds', 'warmup_seconds'):
                if not isinstance(result[key], (int, float)) or not math.isfinite(result[key]) or result[key] < 0:
                    raise ValueError('invalid numeric field: ' + key)
            if not isinstance(result['real_samples'], int) or result['real_samples'] <= 0:
                raise ValueError('invalid real sample count')
            if not math.isclose(result['duration_seconds'], result['real_samples']/16000, rel_tol=1e-9):
                raise ValueError('duration/sample count mismatch')
            if not math.isclose(result['rtf'], result['compute_seconds']/result['duration_seconds'], rel_tol=1e-9):
                raise ValueError('RTF timing contract mismatch')
            if not isinstance(result['segments'], list) or not isinstance(result['ticks'], list) or not isinstance(result['transcript'], str):
                raise ValueError('invalid output shapes')
            record['result'] = result
        except (OSError, ValueError) as error:
            record.update(status='failed', error='missing/invalid runner report: ' + str(error))
    (directory / 'trial.json').write_text(json.dumps(record, indent=2) + '\n')
    return record

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--fixture-manifest', type=Path, required=True)
    parser.add_argument('--model-dir', type=Path, required=True)
    parser.add_argument('--diar-v2', type=Path, required=True)
    parser.add_argument('--diar-v3', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repetitions', type=int, default=5)
    parser.add_argument('--timeout', type=float, default=1200)
    parser.add_argument('--runner', action='append', choices=['runner-baseline','runner-ort-v3','runner-burn'])
    parser.add_argument('--provider', action='append', choices=['cpu','cuda'])
    parser.add_argument('--mode', action='append', choices=['asr','diar','combined'])
    parser.add_argument('--warmup', type=int, default=1)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error('repetitions must be positive')
    import sys
    sys.path.insert(0, str(BASE / 'python'))
    from fixture import verify
    from integrity import verify_models
    fixture_manifest = verify(args.fixture_manifest, args.fixture.parent)
    if args.fixture.name != 'fixture.wav':
        parser.error('fixture must be the frozen fixture.wav')
    models = verify_models(args.model_dir, args.diar_v2, args.diar_v3)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / 'provenance.json').write_text(json.dumps({'fixture': fixture_manifest, 'models': models}, indent=2) + '\n')
    records = []
    for name in args.runner or ['runner-baseline','runner-ort-v3','runner-burn']:
        install = BASE / name / 'target' / 'install'
        artifacts = json.loads((install / 'artifacts.json').read_text())
        binary = install / artifacts['binary']['name']
        if digest(binary) != artifacts['binary']['sha256']:
            raise ValueError('staged binary identity mismatch')
        for sidecar in artifacts['ort_sidecars']:
            if digest(install / sidecar['name']) != sidecar['sha256']:
                raise ValueError('staged sidecar identity mismatch')
        env = dict(os.environ, LD_LIBRARY_PATH=str(install))
        env.pop('ORT_DYLIB_PATH', None)
        for provider in args.provider or ['cpu','cuda']:
            for mode in args.mode or ['asr','diar','combined']:
                for repetition in range(args.repetitions):
                    directory = args.output / f'{name}-{provider}-{mode}-{repetition+1}'
                    if directory.exists():
                        raise FileExistsError('Refusing to overwrite archived trial: ' + str(directory))
                    command = [str(binary), '--model-dir', str(args.model_dir), '--diar-model',
                               str(args.diar_v2 if name == 'runner-baseline' else args.diar_v3),
                               '--fixture', str(args.fixture), '--provider', provider, '--mode', mode,
                               '--output', str(directory / 'result.json'), '--warmup', str(args.warmup)]
                    record = trial(command, directory, args.timeout, env)
                    record.update(runner=name, provider=provider, mode=mode, repetition=repetition+1,
                                  fixture_sha256=digest(args.fixture),
                                  lock_sha256=digest(BASE / name / 'Cargo.lock'),
                                  artifacts=artifacts, loader_path=str(install))
                    (directory / 'trial.json').write_text(json.dumps(record, indent=2) + '\n')
                    records.append(record)
                    (args.output / 'trials.json').write_text(json.dumps(records, indent=2) + '\n')
                    print(name, provider, mode, repetition+1, record['status'], flush=True)
    return 0 if all(r['status'] == 'pass' for r in records) else 1

if __name__ == '__main__':
    raise SystemExit(main())
