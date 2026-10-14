"""Prepare and verify the approved AMI fixture before any inference.

Large source/derived audio stays in an explicitly supplied external directory.
"""
import argparse
import hashlib
import json
import math
import subprocess
import wave
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

AUDIO_URL = 'https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/ES2008a/audio/ES2008a.Mix-Headset.wav'
ANNOTATION_URL = 'https://groups.inf.ed.ac.uk/ami/AMICorpusAnnotations/ami_public_manual_1.6.2.zip'


def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def identity(path):
    p = Path(path)
    return {'sha256': digest(p), 'bytes': p.stat().st_size}


def clipped(rows, duration):
    return [dict(r, start=max(0.0, r['start']), end=min(duration, r['end']))
            for r in rows if r['start'] < duration and r['end'] > 0 and r['end'] > r['start']]


def validate_reference(ref):
    duration = ref['duration']
    if not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration <= 0:
        raise ValueError('invalid reference duration')
    for kind in ('segments', 'words'):
        for r in ref[kind]:
            if not all(isinstance(r[k], (int, float)) and math.isfinite(r[k]) for k in ('start', 'end')) or not 0 <= r['start'] < r['end'] <= duration:
                raise ValueError('invalid reference ' + kind + ' interval')
            if not isinstance(r['speaker'], str) or not r['speaker']:
                raise ValueError('invalid reference speaker')
            if kind == 'words' and (not isinstance(r['text'], str) or not r['text'].strip()):
                raise ValueError('invalid reference word text')


def eligibility(segments, duration=None):
    from pyannote.core import Annotation, Segment, Timeline
    from pyannote.metrics.diarization import DiarizationErrorRate
    if duration is None:
        duration = max((r['end'] for r in segments), default=1.0)
    validate_reference({'duration': duration, 'segments': segments, 'words': []})
    reference = Annotation(uri='ES2008a')
    for i, r in enumerate(segments):
        reference[Segment(r['start'], r['end']), i] = r['speaker']
    # Use the metric's exact global UEM extrusion: every reference boundary
    # excludes all speakers, and overlap remains scored.
    metric = DiarizationErrorRate(collar=0.25, skip_overlap=False)
    scored, _, uem = metric.uemify(reference, Annotation(uri='ES2008a'),
                                  uem=Timeline([Segment(0, duration)]),
                                  collar=0.25, skip_overlap=False, returns_uem=True)
    scored_seconds = {speaker: scored.label_timeline(speaker).support().duration()
                      for speaker in sorted(reference.labels())}
    # Sweep simultaneous endpoints together: no ordering-dependent transitions.
    times = sorted({t for r in segments for t in (r['start'], r['end'])})
    totals = {r['speaker']: 0.0 for r in segments}
    sets = []
    for a, b in zip(times, times[1:]):
        active = frozenset(r['speaker'] for r in segments if r['start'] < b and r['end'] > a)
        for speaker in active:
            totals[speaker] += b - a
        if not sets or sets[-1] != active:
            sets.append(active)
    # Include silence between turns; exclude initial/final outside-span silence.
    transitions = max(0, len(sets) - 1)
    return {'speaker_seconds': scored_seconds, 'raw_speaker_seconds': totals,
            'scoring_policy': {'pyannote_metrics': '3.2.1', 'collar_seconds': 0.25,
                               'skip_overlap': False, 'uem': [0, duration],
                               'support': 'per-speaker union after all-reference-boundary collar extrusion'},
            'active_set_transitions': transitions,
            'eligible': len(scored_seconds) == 4 and min(scored_seconds.values(), default=0) >= 2 and transitions >= 8}


def annotations(archive):
    words, segments = [], []
    with zipfile.ZipFile(archive) as z:
        for speaker in 'ABCD':
            base = 'ES2008a.' + speaker
            for element in ET.fromstring(z.read('words/' + base + '.words.xml')):
                if element.tag == 'w' and element.get('punc') != 'true' and element.get('starttime'):
                    words.append({'start': float(element.get('starttime')), 'end': float(element.get('endtime')),
                                  'speaker': speaker, 'text': element.text or ''})
            for element in ET.fromstring(z.read('segments/' + base + '.segments.xml')):
                if element.tag == 'segment':
                    segments.append({'start': float(element.get('transcriber_start')),
                                     'end': float(element.get('transcriber_end')), 'speaker': speaker})
    return words, segments


