#!/usr/bin/env python3
"""Summarize 60-second Web probes without treating guard stops as completed playback."""
import argparse
import csv
import gzip
import json
from pathlib import Path
import re
import statistics


def read_run(path):
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rt') as source:
        return json.load(source)


def summarize(folder, log_span):
    rows = []
    timeline = []
    paths = sorted([*folder.glob('baseline-*-r*.json*'), *folder.glob('current-*-r*.json*')])
    for path in paths:
        run = read_run(path)
        samples = run['samples']
        playing = [s for s in samples if s['phase'] == 'play']
        last = playing[-1] if playing else samples[-1]
        row = {
            'run': path.name, 'status': run['status'], 'error': run.get('error'),
            'dataset_sha256': run['dataset_sha256'], 'dataset_bytes': run['dataset_bytes'],
            'reached_log_seconds': last['cursor'] * log_span,
            'play_wall_seconds': run.get('play_ms', (last['now'] - run.get('play_start_ms', last['now']))) / 1000,
            'peak_pss_mib': max(s['process']['pss_kib'] for s in samples) / 1024,
            'max_wasm_mib': max(s.get('wasm_bytes', 0) for s in samples) / 2**20,
            'last_status': last['status'], 'last_metrics': last['metrics'],
            'global_errors': [e for s in samples for e in s['errors']],
        }
        jobs = [j for j in run['jobs'] if j['phase'] == 'play' and j.get('decode') and 'received' in j]
        if jobs:
            row['jpeg_await_p50_ms'] = statistics.median(j['decode']['end'] - j['decode']['start'] for j in jobs)
        row['play_paints'] = len([p for p in run['paints'] if p['phase'] == 'play'])
        if run['status'] == 'ok':
            row['end_metrics'] = run['pre_gc']['metrics']
            row['post_gc_pss_mib'] = run['post_gc']['process']['pss_kib'] / 1024
        for sample in playing:
            match = re.search(r'(\d+) windows/([\d.]+) M(?:i)?B logical/([\d.]+) M(?:i)?B RAM', sample['metrics'] or '')
            timeline.append({
                'run': path.name, 'wall_seconds': (sample['now'] - run['play_start_ms']) / 1000,
                'log_seconds': sample['cursor'] * log_span,
                'pss_mib': sample['process']['pss_kib'] / 1024,
                'wasm_mib': sample.get('wasm_bytes', 0) / 2**20,
                'windows': int(match[1]) if match else None,
                'store_backing_mib': float(match[3]) if match else None,
                'gpu_process_pss_mib': sum(p['pss_kib'] for p in sample['process'].get('processes', [])
                                           if p['role'] == 'gpu-process') / 1024,
                'renderer_pss_mib': sum(p['pss_kib'] for p in sample['process'].get('processes', [])
                                        if p['role'] == 'renderer') / 1024,
                'main_heap_used_mib': sample['heap']['usedSize'] / 2**20,
                'main_backing_excluding_wasm_mib':
                    (sample['heap']['backingStorageSize'] - sample.get('wasm_bytes', 0)) / 2**20,
            })
        rows.append(row)
    (folder / 'summary.json').write_text(json.dumps(rows, indent=2) + '\n')
    if timeline:
        with (folder / 'timeline.csv').open('w') as output:
            writer = csv.DictWriter(output, fieldnames=list(timeline[0]), lineterminator='\n')
            writer.writeheader()
            writer.writerows(timeline)
    print('| Run | Status | Reached log s | Wall s | Peak PSS MiB | Max WASM MiB |')
    print('|---|---|---:|---:|---:|---:|')
    for row in rows:
        print(f"| {row['run']} | {row['status']} | {row['reached_log_seconds']:.2f} | "
              f"{row['play_wall_seconds']:.2f} | {row['peak_pss_mib']:.1f} | {row['max_wasm_mib']:.1f} |")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('folder', type=Path)
    parser.add_argument('--log-span', type=float, default=59.9)
    args = parser.parse_args()
    if args.log_span <= 0:
        parser.error('--log-span must be positive')
    summarize(args.folder, args.log_span)
