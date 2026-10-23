#!/usr/bin/env python3
"""Repeat a large-Chunk Web workload with the current production-source-free harness."""
import argparse
from pathlib import Path
import subprocess
import sys

from measure_viewer_pilot import ROOT


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--duration-seconds', type=float, required=True)
    parser.add_argument('--cameras', type=int, default=8)
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--gpu', choices=['hardware', 'software'], default='hardware')
    parser.add_argument('--wake-display', action='store_true')
    parser.add_argument('--vulkan-icd', type=Path)
    parser.add_argument('--dist', type=Path, default=ROOT / 'apps/viewer-web/dist')
    args = parser.parse_args()
    if args.output.exists() or args.repeats < 1:
        parser.error('use a new output directory and positive repeat count')
    args.output.mkdir(parents=True)
    failed = False
    for repeat in range(args.repeats):
        command = [sys.executable, str(Path(__file__).with_name('measure_web_recording.py')),
                   '--data', str(args.data.resolve()), '--output', str((args.output / f'run-{repeat}').resolve()),
                   '--dist', str(args.dist.resolve()), '--gpu', args.gpu, '--duration-seconds', str(args.duration_seconds),
                   '--seconds', str(args.seconds), '--cameras', str(args.cameras)]
        if args.wake_display:
            command.append('--wake-display')
        if args.vulkan_icd:
            command += ['--vulkan-icd', str(args.vulkan_icd)]
        failed = subprocess.run(command, timeout=240, check=False).returncode != 0 or failed
    return 1 if failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