def verify(manifest_path, asset_dir):
    manifest = json.loads(Path(manifest_path).read_text())
    if manifest['schema_version'] != 1 or not manifest['eligibility']['eligible']:
        raise ValueError('invalid or ineligible frozen manifest')
    root = Path(asset_dir)
    for name, expected in manifest['files'].items():
        if Path(name).name != name or identity(root / name) != expected:
            raise ValueError('fixture integrity mismatch: ' + name)
    ref = json.loads((root / 'reference.json').read_text())
    validate_reference(ref)
    if ref['duration'] != manifest['duration'] or manifest['duration'] not in (120, 240) or manifest['start'] != 0 or manifest['meeting'] != 'ES2008a':
        raise ValueError('reference duration or selection mismatch')
    if eligibility(ref['segments'], ref['duration']) != manifest['eligibility']:
        raise ValueError('reference eligibility mismatch')
    _, source_segments = annotations(root / 'annotations.zip')
    attempts = []
    for duration in (120, 240):
        check = eligibility(clipped(source_segments, duration), duration)
        attempts.append({'duration': duration, **check})
        if check['eligible']:
            break
    if attempts != manifest['selection_attempts'] or attempts[-1]['duration'] != manifest['duration'] or not attempts[-1]['eligible']:
        raise ValueError('annotation-only selection evidence mismatch')
    if ref['segments'] != clipped(source_segments, manifest['duration']):
        raise ValueError('reference segments differ from source annotations')
    with wave.open(str(root / 'fixture.wav')) as w:
        if (w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()) != (1, 2, 16000, int(ref['duration'] * 16000)):
            raise ValueError('fixture WAV format mismatch')
    return manifest


def prepare(asset_dir, manifest_path, download=False):
    root = Path(asset_dir).resolve()
    # Reuse never regenerates a frozen fixture or silently accepts changed bytes.
    if Path(manifest_path).exists():
        return verify(manifest_path, root)
    root.mkdir(parents=True, exist_ok=True)
    if download:
        for name, url in [('annotations.zip', ANNOTATION_URL), ('ES2008a.Mix-Headset.wav', AUDIO_URL)]:
            if not (root / name).exists():
                subprocess.run(['curl', '-fL', '--max-time', '300', '-o', str(root / name), url], check=True)
    words, segments = annotations(root / 'annotations.zip')
    attempts = []
    for duration in (120, 240):
        selected = clipped(segments, duration)
        check = eligibility(selected, duration)
        attempts.append({'duration': duration, **check})
        if check['eligible']:
            break
    else:
        raise ValueError('AMI fixture fails approved eligibility at 120 and 240 seconds: ' + json.dumps(attempts))
    reference = {'schema_version': 1, 'meeting': 'ES2008a', 'duration': duration,
                 'segments': selected, 'words': sorted(clipped(words, duration), key=lambda r: (r['start'], r['speaker'], r['end']))}
    validate_reference(reference)
    (root / 'reference.json').write_text(json.dumps(reference, indent=2) + '\n')
    command = ['ffmpeg', '-nostdin', '-y', '-i', str(root / 'ES2008a.Mix-Headset.wav'),
               '-t', str(duration), '-ar', '16000', '-ac', '1', '-c:a', 'pcm_s16le', '-map_metadata', '-1', str(root / 'fixture.wav')]
    subprocess.run(command, check=True, capture_output=True)
    manifest = {'schema_version': 1, 'meeting': 'ES2008a', 'start': 0, 'duration': duration,
                'eligibility': check, 'selection_attempts': attempts,
                'sources': {'audio': AUDIO_URL, 'annotations': ANNOTATION_URL,
                            'license': 'AMI corpus CC BY 4.0; see https://groups.inf.ed.ac.uk/ami/download/'},
                'reference_policy': 'DER: manual transcriber segments; WER: timed w elements, no punc/nonlexical tags, clipped and chronological (speaker tie-break)',
                'conversion': {'command': command, 'ffmpeg_version': subprocess.check_output(['ffmpeg', '-version'], text=True).splitlines()[0]},
                'files': {name: identity(root / name) for name in ('ES2008a.Mix-Headset.wav', 'annotations.zip', 'fixture.wav', 'reference.json')}}
    # Exclusive creation freezes manifest before inference; cannot overwrite accidentally.
    target = Path(manifest_path)
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open('x') as f:
        json.dump(manifest, f, indent=2)
        f.write('\n')
    return verify(target, root)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--assets', required=True, type=Path)
    p.add_argument('--manifest', required=True, type=Path)
    p.add_argument('--download', action='store_true')
    args = p.parse_args()
    print(json.dumps(prepare(args.assets, args.manifest, args.download), indent=2))
