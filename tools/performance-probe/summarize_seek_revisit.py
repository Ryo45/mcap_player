#!/usr/bin/env python3
"""Archive seek measurements and compute per-run and across-run revisit statistics."""
import argparse
from collections import defaultdict
import gzip
import hashlib
import json
import math
from pathlib import Path
import shutil
import statistics


def distribution(values):
    values = sorted(values)
    return {'median': statistics.median(values), 'p95': values[math.ceil(.95 * len(values)) - 1], 'max': values[-1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for phase in ['before', 'after', 'lichtblick', 'physical-before', 'physical-after']:
        parser.add_argument('--' + phase, type=Path, required=True)
    parser.add_argument('--forward', type=Path)
    parser.add_argument('--excluded', type=Path, action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / 'runs'
    archive.mkdir(exist_ok=True)
    manifest = []
    groups = defaultdict(list)
    roots = [(phase, getattr(args, phase.replace('-', '_')), True) for phase in
             ['before', 'after', 'lichtblick', 'physical-before', 'physical-after']]
    if args.forward:
        roots += [('forward-control', args.forward, False)]
    roots += [('excluded-' + root.name, root, False) for root in args.excluded]
    for phase, root, include in roots:
        for path in sorted(root.rglob('result.json')):
            result = json.loads(path.read_text())
            relative = path.parent.relative_to(root)
            final = include and len(relative.parts) == 1 and relative.name[0].isdigit()
            name = phase + '--' + str(relative).replace('/', '--')
            files = []
            for filename in ['result.json', 'app.log', 'references.json', 'running.json', 'resource-guard.json', 'stderr.log']:
                source = path.parent / filename
                if not source.exists():
                    continue
                data = source.read_bytes()
                target = archive / f'{name}-{filename}.gz'
                target.write_bytes(gzip.compress(data, mtime=0))
                files.append({'file': str(target.relative_to(args.output)), 'original_sha256': hashlib.sha256(data).hexdigest()})
            manifest.append({'phase': phase, 'name': str(relative), 'included': final,
                             'status': result['status'], 'error': result.get('error'), 'files': files})
            if final:
                pattern = relative.name.split('-')[1]
                groups[(phase, pattern)].append(result)
                if relative.name == '0-scrub-native' or relative.name == '0-scrub-lichtblick':
                    source = path.parent / 'seeked.png'
                    if source.exists():
                        screenshots = args.output / 'screenshots'
                        screenshots.mkdir(exist_ok=True)
                        shutil.copy2(source, screenshots / f'{phase}-scrub.png')
    rows = []
    for (phase, pattern), runs in sorted(groups.items()):
        good = [run for run in runs if run['status'] == 'ok']
        row = {'phase': phase, 'pattern': pattern, 'successes': len(good), 'runs': len(runs)}
        per_run = []
        for run in good:
            physical = 'measurement' in run
            samples = run['measurement']['samples'] if physical else run['seeks']
            # A repeated operation sequence's second half revisits its established working set.
            late = samples[len(samples) // 2:]
            metric = {'all_ms': distribution([v['ms' if physical else 'latency_ms'] for v in samples]),
                      'revisit_ms': distribution([v['ms' if physical else 'latency_ms'] for v in late]),
                      'operations': len(samples)}
            if physical:
                metric.update(expansions=sum(v['chunks_expanded'] for v in samples),
                              revisit_expansions=sum(v['chunks_expanded'] for v in late),
                              seek_cache_bytes=run['measurement']['io'].get('seek_cache_backing_bytes', 0),
                              hwm_mib=run['measurement']['memory']['hwm_kib'] / 1024,
                              retained_rss_mib=run['measurement']['memory']['rss_kib'] / 1024)
            else:
                metric['peak_pss_mib'] = run['summary']['peak_sampled_pss_mib']
            per_run.append(metric)
        row['per_run'] = per_run
        if per_run:
            row['revisit_median_ms'] = statistics.median(v['revisit_ms']['median'] for v in per_run)
            row['revisit_p95_ms'] = statistics.median(v['revisit_ms']['p95'] for v in per_run)
            row['peak_pss_mib'] = statistics.median(v['peak_pss_mib'] for v in per_run) if 'peak_pss_mib' in per_run[0] else None
        rows.append(row)
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (args.output / 'summary.json').write_text(json.dumps(rows, indent=2) + '\n')
    lines = ['# Seek再訪の集計', '', '各条件3プロセス。後半の操作列を再訪として集計し、各プロセスの中央値／nearest-rank P95の中央値を示す。', '',
             '| phase | pattern | 成功/試行 | 再訪中央値 ms | 再訪P95 ms | 操作全体のsample最大PSS MiB |',
             '|---|---|---:|---:|---:|---:|']
    for row in rows:
        values = [f'{row[key]:.3f}' for key in ['revisit_median_ms', 'revisit_p95_ms']] if row['per_run'] else ['—', '—']
        pss = f'{row["peak_pss_mib"]:.1f}' if row.get('peak_pss_mib') is not None else '—'
        lines.append(f'| {row["phase"]} | {row["pattern"]} | {row["successes"]}/{row["runs"]} | ' + ' | '.join(values) + ' | ' + pss + ' |')
    (args.output / 'summary.md').write_text('\n'.join(lines) + '\n')
    print(json.dumps({'rows': len(rows), 'archived_trials': len(manifest)}))


if __name__ == '__main__':
    main()
