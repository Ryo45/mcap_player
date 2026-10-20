#!/usr/bin/env python3
"""Archive the cadence trials and separate image changes from actual render calls."""
import argparse
from collections import defaultdict
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import statistics

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-20-desktop-workloads'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    args = parser.parse_args()
    archive = REPORT / 'cadence'
    archive.mkdir(exist_ok=True)
    groups = defaultdict(list)
    records = []
    sources = set()
    binaries = defaultdict(set)
    for directory in sorted(args.input.iterdir()):
        if not directory.is_dir():
            continue
        result = json.loads((directory / 'result.json').read_text())
        assert result['status'] == 'ok', directory
        assert result['case_sha256'] == hashlib.sha256((REPORT / (result['case'] + '.json')).read_bytes()).hexdigest()
        assert result['harness_sha256'] == hashlib.sha256((ROOT / 'tools/performance-probe/measure_desktop_workload.py').read_bytes()).hexdigest()
        assert result['resource_sampler_sha256'] == hashlib.sha256((ROOT / 'tools/performance-probe/measure_viewer_pilot.py').read_bytes()).hexdigest()
        sources.add(result['source_commit'])
        binaries[result['app']].add(result['binary_sha256'])
        record = {'name': directory.name, 'files': {}}
        for filename in ['result.json', 'app.log', 'references.json']:
            source = directory / filename
            if source.exists():
                payload = source.read_bytes()
                (archive / f'{directory.name}-{filename}.gz').write_bytes(gzip.compress(payload, mtime=0))
                record['files'][filename] = hashlib.sha256(payload).hexdigest()
        records.append(record)
        if result['calibration']:
            continue
        assert len(result['seeks']) == 4
        assert 12 <= result['observed_play_seconds'] <= 13
        assert result['pss_samples_dropped'] == 0
        assert result['summary']['pss_sample_count'] >= 2
        assert set(result['before_play']['pids']) == set(result['after_play']['pids'])
        experiment = result['cadence_experiment']
        assert not experiment['force_scene_redraw']
        assert experiment['wrapper_sha256'] == hashlib.sha256((ROOT / 'tools/performance-probe/measure_desktop_cadence.py').read_bytes()).hexdigest()
        summary = result['summary']
        cameras = summary['cameras']
        row = {'cpu': summary['cpu_one_core_percent'], 'pss': summary['peak_sampled_pss_mib']}
        if cameras:
            row.update(focus_hz=cameras[0]['updates_hz'], background_min_hz=min(v['updates_hz'] for v in cameras[1:]),
                       focus_lag_p95_ms=cameras[0]['lag_ms']['p95'], background_worst_lag_p95_ms=max(v['lag_ms']['p95'] for v in cameras[1:]))
        else:
            row['scene_change_hz'] = summary['scene_changes_hz']
        if result['app'] == 'native':
            counts = experiment['native_calls_during_play']
            assert counts['present'] > 0
            row.update(present_calls_hz=counts['present'] / result['observed_play_seconds'],
                       scene_render_hz=counts['scene'] / result['observed_play_seconds'])
        else:
            assert any(p['role'] == 'renderer' for p in result['before_play']['processes'])
            snapshot = experiment['lichtblick_color_clears']
            case = json.loads((REPORT / (result['case'] + '.json')).read_text())
            # Layout import can replace canvases. Detached canvases have zero rects
            # and must not become spurious 0Hz background panels.
            visible = [v for v in snapshot['canvases'] if v['rect']['width'] > 0 and v['rect']['height'] > 0]
            assert len(visible) == max(1, case['cameras'])
            rates = [len(v['times']) * 1000 / (snapshot['end'] - snapshot['begin']) for v in visible]
            row.update(color_clear_min_hz=min(rates), color_clear_max_hz=max(rates))
        policy = 'all10' if experiment['all_camera_10hz'] else 'default'
        groups[result['case'], result['app'], policy].append(row)
    assert len(records) == 23, len(records)
    assert len(sources) == 1
    assert all(len(v) == 1 for v in binaries.values())
    aggregates = []
    for (case, app, policy), rows in groups.items():
        assert len(rows) == 3, (case, app, policy, len(rows))
        aggregates.append({'case': case, 'app': app, 'policy': policy, 'n': 3,
                           'median': {k: statistics.median(v[k] for v in rows) for k in rows[0]},
                           'runs': rows})
    (archive / 'summary.json').write_text(json.dumps({'aggregates': aggregates, 'archives': records}, indent=2) + '\n')
    shutil.copyfile(args.input / 'manifest.json', archive / 'manifest.json')
    scripts = ['build_desktop_cadence_probe.py', 'measure_desktop_cadence.py',
               'run_desktop_cadence.py', 'summarize_desktop_cadence.py']
    environment = {'source_commit': next(iter(sources)), 'binary_sha256': {k: next(iter(v)) for k, v in binaries.items()},
                   'native_cadence_patch_sha256': hashlib.sha256((REPORT / 'native-cadence.patch').read_bytes()).hexdigest(),
                   'scripts_sha256': {name: hashlib.sha256((ROOT / 'tools/performance-probe' / name).read_bytes()).hexdigest() for name in scripts},
                   'successful_timed_runs': 21, 'successful_calibrations': 2,
                   'forced_scene_redraw': False, 'excluded': ['../cadence-gpu-contended/manifest.json', '../cadence-x11-failure/manifest.json']}
    (archive / 'environment.json').write_text(json.dumps(environment, indent=2) + '\n')
    print(json.dumps([{k: v for k, v in row.items() if k != 'runs'} for row in aggregates], indent=2))


if __name__ == '__main__':
    main()
