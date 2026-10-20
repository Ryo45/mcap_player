#!/usr/bin/env python3
"""Matched visible cadence: 4/8 cameras, 60k points; fresh processes, three repeats."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-20-desktop-workloads'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--references-root', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    manifest_path = args.output / 'manifest.json'
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else []

    def run(name, case, app, all_ten=False, calibrate=False):
        output = args.output / name
        if output.exists():
            if json.loads((output / 'result.json').read_text())['status'] != 'ok':
                raise RuntimeError('existing failed trial: ' + name)
            return
        with socket.socket() as available:
            available.bind(('127.0.0.1', 0))
            port = available.getsockname()[1]
        command = [sys.executable, str(ROOT / 'tools/performance-probe/measure_desktop_cadence.py'),
                   '--case', str(REPORT / (case + '.json')), '--app', app,
                   '--output', str(output), '--seconds', '12', '--port', str(port)]
        if all_ten:
            command += ['--all-camera-10hz']
        if calibrate:
            command += ['--calibrate', '--jpeg-dir', '/tmp/mcap-gpu-comparison-jpegs']
        elif case.startswith('camera'):
            root = args.output if app == 'native' else args.references_root
            command += ['--references', str(root / f'calibrate-{case}-{app}/references.json')]
        print('Cadence: ' + name, flush=True)
        status = subprocess.run(command, cwd=ROOT).returncode
        manifest.append({'name': name, 'case': case, 'app': app, 'all_camera_10hz': all_ten,
                         'calibration': calibrate, 'exit_code': status})
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
        if status:
            raise SystemExit(status)

    for case in ['camera-4', 'camera-8']:
        run(f'calibrate-{case}-native', case, 'native', all_ten=True, calibrate=True)
    for repeat in range(3):
        for case in ['camera-4', 'camera-8', 'cloud-60000']:
            for app in (['native', 'lichtblick'] if repeat % 2 == 0 else ['lichtblick', 'native']):
                run(f'{repeat}-{case}-{app}', case, app, all_ten=app == 'native' and case.startswith('camera'))
            if case == 'camera-8':
                run(f'{repeat}-{case}-native-background5', case, 'native')


if __name__ == '__main__':
    main()
