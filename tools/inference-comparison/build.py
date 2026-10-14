#!/usr/bin/env python3
"""Build/test each independent resolution root, archive output and linked dependencies."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

BASE = Path(__file__).resolve().parent
RUNNERS = ('runner-baseline', 'runner-ort-v3', 'runner-burn')


def run(command, env, log):
    start = time.monotonic()
    with log.open('w') as output:
        child = subprocess.run(command, env=env, stdout=output, stderr=subprocess.STDOUT)
    return {'command': command, 'exit_code': child.returncode,
            'seconds': time.monotonic() - start, 'log': str(log)}


def file_details(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return {'sha256': digest.hexdigest(), 'size_bytes': path.stat().st_size}


def inspect_binary(binary):
    checks = {}
    for label, command in (
        ('ldd', ['ldd', str(binary)]),
        ('readelf', ['readelf', '-d', str(binary)]),
    ):
        try:
            child = subprocess.run(command, capture_output=True, text=True, check=False)
            checks[label] = {
                'command': command,
                'exit_code': child.returncode,
                'stdout': child.stdout,
                'stderr': child.stderr,
            }
        except OSError as error:
            checks[label] = {
                'command': command,
                'exit_code': None,
                'stdout': '',
                'stderr': str(error),
            }

    needed = re.findall(
        r'\(NEEDED\).*?Shared library: \[(.*?)\]', checks['readelf']['stdout'])
    ldd_dependencies = []
    for line in checks['ldd']['stdout'].splitlines():
        match = re.match(r'\s*(\S+)\s+=>\s+(\S+)(?:\s+\(([^)]+)\))?', line)
        if match:
            ldd_dependencies.append({
                'name': match.group(1),
                'path': match.group(2),
                'address': match.group(3),
                'not_found': match.group(2) == 'not',
            })
        else:
            match = re.match(r'\s*(\S+)\s+\(([^)]+)\)', line)
            if match:
                ldd_dependencies.append({
                    'name': match.group(1),
                    'path': None,
                    'address': match.group(2),
                    'not_found': False,
                })
    return checks, needed, ldd_dependencies


def copy_preserving_symlink(source, destination):
    if source.is_symlink():
        destination.symlink_to(os.readlink(source))
    else:
        shutil.copy2(source, destination)


def stage_artifacts(name, release, binary, include_ort_sidecars):
    install = BASE / name / 'target' / 'install'
    if install.is_symlink():
        install.unlink()
    elif install.exists():
        shutil.rmtree(install)
    install.mkdir(parents=True)

    installed_binary = install / binary.name
    shutil.copy2(binary, installed_binary)

    sidecars = []
    ort_libraries = sorted(release.glob('libonnxruntime*.so*')) if include_ort_sidecars else []
    for source in ort_libraries:
        if not (source.is_file() or source.is_symlink()):
            continue
        destination = install / source.name
        copy_preserving_symlink(source, destination)
        sidecar = {
            'name': source.name,
            'source_path': str(source.relative_to(BASE)),
            'install_path': str(destination.relative_to(BASE)),
            **file_details(source),
            'is_symlink': source.is_symlink(),
        }
        if source.is_symlink():
            sidecar['symlink_target'] = os.readlink(source)
        sidecars.append(sidecar)

    dependency_checks, linked_dependencies, ldd_dependencies = inspect_binary(binary)
    manifest = {
        'schema_version': 1,
        'runner': name,
        'binary': {
            'name': binary.name,
            'source_path': str(binary.relative_to(BASE)),
            'install_path': str(installed_binary.relative_to(BASE)),
            **file_details(binary),
        },
        'dependencies': dependency_checks,
        'linked_dependencies': linked_dependencies,
        'ldd_dependencies': ldd_dependencies,
        'ort_sidecars': sidecars,
    }
    manifest_path = install / 'artifacts.json'
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest, manifest_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--runner', choices=RUNNERS, action='append')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []
    for name in args.runner or RUNNERS:
        directory = BASE / name
        env = dict(os.environ, CARGO_TARGET_DIR=str(directory / 'target'))
        record = {'runner': name, 'commands': []}
        for action in ('test', 'build'):
            command = ['cargo'] + (['+1.95.0'] if name == 'runner-burn' else []) + [action, '--locked', '--manifest-path', str(directory / 'Cargo.toml')]
            if action == 'build':
                command.append('--release')
            result = run(command, env, args.output / f'{name}-{action}.log')
            record['commands'].append(result)
            if result['exit_code']:
                break
        record['status'] = 'pass' if all(x['exit_code'] == 0 for x in record['commands']) and len(record['commands']) == 2 else 'failed'
        if record['status'] == 'pass':
            try:
                release = directory / 'target' / 'release'
                manifest_data = __import__('tomllib').loads((directory / 'Cargo.toml').read_text())
                binary = release / manifest_data['package']['name']
                manifest, manifest_path = stage_artifacts(
                    name, release, binary, include_ort_sidecars=name in ('runner-baseline', 'runner-ort-v3'))
                output_manifest = args.output / f'{name}-artifacts.json'
                output_manifest.write_text(json.dumps(manifest, indent=2) + '\n')
                record['artifacts'] = {
                    'install_manifest': str(manifest_path),
                    'output_manifest': str(output_manifest),
                }
            except (OSError, ValueError, KeyError) as error:
                record['status'] = 'failed'
                record['artifact_error'] = str(error)
        results.append(record)
        (args.output / 'builds.json').write_text(json.dumps(results, indent=2) + '\n')
    return 0 if all(r['status'] == 'pass' for r in results) else 1


if __name__ == '__main__':
    raise SystemExit(main())
