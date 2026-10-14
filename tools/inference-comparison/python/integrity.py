"""Experiment-only pins. Never substitute v3 for the production v2.1 file."""
from pathlib import Path
from fixture import identity

ENGLISH_REVISION = '12bca7934f692a845057af06c5861d5e99495318'
ENGLISH_SHA256 = {
    'encoder.onnx': '4c3acc1fe273b710b818012919a53ac0d535b5cbb614eb65e240ddd9480df27b',
    'encoder.onnx.data': '2c43a65deb934ad505bf3a5c9f2e47bc59ed43daf8a806c64597e2030fcc3278',
    'decoder_joint.onnx': '3111f21cc147631e6038422239d974547c80915b8a7e5139a67fa5bed63d4a31',
    'tokenizer.model': '07d4e5a63840a53ab2d4d106d2874768143fb3fbdd47938b3910d2da05bfb0a9',
}
V21_SHA256 = 'f010a62654fd6d2adb70166265df2e53f5a766e354d2cce152b4c7966a04d02d'
V3_REVISION = '52f2deee40f20d7dc1f459416e9cd213fb9472df'
V3_SHA256 = '915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a'
V3_BYTES = 400506656


def verify_file(path, sha256, size=None):
    actual = identity(path)
    if actual['sha256'] != sha256 or (size is not None and actual['bytes'] != size):
        raise ValueError('model integrity mismatch: ' + str(path))
    return {'path': str(Path(path).resolve()), **actual}


def verify_models(english_dir, v21, v3):
    return {'english_revision': ENGLISH_REVISION, 'v21_revision': ENGLISH_REVISION,
            'v3_revision': V3_REVISION,
            'english': {name: verify_file(Path(english_dir) / name, sha) for name, sha in ENGLISH_SHA256.items()},
            'v21': verify_file(v21, V21_SHA256), 'v3': verify_file(v3, V3_SHA256, V3_BYTES)}
