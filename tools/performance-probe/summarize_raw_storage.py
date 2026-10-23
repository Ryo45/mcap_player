#!/usr/bin/env python3
"""Archive all Raw storage trials, including excluded attempts and failures."""
import argparse
from collections import defaultdict
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import statistics


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--title', default='Raw storage比較の集計')
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    archive = args.output / 'runs'
    archive.mkdir(parents=True, exist_ok=True)
    manifest = []
    groups = defaultdict(list)
    for path in sorted(args.input.rglob('result.json')):
        run = json.loads(path.read_text())
        relative = path.parent.relative_to(args.input)
        final = len(relative.parts) == 1 and relative.name[0].isdigit()
        name = str(relative).replace('/', '--')
        files = []
        for filename in ['result.json', 'app.log', 'references.json', 'running.json', 'resource-guard.json']:
            source = path.parent / filename
            if source.exists():
                content = source.read_bytes()
                target = archive / f'{name}-{filename}.gz'
                target.write_bytes(gzip.compress(content, mtime=0))
                files.append({'file': str(target.relative_to(args.output)), 'original_sha256': sha(content), 'original_bytes': len(content)})
        manifest.append({'name': str(relative), 'final_matrix': final, 'status': run['status'], 'error': run.get('error'), 'resource_guard': run.get('resource_guard'), 'files': files})
        if final:
            groups[(run['case'], run['app'])].append(run)
            if relative.name.startswith('0-') and (any(v in relative.name for v in ['none-8m-', 'zstd-256m-', 'unchunked-']) or '-4-' in relative.name and any(v in relative.name for v in ['zstd-single-4s-', 'zstd-8m-4s-'])):
                for filename in ['played.png', 'failed.png']:
                    source = path.parent / filename
                    if source.exists():
                        screenshots = args.output / 'screenshots'
                        screenshots.mkdir(exist_ok=True)
                        shutil.copy2(source, screenshots / f'{relative.name}-{filename}')
    summary = []
    for (case, app), runs in sorted(groups.items()):
        good = [r for r in runs if r['status'] == 'ok']
        row = {'case': case, 'app': app, 'runs': len(runs), 'successes': len(good), 'failures': [r.get('error') for r in runs if r['status'] != 'ok']}
        if good:
            metrics = {
                'cpu_percent': [r['summary']['cpu_one_core_percent'] for r in good],
                'peak_pss_mib': [r['summary']['peak_sampled_pss_mib'] for r in good],
                'focus_updates_hz': [r['summary']['cameras'][0]['updates_hz'] for r in good],
                'minimum_updates_hz': [min(c['updates_hz'] for c in r['summary']['cameras']) for r in good],
                'minimum_progress_hz': [min(c['progress_hz'] for c in r['summary']['cameras']) for r in good],
                'opening_ready_ms': [r['opening_to_ready_ms'] for r in good if 'opening_to_ready_ms' in r],
                'after_pause_pss_mib': [r['after_pause']['pss_kib']/1024 for r in good],
                'worst_lag_p95_ms': [max(c['lag_ms']['p95'] for c in r['summary']['cameras']) for r in good],
                'median_seek_ms': [statistics.median(s['latency_ms'] for s in r['seeks'] if s['offset_ns'] != 0) for r in good if any(s['offset_ns'] != 0 for s in r['seeks'])],
            }
            row['metrics'] = {k: {'median': statistics.median(v), 'min': min(v), 'max': max(v)} for k, v in metrics.items() if v}
        summary.append(row)
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    lines = ['# ' + args.title, '', '計画は各条件3反復。実際の成功/試行数は表を参照。数値は成功した反復の中央値で、3/3未満は完全な比較として扱わない。', '',
             '| case | app | 成功/試行 | focus Hz | 最低 Hz | timeline frame/s | wall位相P95 ms | CPU % | PSS MiB | 起動〜ready ms |',
             '|---|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    for row in summary:
        metric = row.get('metrics')
        values = [f'{metric[k]["median"]:.2f}' if k in metric else '—' for k in ['focus_updates_hz', 'minimum_updates_hz', 'minimum_progress_hz', 'worst_lag_p95_ms', 'cpu_percent', 'peak_pss_mib', 'opening_ready_ms']] if metric else ['—'] * 7
        lines.append(f'| {row["case"]} | {row["app"]} | {row["successes"]}/{row["runs"]} | ' + ' | '.join(values) + ' |')
    lines += ['', 'PSSは再生中のsample最大。timeline frame/sは画像のlog timeの進み。wall位相は再生開始を基準にした差であり、実際のcursorと画像間の年齢ではない。起動〜readyはUI自動設定・検査待ち込み。失敗試行・予備試行・計測側修正前の試行もmanifestへ保存する。', '']
    (args.output / 'summary.md').write_text('\n'.join(lines))
    print(json.dumps({'final_runs': sum(r['runs'] for r in summary), 'successful': sum(r['successes'] for r in summary), 'archived_results': len(manifest)}))


if __name__ == '__main__':
    main()
