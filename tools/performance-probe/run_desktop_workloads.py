#!/usr/bin/env python3
"""Sequential fresh-process Desktop workload matrix, with separate calibration."""
import argparse
import json
from pathlib import Path
import subprocess
import socket
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-20-desktop-workloads'
CASES = ['camera-1', 'camera-4', 'camera-8', 'camera-focus-7',
         'raw-closed', 'raw-open', 'scan-360', 'scan-3600', 'scan-36000',
         'cloud-10000', 'cloud-60000']


def memory_guard(result):
    return result.get('status') == 'failed' and any(
        text in result.get('error', '') for text in ['PSS guard', 'summed RSS guard'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--stage', choices=['calibrate', 'smoke', 'measure'], required=True)
    parser.add_argument('--case', choices=CASES, action='append')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    manifest_path = args.output / (args.stage + '-manifest.json')
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else []
    cases = args.case or CASES
    for repeat in range(3 if args.stage == 'measure' else 1):
        for case in cases:
            apps = ['lichtblick', 'native'] if args.stage == 'calibrate' or repeat % 2 else ['native', 'lichtblick']
            for app in apps:
                name = f'{repeat}-{case}-{app}' if args.stage == 'measure' else f'{args.stage}-{case}-{app}'
                output = args.output / name
                if output.exists():
                    result_path = output / 'result.json'
                    if not result_path.exists() or (json.loads(result_path.read_text())['status'] != 'ok' and not memory_guard(json.loads(result_path.read_text()))):
                        raise RuntimeError('Existing incomplete/failed run: ' + str(output))
                    print('Already completed: ' + name, flush=True)
                    continue
                with socket.socket() as available:
                    available.bind(('127.0.0.1', 0))
                    port = available.getsockname()[1]
                command = [sys.executable, str(ROOT / 'tools/performance-probe/measure_desktop_workload.py'),
                           '--case', str(REPORT / (case + '.json')), '--app', app, '--output', str(output), '--port', str(port)]
                if args.stage == 'calibrate':
                    command += ['--calibrate', '--jpeg-dir', '/tmp/mcap-gpu-comparison-jpegs']
                else:
                    command += ['--seconds', '3' if args.stage == 'smoke' else '12']
                    if case.startswith('camera'):
                        command += ['--references', str(args.output / f'calibrate-{case}-{app}/references.json')]
                print('Starting: ' + name, flush=True)
                start = time.time()
                status = subprocess.run(command, cwd=ROOT).returncode
                manifest.append({'name': name, 'repeat': repeat, 'case': case, 'app': app,
                                 'start_unix': start, 'seconds': time.time() - start, 'exit_code': status})
                manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
                if status:
                    result_path = output / 'result.json'
                    if result_path.exists() and memory_guard(json.loads(result_path.read_text())):
                        print('Recorded experiment memory guard: ' + name, flush=True)
                    else:
                        raise SystemExit(status)


if __name__ == '__main__':
    main()
