#!/usr/bin/env python3
"""Copy external experiment evidence to persistent storage, excluding large assets."""
import argparse
from pathlib import Path
import shutil

BASE = Path(__file__).resolve().parent
DEFAULT_NAMES = ['inference-comparison-trials', 'inference-comparison-first-observed',
                 'inference-comparison-burn-first', 'inference-comparison-cpu-smoke',
                 'inference-comparison-cuda-warmed', 'inference-comparison-cpu-matrix',
                 'inference-comparison-builds', 'inference-comparison-builds-burn']

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--destination', type=Path, required=True)
    p.add_argument('--source',type=Path,action='append')
    a=p.parse_args()
    a.destination.mkdir(parents=True,exist_ok=True)
    sources=a.source or [Path('/tmp')/name for name in DEFAULT_NAMES]
    for source in sources:
        if source.is_dir():
            target=a.destination/source.name
            if target.exists():
                raise FileExistsError('Refusing to overwrite archive '+str(target))
            shutil.copytree(source,target)
    shutil.copy2(BASE/'host.json',a.destination/'host.json')
    shutil.copytree(BASE/'fixtures',a.destination/'fixture-provenance',ignore=shutil.ignore_patterns('__pycache__'))
    print(a.destination)

if __name__=='__main__':
    main()
