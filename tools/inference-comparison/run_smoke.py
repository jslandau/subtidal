#!/usr/bin/env python3
"""Explicit short English fixture trials, separate from AMI quality gates."""
import argparse
import json
import os
from pathlib import Path
from run_trials import BASE, digest, trial

ROOT = BASE.parents[1]

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--model-dir',type=Path,required=True)
    p.add_argument('--diar-v2',type=Path,required=True)
    p.add_argument('--diar-v3',type=Path,required=True)
    p.add_argument('--runner',action='append',choices=['runner-baseline','runner-ort-v3','runner-burn'])
    p.add_argument('--provider',action='append',choices=['cpu','cuda'])
    p.add_argument('--timeout',type=float,default=1200)
    a=p.parse_args()
    import sys
    sys.path.insert(0,str(BASE/'python'))
    from integrity import verify_models
    models=verify_models(a.model_dir,a.diar_v2,a.diar_v3)
    fixture=ROOT/'tests/fixtures/macos-webgpu-smoke.wav'
    a.output.mkdir(parents=True,exist_ok=True)
    records=[]
    for name in a.runner or ['runner-baseline','runner-ort-v3','runner-burn']:
        install=BASE/name/'target/install'
        artifacts=json.loads((install/'artifacts.json').read_text())
        binary=install/artifacts['binary']['name']
        if digest(binary)!=artifacts['binary']['sha256']:
            raise ValueError('staged binary mismatch')
        env=dict(os.environ,LD_LIBRARY_PATH=str(install))
        env.pop('ORT_DYLIB_PATH',None)
        for provider in a.provider or ['cpu','cuda']:
            directory=a.output/(name+'-'+provider)
            if directory.exists():
                raise FileExistsError(str(directory))
            command=[str(binary),'--fixture',str(fixture),'--model-dir',str(a.model_dir),
                     '--diar-model',str(a.diar_v2 if name=='runner-baseline' else a.diar_v3),
                     '--provider',provider,'--mode','combined','--warmup','0',
                     '--output',str(directory/'result.json')]
            record=trial(command,directory,a.timeout,env)
            if record['status'] == 'pass':
                from score import normalize
                expected = 'The quick brown fox jumps over the lazy dog and runs through the forest'
                from jiwer import wer
                record['expected_text'] = expected
                record['smoke_wer'] = wer(normalize(expected), normalize(record['result']['transcript']))
                record['speech_nonempty'] = bool(record['result']['transcript'].strip())
                if not record['speech_nonempty']:
                    record['status'] = 'failed'
                    record['error'] = 'smoke fixture produced no caption'
            record.update(runner=name,provider=provider,fixture_sha256=digest(fixture),
                          models=models,artifacts=artifacts,scope='short English smoke, not AMI quality gates')
            records.append(record)
            (a.output/'smoke.json').write_text(json.dumps(records,indent=2)+'\n')
            print(name,provider,record['status'],flush=True)
    return 0 if all(r['status']=='pass' for r in records) else 1

if __name__=='__main__':
    raise SystemExit(main())
