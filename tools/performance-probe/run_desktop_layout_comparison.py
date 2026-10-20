#!/usr/bin/env python3
"""Run fresh, sequential Desktop processes with alternating application order."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--native-references', type=Path, required=True)
    parser.add_argument('--lichtblick-references', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seconds', type=float, default=12)
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    if args.output.exists() or not 1 <= args.repeats <= 5:
        parser.error('new output directory and 1..5 repeats required')
    args.output.mkdir(parents=True)
    manifest = {'started_unix': time.time(), 'jobs': []}
    probe = Path(__file__).with_name('measure_desktop_layout.py')
    for repeat in range(args.repeats):
        for case in ['default', 'scan', 'raw']:
            apps = ['native', 'lichtblick'] if repeat % 2 == 0 else ['lichtblick', 'native']
            for app in apps:
                name = f'{repeat}-{case}-{app}'
                references = getattr(args, app + '_references')
                command = [sys.executable, str(probe), '--app', app, '--case', case,
                           '--data', str(args.data), '--references', str(references),
                           '--output', str(args.output/name), '--seconds', str(args.seconds),
                           '--port', str(9290 + len(manifest['jobs']))]
                job = {'name': name, 'command': command, 'started_unix': time.time()}
                print(name, flush=True)
                completed = subprocess.run(command, timeout=180, check=False)
                job.update(exit_code=completed.returncode, finished_unix=time.time())
                manifest['jobs'].append(job)
                (args.output/'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
                if completed.returncode:
                    raise SystemExit(completed.returncode)


if __name__ == '__main__':
    main()
