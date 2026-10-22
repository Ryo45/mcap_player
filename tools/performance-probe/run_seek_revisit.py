#!/usr/bin/env python3
"""Measure verified, sequential Desktop seek revisits; retain every failed attempt."""
import argparse
import hashlib
import json
from pathlib import Path
import socket
import subprocess

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-22-seek-revisit'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--phase', choices=['before', 'after'], required=True)
    parser.add_argument('--app', choices=['native', 'lichtblick'], action='append')
    parser.add_argument('--case', choices=['same', 'alternating', 'scrub', 'single'], action='append')
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--calibrate', action='store_true')
    parser.add_argument('--headless-binary', type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    for repeat in range(1 if args.calibrate else args.repeats):
        for name in args.case or (['scrub', 'single'] if args.calibrate else ['same', 'alternating', 'scrub', 'single']):
            for app in args.app or ['native']:
                if (args.output / 'STOP').exists():
                    return
                calibration = 'single' if name == 'single' else 'full'
                output = args.output / (f'calibrate-{calibration}-{app}' if args.calibrate else f'{repeat}-{name}-{app}')
                if output.exists():
                    print('Preserving existing attempt: ' + str(output), flush=True)
                    continue
                case = REPORT / f'{name}{"-after" if args.phase == "after" else ""}.json'
                if args.headless_binary:
                    case_data = json.loads(case.read_text())
                    pattern = 'alternating' if name == 'single' else name
                    binary = args.headless_binary.resolve()
                    output.mkdir()
                    command = [str(binary), 'seek-workload', case_data['data'], pattern, str(len(case_data['seek_offsets_ns']))]
                    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                    (output / 'stderr.log').write_text(completed.stderr)
                    result = {'status': 'ok' if completed.returncode == 0 else 'failed',
                              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                              'case_sha256': hashlib.sha256(case.read_bytes()).hexdigest(), 'command': command,
                              'returncode': completed.returncode, 'stdout': completed.stdout}
                    if completed.returncode == 0:
                        result['measurement'] = json.loads(completed.stdout)
                    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
                    print('Physical ' + str(output) + ': ' + result['status'], flush=True)
                    continue
                with socket.socket() as probe:
                    probe.bind(('127.0.0.1', 0))
                    port = probe.getsockname()[1]
                command = ['python3', str(ROOT / 'tools/performance-probe/measure_desktop_workload.py'),
                           '--case', str(case), '--app', app, '--output', str(output), '--port', str(port)]
                if args.calibrate:
                    command.append('--calibrate')
                else:
                    if args.phase == 'after' and app == 'native':
                        references = args.output / f'calibrate-{calibration}-{app}/references.json'
                    else:
                        old = '/tmp/mcap-raw-storage-prefetch-single' if name == 'single' else '/tmp/mcap-raw-storage-prefetch'
                        if app == 'lichtblick':
                            old = '/tmp/mcap-raw-storage-single-chunk' if name == 'single' else '/tmp/mcap-raw-storage-results'
                        references = Path(old) / f'calibrate-1-{app}/references.json'
                    command += ['--seek-only', '--references', str(references)]
                print('Starting ' + str(output), flush=True)
                subprocess.run(command, cwd=ROOT, check=False)


if __name__ == '__main__':
    main()
