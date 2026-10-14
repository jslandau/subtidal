#!/usr/bin/env python3
"""Validate independent resolution roots and unchanged production inference inputs."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
BASE = Path(__file__).resolve().parent
PRODUCTION = {
    'Cargo.toml': 'e4b13dd306e0278291d732d579b7630edac92f2d0ccb1d86843201568a0bca48',
    'Cargo.lock': '3317987907d693b2e7b70ff369519311a21a49456a8d7c71ec17b79355b25621',
    'src/models/mod.rs': '11671c712eb5ea9611fb0629aae12e958aa73cc1132e16b6091655a21fdd5b65',
    'src/stt/nemotron.rs': '8033cfb4931f2a42502e5ad4f1e54219d5737f847a0cf626e9d1ca15373bab72',
    'src/stt/diarization.rs': '46a19f0da86a96256095b03c8e0cd5670e860773ceebd0e7cf37858591ac3690',
}

def metadata(name):
    directory = BASE / name
    env = dict(os.environ, CARGO_TARGET_DIR=str(directory / 'target'))
    return json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--format-version', '1',
        '--manifest-path', str(directory / 'Cargo.toml')], env=env))

class IsolationTests(unittest.TestCase):
    def test_comparison_dependency_isolation(self):
        for path, digest in PRODUCTION.items():
            self.assertEqual(hashlib.sha256((ROOT / path).read_bytes()).hexdigest(), digest, path)
        roots = set()
        for name, parakeet, ort in [('runner-baseline', '0.3.6', '2.0.0-rc.12'),
                                    ('runner-ort-v3', '0.4.1', '2.0.0-rc.14'),
                                    ('runner-burn', '0.4.1', None)]:
            data = metadata(name)
            self.assertEqual(Path(data['workspace_root']), BASE / name)
            self.assertTrue((BASE / name / 'Cargo.lock').is_file())
            roots.add(data['workspace_root'])
            packages = data['packages']
            self.assertEqual({p['version'] for p in packages if p['name'] == 'parakeet-rs'}, {parakeet})
            self.assertEqual({p['version'] for p in packages if p['name'] == 'ort'}, {ort} if ort else set())
            if ort is None:
                self.assertFalse(any(p['name'] in ('ort', 'ort-sys') for p in packages))
        self.assertEqual(len(roots), 3)

if __name__ == '__main__':
    unittest.main()
