#!/usr/bin/env python3
"""Camera-8 control: repeat the pixel/CPU protocol without periodic smaps_rollup."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import threading
from types import SimpleNamespace

import measure_desktop_workload as probe


class NoPeriodicPss:
    def __init__(self, _pid, _origin):
        self.stop_event = threading.Event()
        self.thread = SimpleNamespace(join=lambda **_kwargs: None)
        self.dropped = 0

    def drain(self):
        return []


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--references-root', type=Path, required=True)
    parser.add_argument('--app', choices=['native', 'lichtblick'])
    args = parser.parse_args()
    if args.app:
        probe.PssSampler = NoPeriodicPss
        sys.argv = [str(Path(probe.__file__)), '--case', str(probe.REPORT / 'camera-8.json'),
                    '--app', args.app, '--output', str(args.output), '--seconds', '12',
                    '--references', str(args.references_root / f'calibrate-camera-8-{args.app}/references.json')]
        # Let the OS choose an unused port, including after previous TIME_WAIT sockets.
        import socket
        with socket.socket() as available:
            available.bind(('127.0.0.1', 0))
            sys.argv += ['--port', str(available.getsockname()[1])]
        try:
            probe.main()
        finally:
            path = args.output / 'result.json'
            if path.exists():
                result = json.loads(path.read_text())
                result['control'] = {'periodic_pss': False, 'cpu_rss_pixels': 'unchanged',
                                     'paused_pss': 'still collected outside timed interval',
                                     'wrapper_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
                path.write_text(json.dumps(result, indent=2) + '\n')
        return
    if args.output.exists():
        parser.error('new output required')
    args.output.mkdir(parents=True)
    manifest = []
    for repeat in range(3):
        for app in (['native', 'lichtblick'] if repeat % 2 == 0 else ['lichtblick', 'native']):
            name = f'{repeat}-camera-8-{app}'
            command = [sys.executable, str(Path(__file__)), '--output', str(args.output / name),
                       '--references-root', str(args.references_root), '--app', app]
            print('Control: ' + name, flush=True)
            status = subprocess.run(command).returncode
            manifest.append({'name': name, 'exit_code': status})
            (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
            if status:
                raise SystemExit(status)


if __name__ == '__main__':
    main()
