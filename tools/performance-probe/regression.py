#!/usr/bin/env python3
"""Prepare/run/check fixed representative production workloads without modifying sources."""
import argparse
import hashlib
import json
import os
import math
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time
import tempfile

from measure_viewer_pilot import ROOT, process_sample
from measure_web_recording import digest

HERE = Path(__file__).resolve().parent
CASES = HERE / 'regression_cases.json'
# A median must cross BOTH relative and absolute tolerances. These are detection
# thresholds, not product SLOs. Failed/missing runs never enter a successful median.
LIMITS = {
    'workload_ms': ('higher', .20, 10), 'seek_revisit_ms': ('higher', .25, 1),
    'peak_pss_mib': ('higher', .20, 64), 'peak_rss_mib': ('higher', .20, 64), 'open_ms': ('higher', .25, 100),
    'focus_content_hz': ('lower', .15, 1), 'background_min_content_hz': ('lower', .20, .5),
    'log_seconds_per_wall_second': ('lower', .10, .05),
    'heartbeat_max_ms': ('higher', .50, 20), 'seek_focus_ms': ('higher', .25, 20),
}


def write(path, value):
    path = Path(path)
    staging = path.with_name(path.name + '.tmp')
    staging.write_text(json.dumps(value, indent=2) + '\n')
    staging.replace(path)


def machine():
    cpu = next((line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), '')
    return {'cpu': cpu, 'machine': platform.machine(), 'kernel': platform.release(), 'python': platform.python_version()}


def prepare(args, spec):
    args.data.mkdir(parents=True, exist_ok=True)
    manifest_path = args.data / 'fixtures.json'
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text())
        if manifest['cases_sha256'] != digest(CASES):
            raise RuntimeError('Fixture recipe changed; prepare a new directory')
        for name, entry in manifest['fixtures'].items():
            if digest(args.data / (name + '.mcap')) != entry['sha256']:
                raise RuntimeError('Existing fixture changed: ' + name)
    else:
        manifest = {'cases_sha256': digest(CASES), 'generator_sha256': digest(args.probe), 'fixtures': {}, 'complete': False}
        write(manifest_path, manifest)
    if manifest.get('complete'):
        print('Verified existing fixtures', flush=True)
        return
    if manifest['generator_sha256'] != digest(args.probe):
        raise RuntimeError('Generator changed during preparation; use a new directory')
    for name, command in spec['fixtures'].items():
        if name in manifest['fixtures']:
            continue
        target = args.data / (name + '.mcap')
        if os.path.lexists(target):
            raise RuntimeError('Unverified pre-existing fixture: ' + str(target))
        # Generator create_new must see a new file. A unique private subdirectory
        # also makes normal failure cleanup safe and avoids partial final inputs.
        with tempfile.TemporaryDirectory(prefix='.' + name + '-', dir=args.data) as staging:
            temporary = Path(staging) / target.name
            if command[0] == 'rechunk':
                command = [command[0], str(args.data / (command[1][1:] + '.mcap')), str(temporary), *command[2:]]
            else:
                command = [command[0], str(temporary), *command[1:]]
            print('Generating', name, flush=True)
            completed = subprocess.run([str(args.probe.resolve()), *command], check=True, capture_output=True, text=True, timeout=180)
            # Geometry generation intentionally has no stdout JSON.
            metadata = json.loads(completed.stdout) if completed.stdout.strip() else None
            entry = {'sha256': digest(temporary), 'bytes': temporary.stat().st_size}
            os.link(temporary, target)  # Atomic create; never overwrite an existing file.
            write(args.data / (name + '.json'), {'command': command, 'result': metadata})
            manifest['fixtures'][name] = entry
            write(manifest_path, manifest)
    manifest['complete'] = True
    write(manifest_path, manifest)


