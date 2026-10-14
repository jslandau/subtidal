"""DER uses optimal speaker permutation, 250ms collar, and scores overlap.
WER is speaker-independent chronological lexical transcript WER.
"""
import argparse
import json
import math
import unicodedata
from pathlib import Path


def normalize(text):
    text = unicodedata.normalize('NFKC', text).lower().replace('’', "'")
    # Apostrophes are retained (contractions); other punctuation becomes spaces.
    return ' '.join(''.join(c if c == "'" or not unicodedata.category(c).startswith('P') else ' ' for c in text).split())


def validate(rows, duration):
    for r in rows:
        if not all(math.isfinite(r[k]) for k in ('start', 'end')) or not 0 <= r['start'] < r['end'] <= duration:
            raise ValueError('invalid segment interval')
        if not isinstance(r['speaker'], (str, int)):
            raise ValueError('invalid speaker label')


def score(reference, hypothesis):
    from jiwer import process_words
    from pyannote.core import Annotation, Segment, Timeline
    from pyannote.metrics.diarization import DiarizationErrorRate
    duration = reference['duration']
    def annotation(rows):
        validate(rows, duration)
        result = Annotation(uri='ES2008a')
        for i, row in enumerate(rows):
            result[Segment(row['start'], row['end']), i] = str(row['speaker'])
        return result
    ref = annotation(reference['segments'])
    hyp = annotation(hypothesis['segments'])
    metric = DiarizationErrorRate(collar=0.25, skip_overlap=False)
    details = metric(ref, hyp, uem=Timeline([Segment(0, duration)]), detailed=True)
    reference_text = normalize(' '.join(w['text'] for w in sorted(reference['words'], key=lambda w: (w['start'], w['speaker'], w['end']))))
    hypothesis_text = normalize(hypothesis['text'])
    if not reference_text:
        raise ValueError('empty lexical reference')
    wer = process_words(reference_text, hypothesis_text)
    return {'der': float(details['diarization error rate']),
            'der_components': {k: float(v) for k, v in details.items()},
            'wer': float(wer.wer), 'word_errors': {'substitutions': wer.substitutions, 'deletions': wer.deletions, 'insertions': wer.insertions, 'hits': wer.hits},
            'policy': {'collar_seconds': 0.25, 'skip_overlap': False, 'speaker_permutation': 'optimal', 'wer': 'NFKC lowercase punctuation-to-space except apostrophe; collapse whitespace; no number expansion'}}


def gates(baseline, candidate, max_der_regression=0.0, max_wer_regression=0.0):
    values = [baseline[k] for k in ('der', 'wer')] + [candidate[k] for k in ('der', 'wer')]
    if not all(math.isfinite(v) and v >= 0 for v in values):
        raise ValueError('invalid metric values')
    return {'der_nonregression': candidate['der'] <= baseline['der'] + max_der_regression,
            'wer_nonregression': candidate['wer'] <= baseline['wer'] + max_wer_regression}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--reference', required=True, type=Path)
    p.add_argument('--hypothesis', required=True, type=Path)
    p.add_argument('--output', required=True, type=Path)
    a = p.parse_args()
    result = score(json.loads(a.reference.read_text()), json.loads(a.hypothesis.read_text()))
    a.output.write_text(json.dumps(result, indent=2) + '\n')
