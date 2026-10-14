#!/usr/bin/env python3
"""Summarize completed trials without promoting missing measurements to a pass."""
import argparse
import json
from pathlib import Path
import statistics
import sys
BASE = Path(__file__).resolve().parent
sys.path.insert(0, str(BASE / 'python'))
from score import score

def distribution(values):
    if not values:
        return None
    ordered = sorted(values)
    return {'n': len(values), 'median': statistics.median(values), 'min': min(values),
            'max': max(values), 'pstdev': statistics.pstdev(values),
            'p95': ordered[min(len(ordered)-1, int(0.95*len(ordered)))]}

def summarize(records, reference, provenance=None):
    expected_hash = (provenance or {}).get('fixture', {}).get('files', {}).get('fixture.wav', {}).get('sha256')
    eligible = (provenance or {}).get('fixture', {}).get('eligibility', {}).get('eligible', False)
    groups = {}
    for record in records:
        key = '/'.join(record[k] for k in ('runner','provider','mode'))
        group = groups.setdefault(key, {'trials': [], 'scores': []})
        group['trials'].append(record)
        if record['status'] == 'pass':
            result = record['result']
            hypothesis = {'text': result['transcript'], 'segments': [
                {'start': row['start_sample']/16000, 'end': row['end_sample']/16000,
                 'speaker': row['speaker_id']} for row in result['segments']]}
            metrics = score(reference, hypothesis)
            if record['mode'] == 'asr':
                metrics['der'] = None
                metrics['der_components'] = None
            elif record['mode'] == 'diar':
                metrics['wer'] = None
                metrics['word_errors'] = None
            group['scores'].append(metrics)
    summary = {}
    for key, group in groups.items():
        valid = [r['result'] for r in group['trials'] if r['status'] == 'pass']
        contract_valid = (bool(expected_hash and eligible)
                          and len({r.get('repetition') for r in group['trials']}) == len(group['trials'])
                          and len({r.get('lock_sha256') for r in group['trials']}) == 1
                          and len({r.get('artifacts', {}).get('binary', {}).get('sha256') for r in group['trials']}) == 1
                          and len({json.dumps(r.get('result', {}).get('runtime_config'), sort_keys=True) for r in group['trials']}) == 1) and all(
            r.get('fixture_sha256') == expected_hash
            and r.get('result', {}).get('warmup_runs', 0) >= 1
            and isinstance(r.get('result', {}).get('runtime_config'), dict)
            and r.get('result', {}).get('runner') == r['runner']
            and r.get('result', {}).get('provider') == r['provider']
            and r.get('result', {}).get('mode') == r['mode']
            and r.get('result', {}).get('real_samples') == int(reference['duration'] * 16000)
            and (r['provider'] != 'cuda' or r['runner'] == 'runner-burn'
                 or r.get('result', {}).get('cuda_preflight') == 'passed')
            for r in group['trials'])
        summary[key] = {'measurement_contract_valid': contract_valid,
                        'requested': len(group['trials']), 'completed': len(valid),
                        'statuses': [r['status'] for r in group['trials']],
                        'scores': group['scores'],
                        'rtf': distribution([r['rtf'] for r in valid]),
                        'load_seconds': distribution([r['load_seconds'] for r in valid]),
                        'warmup_seconds': distribution([r['warmup_seconds'] for r in valid]),
                        'first_diar_compute_seconds': distribution([r['first_diar_output']['compute_seconds'] for r in valid if r['first_diar_output']]),
                        'first_diar_audio_position_seconds': distribution([r['first_diar_output']['audio_sample']/16000 for r in valid if r['first_diar_output']]),
                        'first_asr_compute_seconds': distribution([r['first_asr_output']['compute_seconds'] for r in valid if r['first_asr_output']]),
                        'first_asr_audio_position_seconds': distribution([r['first_asr_output']['audio_sample']/16000 for r in valid if r['first_asr_output']]),
                        'asr_inference_tick_seconds': distribution([t['asr_seconds'] for r in valid for t in r['ticks'] if t['asr_inference']]),
                        'asr_buffer_tick_seconds': distribution([t['asr_seconds'] for r in valid for t in r['ticks'] if not t['asr_inference']]),
                        'diar_inference_tick_seconds': distribution([t['diar_seconds'] for r in valid for t in r['ticks'] if t.get('diar_inference_passes', 0)]),
                        'diar_buffer_tick_seconds': distribution([t.get('diar_feed_seconds', t['diar_seconds']) for r in valid for t in r['ticks'] if not t.get('diar_inference_passes', 0)]),
                        'peak_rss_kib': distribution([r['memory']['peak_rss_kib'] for r in group['trials'] if r.get('memory',{}).get('peak_rss_kib') is not None]),
                        'peak_child_gpu_mib': distribution([r['memory']['peak_child_gpu_mib'] for r in group['trials'] if r.get('memory',{}).get('peak_child_gpu_mib') is not None])}
    return summary

def decision(summary):
    keys = [name + '/cuda/combined' for name in ['runner-baseline','runner-ort-v3','runner-burn']]
    if any(k not in summary or summary[k]['completed'] != 5 or summary[k]['requested'] != 5
           or not summary[k].get('measurement_contract_valid', False) for k in keys):
        return {'recommendation': 'gather more evidence; retain production ORT', 'status': 'incomplete',
                'reason': 'Five completed CUDA-requested combined trials are required for A, B and C; strict ORT preflight must also pass.'}
    a,b,c = [summary[k] for k in keys]
    metric = lambda g,k: statistics.median(s[k] for s in g['scores'])
    gates = {'C_B_WER': metric(c,'wer') <= metric(b,'wer')+.01,
             'C_B_DER': metric(c,'der') <= metric(b,'der')+.02,
             'C_A_WER': metric(c,'wer') <= metric(a,'wer')+.01,
             'C_A_DER': metric(c,'der') <= metric(a,'der')+.02,
             'C_realtime': c['rtf']['median'] < 1,
             'C_A_throughput': c['rtf']['median'] <= 1.25*a['rtf']['median']}
    return {'status': 'exploratory gates evaluated', 'gates': gates,
            'recommendation': 'candidate for separately approved migration' if all(gates.values()) else 'retain ORT',
            'limitations': 'ORT model GPU node execution remains unproven. CPU completion/capacity must be assessed separately. One fixture is not broad quality evidence.'}

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--trials', type=Path, required=True)
    p.add_argument('--reference', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    a=p.parse_args()
    from fixture import digest
    provenance=json.loads((a.trials.parent / 'provenance.json').read_text())
    if digest(a.reference) != provenance['fixture']['files']['reference.json']['sha256']:
        raise ValueError('reference does not match trial provenance')
    result=summarize(json.loads(a.trials.read_text()), json.loads(a.reference.read_text()), provenance)
    a.output.write_text(json.dumps({'groups':result,'decision':decision(result)},indent=2)+'\n')

if __name__=='__main__':
    main()