def native_case(probe, command, output, limit_mib):
    memory = []
    started = time.monotonic()
    with (output / 'stdout.json').open('w') as stdout, (output / 'stderr.log').open('w') as stderr:
        process = subprocess.Popen([str(probe.resolve()), *command], stdout=stdout, stderr=stderr, start_new_session=True)
        status = 'ok'
        try:
            while process.poll() is None:
                sample = process_sample(process.pid)
                memory.append(sample)
                if sample['pss_kib'] > limit_mib * 1024 or time.monotonic() - started > 90:
                    status = 'external-guard'
                    os.killpg(process.pid, 15)
                    break
                time.sleep(.05)
            code = process.wait(timeout=5)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, 9)
                process.wait(timeout=5)
    result = {'status': status if status != 'ok' or code == 0 else 'failed', 'returncode': code, 'memory': memory}
    if result['status'] == 'ok':
        raw = json.loads((output / 'stdout.json').read_text())
        if 'workload_ms' in raw:
            result['metrics'] = {'workload_ms': raw['workload_ms'], 'peak_rss_mib': raw['final_memory']['hwm_kib'] / 1024}
            if raw['delivered_messages'] != raw['expected_messages'] or raw['feature_decoded'] == 0 or raw['unknown_messages'] != 0:
                result['status'] = 'failed'
        else:
            samples = raw['samples']
            result['metrics'] = {'seek_revisit_ms': statistics.median(s['ms'] for s in samples[2:]),
                                 'peak_rss_mib': max(s['memory']['hwm_kib'] for s in samples) / 1024}
    write(output / 'result.json', result)
    return result


def run(args, spec):
    args.output.mkdir(parents=True, exist_ok=False)
    fixtures = json.loads((args.data / 'fixtures.json').read_text())
    if not fixtures.get('complete') or set(fixtures['fixtures']) != set(spec['fixtures']):
        raise RuntimeError('Fixture preparation is incomplete; run prepare first')
    if fixtures['cases_sha256'] != digest(CASES):
        raise RuntimeError('Fixture recipe differs; prepare fixtures again')
    for name, entry in fixtures['fixtures'].items():
        if digest(args.data / (name + '.mcap')) != entry['sha256']:
            raise RuntimeError('Fixture content changed: ' + name)
    engine_names = ['native', 'web'] if args.engine == 'both' else [args.engine]
    jobs = [(engine, case) for engine in engine_names for case in spec[engine]]
    result = {'version': 1, 'engine': args.engine, 'machine': machine(), 'fixtures': fixtures,
              'cases_sha256': digest(CASES), 'repeats': args.repeats, 'runs': [],
              'probe_sha256': digest(args.probe), 'limits': LIMITS,
              'harness': {p.name: digest(p) for p in [Path(__file__), HERE / 'measure_web_recording.py', HERE / 'web_recording_probe.js', HERE / 'benchmark_browser.py']},
              'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'product_diff_sha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD', '--', 'apps', 'crates'], cwd=ROOT)).hexdigest(),
              'product_sources': {str(p.relative_to(ROOT)): digest(p) for base in [ROOT/'apps', ROOT/'crates', HERE/'src'] for p in sorted(base.rglob('*.rs'))}}
    for repeat in range(args.repeats):
        for engine, case in (jobs if repeat % 2 == 0 else list(reversed(jobs))):
            name = engine + '/' + case['id']
            output = args.output / f'{repeat}-{engine}-{case["id"]}'
            data = args.data / (case['fixture'] + '.mcap')
            print('Running', repeat, name, flush=True)
            row = {'case': name, 'repeat': repeat}
            try:
                if engine == 'native':
                    output.mkdir()
                    command = case['command']
                    measured = native_case(args.probe, [command[0], str(data), *command[1:]], output, args.pss_limit_mib)
                    row.update(status=measured['status'], metrics=measured.get('metrics', {}), environment={})
                else:
                    command = [sys.executable, str(HERE / 'measure_web_recording.py'), '--data', str(data),
                               '--dist', str(args.dist.resolve()), '--output', str(output), '--gpu', args.gpu,
                               '--cameras', str(case['cameras']), '--duration-seconds', str(case['span']),
                               '--seconds', str(case['seconds']), '--pss-limit-mib', str(args.pss_limit_mib)]
                    if case.get('seeks'):
                        command.append('--seeks')
                    if args.wake_display:
                        command.append('--wake-display')
                    if args.vulkan_icd:
                        command += ['--vulkan-icd', str(args.vulkan_icd)]
                    subprocess.run(command, check=False, timeout=240)
                    measured = json.loads((output / 'result.json').read_text())
                    env = measured.get('environment', {})
                    row.update(status=measured['status'], metrics=measured.get('metrics', {}), environment={
                        'browser': env.get('version'), 'gpu_mode': env.get('mode'), 'gpu_devices': env.get('gpu', {}).get('devices'),
                        'adapter': measured.get('initial', {}).get('adapter'), 'viewport': env.get('viewport'),
                        'canvases': measured.get('final', {}).get('canvases')})
            except Exception as error:
                row.update(status='failed', error=repr(error), metrics={})
            result['runs'].append(row)
            write(args.output / 'suite.json', result)
    success = all(row['status'] == 'ok' for row in result['runs'])
    if args.baseline:
        problems = compare(json.loads(args.baseline.read_text()), result)
        write(args.output / 'comparison.json', {'baseline': str(args.baseline), 'baseline_sha256': digest(args.baseline), 'problems': problems})
        print('\n'.join(problems) if problems else 'No regression beyond configured tolerances', flush=True)
        success = success and not problems
    return 0 if success else 1


