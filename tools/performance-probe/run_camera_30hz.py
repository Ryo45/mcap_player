#!/usr/bin/env python3
"""Compare 30 Hz camera playback, preserving all attempts and policy differences."""
import argparse
from pathlib import Path
import socket
import subprocess

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-22-camera-30hz'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--stage', choices=['calibrate', 'smoke', 'measure'], required=True)
    parser.add_argument('--app', choices=['native', 'lichtblick'], action='append')
    parser.add_argument('--views', type=int, nargs='+', default=[1, 4])
    parser.add_argument('--codec', choices=['none', 'zstd'], action='append')
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    for repeat in range(args.repeats if args.stage == 'measure' else 1):
        for codec in args.codec or (['none'] if args.stage != 'measure' else ['none', 'zstd']):
            for views in args.views:
                apps = ['native', 'lichtblick'] if repeat % 2 == 0 else ['lichtblick', 'native']
                for app in apps:
                    if args.app and app not in args.app:
                        continue
                    policies = ['source', 'all'] if app == 'native' and views == 4 and args.stage != 'calibrate' else ['all' if app == 'lichtblick' and views == 4 else 'source']
                    for policy in policies:
                        if (args.output / 'STOP').exists():
                            return
                        name = f'{repeat}-{codec}-{views}-{policy}-{app}' if args.stage == 'measure' else f'calibrate-{views}-{app}' if args.stage == 'calibrate' else f'smoke-{codec}-{views}-{policy}-{app}'
                        output = args.output / name
                        if output.exists():
                            print('Preserving attempt ' + str(output), flush=True)
                            continue
                        with socket.socket() as probe:
                            probe.bind(('127.0.0.1', 0))
                            port = probe.getsockname()[1]
                        command = ['python3', str(ROOT / 'tools/performance-probe/measure_desktop_workload.py'),
                                   '--case', str(REPORT / f'{codec}-{views}-{policy}.json'), '--app', app,
                                   '--output', str(output), '--port', str(port)]
                        if args.stage == 'calibrate':
                            command.append('--calibrate')
                        else:
                            command += ['--references', str(args.output / f'calibrate-{views}-{app}/references.json'),
                                        '--seconds', '3' if args.stage == 'smoke' else '4']
                        print('Starting ' + name, flush=True)
                        subprocess.run(command, cwd=ROOT, check=False)


if __name__ == '__main__':
    main()
