#!/usr/bin/env python3
"""Alternate fresh Native processes to compare camera cadence with the same GPU pixel observer."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-20-native-camera-cadence'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--jpeg-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    jobs = [dict(variant=variant, repeat=repeat) for repeat in range(3)
            for variant in (['baseline', 'periodic'] if repeat % 2 == 0 else ['periodic', 'baseline'])]
    manifest = dict(status='running', jobs=jobs, started_unix=time.time(),
                    cadence_change_sha256=hashlib.sha256((REPORT/'cadence-change.patch').read_bytes()).hexdigest(),
                    runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    path = args.output/'manifest.json'
    path.write_text(json.dumps(manifest, indent=2)+'\n')
    for job in jobs:
        variant = job['variant']; name = f"{variant}-{job['repeat']}"
        print('START', name, flush=True)
        command = [sys.executable, str(ROOT/'tools/performance-probe/compare_native_desktop.py'),
                   '--app', 'native', '--native', str(ROOT/f'target/gpu-native/viewer-native-cadence-{variant}'),
                   '--native-patch', str(REPORT/f'{variant}-measurement.patch'),
                   '--data', str(args.data.resolve()), '--jpeg-dir', str(args.jpeg_dir.resolve()),
                   '--output', str((args.output/name).resolve()), '--seconds', '12',
                   '--seek-offsets', '25310000000,48710000000,16110000000', '--wake-display']
        run = subprocess.run(command, cwd=ROOT, timeout=160, check=False)
        job.update(exit_code=run.returncode, output=name)
        path.write_text(json.dumps(manifest, indent=2)+'\n')
        if run.returncode:
            manifest['status'] = 'failed'; path.write_text(json.dumps(manifest, indent=2)+'\n')
            raise SystemExit(run.returncode)
    manifest.update(status='ok', finished_unix=time.time())
    path.write_text(json.dumps(manifest, indent=2)+'\n')


if __name__ == '__main__':
    main()
