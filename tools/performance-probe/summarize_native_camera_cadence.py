#!/usr/bin/env python3
"""Summarize Native cadence A/B runs and the separate diagnostic trace runs."""
import argparse
import json
from pathlib import Path
import re
import statistics


def trace_summary(directory):
    events = {}
    for stage, wall, camera, arrival in re.findall(
            r'CAMERA_TRACE stage=(\w+) wall_ns=(\d+) camera=(\d+) arrival=(\d+)',
            (directory/'app.log').read_text()):
        events.setdefault((int(camera), int(arrival)), {})[stage] = int(wall)
    pairs = [('received', 'admitted'), ('admitted', 'submitted'), ('submitted', 'started'),
             ('started', 'decoded'), ('decoded', 'uploaded'), ('uploaded', 'painted'),
             ('received', 'painted')]
    result = {}
    for start, end in pairs:
        samples = sorted((stages[end]-stages[start])/1e6 for (_, arrival), stages in events.items()
                         if 1735689600000000000 < arrival < 1735689611000000000
                         and start in stages and end in stages)
        if not samples or samples[0] < 0:
            raise ValueError('missing/negative trace interval')
        result[start+'->'+end] = dict(n=len(samples), median_ms=statistics.median(samples),
                                    p95_ms=samples[int((len(samples)-1)*.95)], max_ms=max(samples))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    manifest = json.loads((args.directory/'manifest.json').read_text())
    if manifest['status'] != 'ok' or len(manifest['jobs']) != 6:
        raise ValueError('complete six-run manifest required')
    groups = []
    for variant in ['baseline', 'periodic']:
        jobs = [j for j in manifest['jobs'] if j['variant'] == variant]
        if len(jobs) != 3:
            raise ValueError('three runs per variant required')
        runs = []
        for job in jobs:
            raw = json.loads((args.directory/job['output']/'result.json').read_text())
            if raw['status'] != 'ok' or raw['camera_trace_enabled']:
                raise ValueError('performance run must succeed with detailed trace disabled')
            summary = raw['summary']
            runs.append(dict(repeat=job['repeat'], observed_hz=summary['observed_updates_hz'],
                             progress_hz=summary['source_progress_hz'],
                             lag_median_ms=summary['wall_lag_median_ms'], lag_p95_ms=summary['wall_lag_p95_ms'],
                             sampled_peak_pss_mib=max(s['resources']['pss_kib']/1024 for s in raw['samples'] if 'resources' in s),
                             cpu_one_core_percent=100*summary['cpu_seconds']/summary['observation_seconds'],
                             rejected_samples=summary['rejected_samples'], samples=summary['samples']))
        keys = ['observed_hz', 'progress_hz', 'lag_median_ms', 'lag_p95_ms',
                'sampled_peak_pss_mib', 'cpu_one_core_percent']
        group = dict(variant=variant, runs=runs,
                     medians={k: statistics.median(r[k] for r in runs) for k in keys},
                     ranges={k: [min(r[k] for r in runs), max(r[k] for r in runs)] for k in keys})
        groups.append(group)
    traces = {variant: trace_summary(args.directory/('trace-'+variant)) for variant in ['baseline', 'periodic']}
    output = dict(groups=groups, diagnostic_traces=traces)
    (args.directory/'summary.json').write_text(json.dumps(output, indent=2)+'\n')
    lines = ['# Native Camera cadence results', '',
             'Three runs per variant; medians of per-run metrics. P95 is the median of per-run P95s.', '',
             '| Variant | Observed Hz | Progress Hz | Lag median / P95 ms | Sampled peak PSS MiB | CPU % of one core |',
             '|---|---:|---:|---:|---:|---:|']
    for group in groups:
        m = group['medians']
        lines.append(f"| {group['variant']} | {m['observed_hz']:.2f} | {m['progress_hz']:.2f} | {m['lag_median_ms']:.0f} / {m['lag_p95_ms']:.0f} | {m['sampled_peak_pss_mib']:.1f} | {m['cpu_one_core_percent']:.1f} |")
    lines += ['', 'Separate diagnostic runs, n=1 per variant; stages are CPU boundaries, not GPU completion.', '',
              '| Stage | Baseline median / P95 ms | Periodic median / P95 ms |', '|---|---:|---:|']
    for stage in traces['baseline']:
        a, b = traces['baseline'][stage], traces['periodic'][stage]
        lines.append(f"| {stage} | {a['median_ms']:.2f} / {a['p95_ms']:.2f} | {b['median_ms']:.2f} / {b['p95_ms']:.2f} |")
    (args.directory/'summary.md').write_text('\n'.join(lines)+'\n')
    print('\n'.join(lines))


if __name__ == '__main__':
    main()
