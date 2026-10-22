#!/usr/bin/env python3
"""Sequential Raw Image storage matrix; preserve failures and all repetitions."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-22-raw-storage'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--stage', choices=['calibrate', 'smoke', 'measure'], required=True)
    parser.add_argument('--variant', action='append')
    parser.add_argument('--views', type=int, nargs='+', default=[1, 4])
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--app', choices=['native', 'lichtblick'], action='append')
    parser.add_argument('--seconds', type=float, default=6)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    variants = args.variant or ['none-8m', 'zstd-8m', 'lz4-8m', 'zstd-128m', 'zstd-256m', 'split-zstd-8m', 'unchunked']
    for repeat in range(args.repeats if args.stage == 'measure' else 1):
        for variant in variants:
            for views in args.views:
                case = REPORT / f'raw-{variant}-camera-{views}.json'
                apps = ['lichtblick', 'native'] if args.stage == 'calibrate' or repeat % 2 else ['native', 'lichtblick']
                for app in apps:
                    if args.app and app not in args.app:
                        continue
                    if (args.output / 'STOP').exists():
                        print('Stopped between application runs', flush=True)
                        return
                    if variant == 'unchunked' and app == 'native':
                        continue  # Explicit refusal measured separately; not a playback score.
                    name = f'{repeat}-{variant}-{views}-{app}' if args.stage == 'measure' else (f'calibrate-{views}-{app}' if args.stage == 'calibrate' else f'smoke-{variant}-{views}-{app}')
                    output = args.output / name
                    if (output / 'result.json').exists():
                        print('Preserved existing: ' + name, flush=True)
                        continue
                    with socket.socket() as probe:
                        probe.bind(('127.0.0.1', 0))
                        port = probe.getsockname()[1]
                    command = [sys.executable, str(ROOT / 'tools/performance-probe/measure_desktop_workload.py'),
                               '--case', str(case), '--app', app, '--output', str(output), '--port', str(port)]
                    if args.stage == 'calibrate':
                        command += ['--calibrate']
                    else:
                        command += ['--seconds', '3' if args.stage == 'smoke' else str(args.seconds), '--references',
                                    str(args.output / f'calibrate-{views}-{app}/references.json')]
                    print('Starting: ' + name, flush=True)
                    status = subprocess.run(command, cwd=ROOT).returncode
                    if status:
                        print('Recorded failure; continuing other conditions: ' + name, flush=True)


if __name__ == '__main__':
    main()
