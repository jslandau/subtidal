import json
import sys
from pathlib import Path

import pytest

from fixture import clipped, digest, eligibility, identity, verify
from integrity import ENGLISH_SHA256, verify_file
from score import gates, normalize, score
from supervisor import run_trial


def row(start, end, speaker):
    return {'start': start, 'end': end, 'speaker': speaker}


def reference(segments):
    return {'duration': 10, 'segments': segments,
            'words': [{'start': 1, 'end': 2, 'speaker': 'A', 'text': 'Hello'},
                      {'start': 2, 'end': 3, 'speaker': 'A', 'text': 'world'}]}


def test_permutation_and_normalization():
    ref = reference([row(1, 4, 'A'), row(5, 9, 'B')])
    result = score(ref, {'segments': [row(1, 4, 'Y'), row(5, 9, 'X')], 'text': 'HELLO, world!'})
    assert result['der'] == 0
    assert result['wer'] == 0
    assert normalize("I’m  fine—yes.") == "i'm fine yes"


def test_overlap_scored_known_miss():
    result = score(reference([row(1, 9, 'A'), row(1, 9, 'B')]),
                   {'segments': [row(1, 9, 'X')], 'text': 'hello world'})
    assert result['der'] == pytest.approx(0.5)
    assert result['der_components']['missed detection'] > 0


def test_collar_known_answer():
    result = score(reference([row(1, 9, 'A')]),
                   {'segments': [row(1.1, 8.9, 'X')], 'text': 'hello world'})
    assert result['der'] == 0


def test_wer_known_answer():
    result = score(reference([row(1, 9, 'A')]), {'segments': [row(1, 9, 'X')], 'text': 'hello there'})
    assert result['wer'] == 0.5
    assert result['word_errors'] == {'substitutions': 1, 'deletions': 0, 'insertions': 0, 'hits': 1}
    assert score(reference([row(1, 9, 'A')]), {'segments': [], 'text': ''})['wer'] == 1


def test_invalid_interval_and_gates():
    with pytest.raises(ValueError):
        score(reference([row(1, 9, 'A')]), {'segments': [row(9, 11, 'X')], 'text': ''})
    assert gates({'der': .2, 'wer': .3}, {'der': .1, 'wer': .4}) == {'der_nonregression': True, 'wer_nonregression': False}
    with pytest.raises(ValueError):
        gates({'der': float('nan'), 'wer': .1}, {'der': .1, 'wer': .1})


def test_eligibility_and_clipping():
    rows = [row(i * 3, i * 3 + 2, 'ABCD'[i % 4]) for i in range(8)]
    assert eligibility(rows)['eligible']
    assert not eligibility(rows[:3])['eligible']
    assert clipped([row(-1, 3, 'A'), row(9, 11, 'B')], 10) == [row(0, 3, 'A'), row(9, 10, 'B')]
    # Duplicated same-speaker overlap must not double count active time.
    result = eligibility([row(0, 2, 'A'), row(1, 3, 'A')], 4)
    assert result['raw_speaker_seconds']['A'] == 3
    assert result['speaker_seconds']['A'] == pytest.approx(2.25)


def test_all_speaker_boundaries_excluded_and_overlap_retained():
    result = eligibility([row(0, 10, 'A'), row(3, 7, 'B')], 10)
    # A loses its edge collars and both B boundary collars; B's overlap scores.
    assert result['speaker_seconds'] == pytest.approx({'A': 9.25, 'B': 3.75})
    result = eligibility([row(0, 2, 'ABCD'[i % 4]) for i in range(4)], 3)
    assert not result['eligible']
    assert result['speaker_seconds'] == pytest.approx(dict.fromkeys('ABCD', 1.75))


@pytest.mark.parametrize('duration,start,end', [
    (0, 1, 2), (float('nan'), 1, 2), (120, -1, 2),
    (120, 1, 121), (120, 2, 1), (120, 1, float('nan')), (120, 1, 1)])
