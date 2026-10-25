#!/usr/bin/env python3
"""Interleave unchanged/updated seek_clone binaries and fail on measured regressions."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[2]
MODES = ['clear-only', 'decode', 'failure']
METRICS = ['ns', 'allocated_bytes', 'allocation_calls', 'peak_extra_live_bytes']


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def compare_pair(before, after):
    problems = []
    if set(before) != set(METRICS + ['p95_ns']) or set(after) != set(before):
        return ['missing metrics']
    if any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or v < 0
           for row in [before, after] for v in row.values()):
        return ['invalid metrics']
    for metric, relative, absolute in [('ns', .10, 30_000), ('p95_ns', .20, 100_000)]:
        if after[metric] - before[metric] > max(before[metric] * relative, absolute):
            problems.append(f'{metric}: {before[metric]} -> {after[metric]}')
    for metric in ['allocated_bytes', 'allocation_calls', 'peak_extra_live_bytes']:
        if after[metric] > before[metric]:
            problems.append(f'{metric}: {before[metric]} -> {after[metric]}')
    return problems


def summarize(data, points, fmt, samples):
    assert data['points'] == points and data['attributes'] == fmt and data['samples'] == samples
    assert len(data['rows']) == samples * len(MODES) * 2
    result = []
    for mode in MODES:
        for control in [False, True]:
            selected = [row for row in data['rows'] if row['mode'] == mode and row['control'] is control]
            assert len(selected) == samples
            assert all(set(row) == set(METRICS + ['mode', 'control']) for row in selected)
            assert all(isinstance(row[k], int) and row[k] >= 0 for row in selected for k in METRICS)
            result.append({'mode': mode, 'control': control,
                           **{key: statistics.median(row[key] for row in selected) for key in METRICS},
                           'p95_ns': sorted(row['ns'] for row in selected)[int((len(selected) - 1) * .95)]})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=50)
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    if not 5 <= args.samples <= 500 or not 3 <= args.repeats <= 10:
        parser.error('samples 5..500; repeats 3..10')
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = {
        'before_sha256': digest(args.before), 'after_sha256': digest(args.after),
        'comparison_script_sha256': digest(__file__),
        'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'product_diff_sha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD', '--', 'crates', 'apps'], cwd=ROOT)).hexdigest(),
        'platform': platform.platform(), 'runs': [], 'samples': args.samples, 'repeats': args.repeats,
        'thresholds': {'median_time': '10% and 30us', 'p95_time': '20% and 100us',
                       'allocator_requested_bytes_calls_peak': 'no increase', 'clear_candidate_bytes': 0},
    }
    results = []
    cases = [(n, f) for n in [60_000, 1_000_000] for f in ['xyz', 'rgba', 'rgba-intensity']]
    for repeat in range(args.repeats):
        for points, fmt in cases if repeat % 2 == 0 else reversed(cases):
            for label in ['before', 'after'] if repeat % 2 == 0 else ['after', 'before']:
                name = f'{repeat}-{points}-{fmt}-{label}'
                print('Measuring ' + name, flush=True)
                binary = getattr(args, label)
                command = [str(binary.resolve()), str(points), fmt, str(args.samples)]
                run = subprocess.run(command, capture_output=True, text=True, timeout=120)
                (args.output / (name + '.stderr')).write_text(run.stderr)
                manifest['runs'].append({'name': name, 'command': command, 'returncode': run.returncode})
                (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
                run.check_returncode()
                data = json.loads(run.stdout)
                (args.output / (name + '.json')).write_text(json.dumps(data, indent=2) + '\n')
                for row in summarize(data, points, fmt, args.samples):
                    results.append({'label': label, 'points': points, 'attributes': fmt, 'repeat': repeat, **row})
    summary, problems = [], []
    for points, fmt in cases:
        for mode in MODES:
            for control in [False, True]:
                row = {'points': points, 'attributes': fmt, 'mode': mode, 'control': control}
                for label in ['before', 'after']:
                    selected = [r for r in results if (r['label'], r['points'], r['attributes'], r['mode'], r['control']) == (label, points, fmt, mode, control)]
                    assert len(selected) == args.repeats and {r['repeat'] for r in selected} == set(range(args.repeats))
                    row[label] = {key: statistics.median(r[key] for r in selected) for key in METRICS + ['p95_ns']}
                problems.extend(f'{points}/{fmt}/{mode}/control={control}: {p}' for p in compare_pair(row['before'], row['after']))
                if mode in ['clear-only', 'failure'] and row['after']['allocated_bytes'] != 0:
                    problems.append(f'{points}/{fmt}/{mode}: empty candidate allocated old state')
                summary.append(row)
    output = {'status': 'failed' if problems else 'ok', 'problems': problems, 'summary': summary, 'run_medians': results}
    (args.output / 'comparison.json').write_text(json.dumps(output, indent=2) + '\n')
    print(json.dumps({'status': output['status'], 'problems': problems}))
    return bool(problems)


if __name__ == '__main__':
    raise SystemExit(main())
