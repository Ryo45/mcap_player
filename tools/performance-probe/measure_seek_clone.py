#!/usr/bin/env python3
"""Isolate discarded seek-state copies using unchanged production stage_restore."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def sha(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/examples/seek_clone')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--samples', type=int, default=50)
    parser.add_argument('--points', type=int, nargs='+', default=[60_000, 1_000_000])
    args = parser.parse_args()
    if not 3 <= args.repeats <= 10 or not 5 <= args.samples <= 500 or any(not 1 <= p <= 1_000_000 for p in args.points):
        parser.error('repeats 3..10, samples 5..500, points 1..1000000 required')
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = {
        'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'binary_sha256': sha(args.binary), 'platform': platform.platform(),
        'cpu': next((line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), None),
        'sources': {str(p.relative_to(ROOT)): sha(p) for p in [
            Path(__file__).resolve(), ROOT / 'tools/performance-probe/examples/seek_clone.rs',
            ROOT / 'crates/viewer-core/src/feature/runtime.rs',
            ROOT / 'crates/viewer-core/src/feature/controllers/point_cloud.rs',
            ROOT / 'crates/viewer-core/src/feature/point_cloud.rs']},
        'runs': [],
    }
    cases = [(points, fmt) for points in args.points for fmt in ['xyz', 'rgba', 'rgba-intensity']]
    rows = []
    for repeat in range(args.repeats):
        for points, fmt in cases if repeat % 2 == 0 else reversed(cases):
            name = f'{repeat}-{points}-{fmt}'
            command = [str(args.binary.resolve()), str(points), fmt, str(args.samples)]
            print('Measuring ' + name, flush=True)
            result = subprocess.run(command, capture_output=True, text=True, timeout=120)
            (args.output / f'{name}.stderr').write_text(result.stderr)
            manifest['runs'].append({'name': name, 'command': command, 'returncode': result.returncode})
            (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
            result.check_returncode()
            data = json.loads(result.stdout)
            (args.output / f'{name}.json').write_text(json.dumps(data, indent=2) + '\n')
            for mode in ['clear-only', 'decode', 'failure']:
                for control in [False, True]:
                    selected = [r for r in data['rows'] if r['mode'] == mode and r['control'] == control]
                    assert len(selected) == args.samples
                    rows.append({'repeat': repeat, 'points': points, 'attributes': fmt, 'mode': mode, 'control': control,
                                 **{k: statistics.median(r[k] for r in selected) for k in ['ns', 'allocated_bytes', 'allocation_calls', 'peak_extra_live_bytes']},
                                 'p95_ns': sorted(r['ns'] for r in selected)[int((len(selected) - 1) * .95)]})
    summary = []
    for points, fmt in cases:
        for mode in ['clear-only', 'decode', 'failure']:
            row = {'points': points, 'attributes': fmt, 'mode': mode}
            for control, label in [(False, 'populated'), (True, 'empty_control')]:
                selected = [r for r in rows if (r['points'], r['attributes'], r['mode'], r['control']) == (points, fmt, mode, control)]
                assert len(selected) == args.repeats
                row[label] = {k: statistics.median(r[k] for r in selected) for k in ['ns', 'p95_ns', 'allocated_bytes', 'allocation_calls', 'peak_extra_live_bytes']}
            summary.append(row)
    (args.output / 'summary.json').write_text(json.dumps({'summary': summary, 'run_medians': rows}, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