def test_verify_rejects_word_bounds_and_duration(tmp_path, duration, start, end):
    ref = {'duration': duration, 'segments': [],
           'words': [{'start': start, 'end': end, 'speaker': 'A', 'text': 'hello'}]}
    asset = tmp_path / 'reference.json'
    asset.write_text(json.dumps(ref))
    manifest = tmp_path / 'manifest.json'
    manifest.write_text(json.dumps({'schema_version': 1, 'eligibility': {'eligible': True},
                                   'files': {'reference.json': identity(asset)}}))
    # Valid accepted hash is insufficient: malformed timing is independently rejected.
    with pytest.raises(ValueError, match='invalid reference'):
        verify(manifest, tmp_path)


def test_frozen_manifest_scored_seconds():
    manifest = json.loads((Path(__file__).resolve().parents[1] / 'fixtures/ES2008a.manifest.json').read_text())
    assert manifest['duration'] == 240
    assert manifest['eligibility']['speaker_seconds'] == pytest.approx(
        {'A': 91.246, 'B': 73.754, 'C': 10.954, 'D': 3.759})
    assert manifest['eligibility']['eligible']
    assert not manifest['selection_attempts'][0]['eligible']
    assert manifest['eligibility_metadata_correction']['asset_bytes_changed'] is False


def test_digest_mismatch(tmp_path):
    p = tmp_path / 'model'
    p.write_bytes(b'hello')
    assert verify_file(p, digest(p), 5)['bytes'] == 5
    with pytest.raises(ValueError):
        verify_file(p, digest(p), 6)
    with pytest.raises(ValueError):
        verify_file(p, '0' * 64)


def test_production_pins_exact():
    source = Path(__file__).resolve().parents[3] / 'src/models/mod.rs'
    content = source.read_text()
    for expected in ENGLISH_SHA256.values():
        assert expected in content


@pytest.mark.parametrize('code,status,gate', [(0, 'ok', True), (7, 'nonzero', False)])
def test_supervisor_return_codes(tmp_path, code, status, gate):
    result = run_trial([sys.executable, '-c', f'print("captured"); raise SystemExit({code})'], tmp_path, 5)
    assert result['status'] == status and result['execution_gate'] == gate
    assert result['returncode'] == code
    assert 'captured' in (tmp_path / 'stdout.log').read_text()
    assert json.loads((tmp_path / 'execution.json').read_text()) == result
    with pytest.raises(FileExistsError):
        run_trial([sys.executable, '-c', 'pass'], tmp_path, 5)


def test_supervisor_timeout(tmp_path):
    result = run_trial([sys.executable, '-c', 'import time; time.sleep(30)'], tmp_path, .05)
    assert result['status'] == 'timeout' and not result['execution_gate']
    assert result['elapsed_seconds'] < 5


def test_supervisor_launch_error(tmp_path):
    result = run_trial(['/nonexistent/experiment-command'], tmp_path, 1)
    assert result['status'] == 'launch_error' and not result['execution_gate']


def test_sampler(tmp_path):
    class Sampler:
        def __init__(self, pid):
            assert pid > 0
        def start(self):
            return self
        def finish(self):
            return {'rss_peak_bytes': 123}
    result = run_trial([sys.executable, '-c', 'pass'], tmp_path, 5, Sampler)
    assert result['memory']['rss_peak_bytes'] == 123


def test_fixture_reuse_tamper(tmp_path):
    # Integrity failure occurs before metadata/scoring can consume changed bytes.
    asset = tmp_path / 'reference.json'
    asset.write_text('{}')
    manifest = tmp_path / 'manifest.json'
    manifest.write_text(json.dumps({'schema_version': 1, 'eligibility': {'eligible': True}, 'files': {'reference.json': identity(asset)}}))
    asset.write_text('{"tampered":true}')
    with pytest.raises(ValueError, match='integrity mismatch'):
        verify(manifest, tmp_path)
