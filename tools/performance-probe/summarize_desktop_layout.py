#!/usr/bin/env python3
"""Validate complete Desktop runs and preserve per-run ranges beside medians."""
import argparse
import json
from pathlib import Path
import re
import statistics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    root = args.directory
    manifest = json.loads((root/'manifest.json').read_text())
    groups = {}
    audits = []
    provenance = {'data_sha256': set(), 'harness_sha256': set(), 'native_patch_sha256': set()}
    binaries = {}
    for job in manifest['jobs']:
        result = json.loads((root/job['name']/'result.json').read_text())
        if job['exit_code'] != 0 or result['status'] != 'ok' or result['calibration']:
            raise ValueError(f"incomplete run: {job['name']}")
        for key, values in provenance.items():
            values.add(result[key])
        binaries.setdefault(result['app'], set()).add(result['binary_sha256'])
        group = groups.setdefault((result['case'], result['app']), [])
        group.append({'run': job['name'], **result['summary']})
        if len(result['seeks']) != 4:
            raise ValueError('missing seek confirmation')
        audit = {'run': job['name'], 'pixel_confirmed_seeks': 4}
        if result['app'] == 'native':
            log = (root/job['name']/'app.log').read_text()
            confirmations = []
            for command, seek in enumerate(result['seeks'], start=2):
                block = log.split(f'GPU_BENCH_COMMAND id={command}\n', 1)[1]
                block = block.split('GPU_BENCH_COMMAND', 1)[0]
                expected = 1735689600000000000 + seek['offset_ns']//100000000*100000000
                if not re.search(rf'GPU_BENCH_FRAME arrival={expected} ', block):
                    raise ValueError(f"Native absolute texture mismatch: {job['name']}")
                confirmations.append(expected)
            audit['absolute_texture_log_ns'] = confirmations
        if result['case'] == 'raw':
            stamps = [s['raw_log_ns'] for s in result['samples'] if 'raw_log_ns' in s]
            if len(set(stamps)) < 20 or any(b < a for a, b in zip(stamps, stamps[1:])):
                raise ValueError('Raw timestamps did not advance monotonically')
            audit['distinct_observed_raw_timestamps'] = len(set(stamps))
        work = sorted(s['observer_work_ms'] for s in result['samples'])
        audit['observer_work_median_ms'] = statistics.median(work)
        audit['observer_work_p95_ms'] = work[int((len(work)-1)*.95)]
        audits.append(audit)
    if len(manifest['jobs']) != 18 or len(groups) != 6 or any(len(runs) != 3 for runs in groups.values()):
        raise ValueError('expected three complete repeats of all six conditions')
    if any(len(values) != 1 for values in provenance.values()) or any(len(v) != 1 for v in binaries.values()):
        raise ValueError('run provenance changed')
    summary = {'provenance': {k: next(iter(v)) for k, v in provenance.items()},
               'binary_sha256': {k: next(iter(v)) for k, v in binaries.items()},
               'audits': audits, 'conditions': {}}
    lines = ['# 3反復の中央値', '',
             'P95はrunごとのP95の中央値。CPUは1 core=100%。PSSはsample最大値の中央値。', '',
             '| case | app | image Hz | lag median / P95 ms | Scene変化 Hz | CPU % | PSS MiB |',
             '|---|---|---:|---:|---:|---:|---:|']
    for case in ['default', 'scan', 'raw']:
        for app in ['native', 'lichtblick']:
            runs = groups[case, app]
            stats = {}
            for key in runs[0]:
                if key == 'run':
                    continue
                values = [run[key] for run in runs]
                stats[key] = {'median': statistics.median(values), 'min': min(values), 'max': max(values)}
            summary['conditions'][case+'-'+app] = {'runs': runs, 'statistics': stats}
            med = {key: value['median'] for key, value in stats.items()}
            lines.append(f"| {case} | {app} | {med['image_updates_hz']:.2f} | "
                         f"{med['lag_median_ms']:.0f} / {med['lag_p95_ms']:.0f} | "
                         f"{med['scene_changes_hz']:.2f} | {med['cpu_one_core_percent']:.1f} | "
                         f"{med['peak_sampled_pss_mib']:.1f} |")
    lines += ['', '各反復・範囲・観測負荷・Seek検証は[summary.json](summary.json)を参照。', '']
    (root/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    (root/'summary.md').write_text('\n'.join(lines))
    print('\n'.join(lines))


if __name__ == '__main__':
    main()