def compare(baseline, current):
    problems = []
    for key in ['version', 'engine', 'machine', 'cases_sha256', 'harness', 'limits']:
        if json.dumps(baseline.get(key), sort_keys=True) != json.dumps(current.get(key), sort_keys=True):
            problems.append('incompatible ' + key)
    if baseline['fixtures']['fixtures'] != current['fixtures']['fixtures']:
        problems.append('incompatible input data')
    spec = json.loads(CASES.read_text())
    engines = ['native', 'web'] if current.get('engine') == 'both' else [current.get('engine')]
    expected = {engine + '/' + case['id'] for engine in engines if engine in spec for case in spec[engine]}
    if not expected or any(suite.get('repeats', 0) < 3 or {r['case'] for r in suite['runs']} != expected for suite in [baseline, current]):
        problems.append('incomplete representative suite or fewer than three repetitions')
    keys = {r['case'] for r in baseline['runs']} | {r['case'] for r in current['runs']}
    for case in sorted(keys):
        groups = [[r for r in suite['runs'] if r['case'] == case] for suite in [baseline, current]]
        if any(len(group) != suite['repeats'] or {r['repeat'] for r in group} != set(range(suite['repeats']))
               or any(r['status'] != 'ok' for r in group) for group, suite in zip(groups, [baseline, current])):
            problems.append(case + ': missing/failed/duplicate run')
            continue
        if any(r.get('environment') != groups[0][0].get('environment') for group in groups for r in group):
            problems.append(case + ': incompatible execution environment')
            continue
        required = ({'seek_revisit_ms', 'peak_rss_mib'} if case.startswith('native/seek-') else {'workload_ms', 'peak_rss_mib'}) if case.startswith('native/') else {'open_ms', 'focus_content_hz', 'background_min_content_hz', 'log_seconds_per_wall_second', 'heartbeat_max_ms', 'peak_pss_mib'}
        if any(not required <= set(row['metrics']) for group in groups for row in group):
            problems.append(case + ': missing required metrics')
            continue
        metrics = set(groups[0][0]['metrics'])
        if any(set(r['metrics']) != metrics for group in groups for r in group):
            problems.append(case + ': missing metrics')
            continue
        for metric in sorted(metrics):
            values = [r['metrics'][metric] for group in groups for r in group]
            if all(value is None for value in values):
                continue
            if any(isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0 for value in values):
                problems.append(case + ': invalid metric ' + metric)
                continue
            if metric not in LIMITS:
                continue
            before, after = [statistics.median(r['metrics'][metric] for r in group) for group in groups]
            direction, relative, absolute = LIMITS[metric]
            delta = after - before if direction == 'higher' else before - after
            if delta > max(abs(before) * relative, absolute):
                problems.append(f'{case}: {metric} {before:.3f} -> {after:.3f}')
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'run', 'check'])
    parser.add_argument('--data', type=Path, default=ROOT / 'target/performance-regression/data')
    parser.add_argument('--probe', type=Path, default=ROOT / 'target/release/performance-probe')
    parser.add_argument('--dist', type=Path, default=ROOT / 'apps/viewer-web/dist')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--baseline', type=Path)
    parser.add_argument('--current', type=Path)
    parser.add_argument('--engine', choices=['native', 'web', 'both'], default='both')
    parser.add_argument('--gpu', choices=['hardware', 'software'], default='hardware')
    parser.add_argument('--wake-display', action='store_true')
    parser.add_argument('--vulkan-icd', type=Path)
    parser.add_argument('--pss-limit-mib', type=int, default=4500)
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    if args.repeats < 3:
        parser.error('regression comparisons require at least three fresh processes')
    if args.action == 'check':
        if not args.baseline or not args.current:
            parser.error('--baseline and --current are required')
        problems = compare(json.loads(args.baseline.read_text()), json.loads(args.current.read_text()))
        print('\n'.join(problems) if problems else 'No regression beyond configured tolerances')
        return 1 if problems else 0
    spec = json.loads(CASES.read_text())
    if args.action == 'prepare':
        prepare(args, spec)
        return 0
    if not args.output:
        parser.error('--output is required')
    return run(args, spec)


if __name__ == '__main__':
    raise SystemExit(main())
