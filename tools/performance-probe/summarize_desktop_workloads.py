#!/usr/bin/env python3
"""Validate and archive the complete 66-run Desktop workload matrix."""
import argparse
from collections import defaultdict
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import statistics

from collections import Counter
from PIL import Image

from run_desktop_workloads import CASES, REPORT


def spread(values):
    return {'median': statistics.median(values), 'min': min(values), 'max': max(values)}


def check_cloud_pixels(source):
    palette = [(255, 48, 32), (32, 255, 64), (48, 80, 255)]
    result = {'source_palette_rgb': palette, 'scope': '60000-point RGB calibration at 0.31s; no full-frame equality or continuous message-age claim'}
    masks = {}
    for app in ['native', 'lichtblick']:
        path = source / f'calibrate-cloud-60000-{app}/calibrated.png'
        pixels = list(Image.open(path).convert('RGB').crop((320, 76, 1920, 1002)).getdata())
        counts = Counter(pixels)
        labels = {color: i for color in counts for i, expected in enumerate(palette)
                  if max(abs(a - b) for a, b in zip(color, expected)) <= 3}
        hits = [sum(count for color, count in counts.items() if labels.get(color) == i) for i in range(3)]
        assert min(hits) > 100
        mask = {i for i, color in enumerate(pixels) if color in labels}
        masks[app] = mask
        result[app] = {'source_palette_pixel_counts': hits, 'cloud_bounds_xyxy': [
            min(i % 1600 for i in mask) + 320, min(i // 1600 for i in mask) + 76,
            max(i % 1600 for i in mask) + 320, max(i // 1600 for i in mask) + 76]}
    result['bounds_max_difference_px'] = max(abs(a - b) for a, b in zip(result['native']['cloud_bounds_xyxy'], result['lichtblick']['cloud_bounds_xyxy']))
    result['cloud_mask_intersection_over_union'] = len(masks['native'] & masks['lichtblick']) / len(masks['native'] | masks['lichtblick'])
    assert result['bounds_max_difference_px'] <= 2
    (REPORT / 'cloud-pixel-check.json').write_text(json.dumps(result, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    args = parser.parse_args()
    groups = defaultdict(list)
    all_runs = []
    archive = REPORT / 'runs'
    screenshots = REPORT / 'screenshots'
    calibration = REPORT / 'calibration'
    for directory in [archive, screenshots, calibration]:
        directory.mkdir(exist_ok=True)

    def compressed(source, destination):
        payload = source.read_bytes()
        destination.write_bytes(gzip.compress(payload, mtime=0))
        return hashlib.sha256(payload).hexdigest()

    for repeat in range(3):
        for case in CASES:
            for app in ['native', 'lichtblick']:
                name = f'{repeat}-{case}-{app}'
                directory = args.input / name
                result = json.loads((directory / 'result.json').read_text())
                assert result['status'] == 'ok', name
                assert result['case'] == case and result['app'] == app, name
                assert result['limits']['timed_seconds'] == 12, name
                assert len(result['seeks']) == 4, name
                assert 'Monitor is On' in result['display_after'], name
                assert result['pss_samples_dropped'] == 0, name
                assert all('error' not in sample for sample in result['pss_samples']), name
                assert 12 <= result['observed_play_seconds'] <= 13, name
                if app == 'lichtblick':
                    assert any(p['role'] == 'renderer' for p in result['before_play']['processes']), name
                summary = result['summary']
                assert summary['pss_sample_count'] >= 2, name
                if result['kind'] == 'scene':
                    assert summary['scene_changes_hz'] >= 4, name
                groups[case, app].append(result)
                metadata = {key: result[key] for key in ['app', 'case', 'kind', 'summary', 'source_commit',
                            'data_sha256', 'data_bytes', 'binary_sha256', 'harness_sha256', 'native_patch_sha256', 'resource_sampler_sha256',
                            'wall_seconds', 'seeks', 'observed_play_seconds', 'cpu_interval_seconds', 'play_command_ms']}
                metadata.update(name=name, result_sha256=compressed(directory / 'result.json', archive / (name + '.json.gz')),
                                log_sha256=compressed(directory / 'app.log', archive / (name + '.log.gz')))
                all_runs.append(metadata)
                if repeat == 0:
                    shutil.copyfile(directory / 'ready.png', screenshots / (case + '-' + app + '.png'))
    assert len({r['harness_sha256'] for r in all_runs}) == 1
    assert len({r['resource_sampler_sha256'] for r in all_runs}) == 1
    assert len({r['native_patch_sha256'] for r in all_runs}) == 1
    assert len({r['source_commit'] for r in all_runs}) == 1
    for app in ['native', 'lichtblick']:
        assert len({r['binary_sha256'] for r in all_runs if r['app'] == app}) == 1

    aggregates = []
    table = ['| 表示 | app | focus Hz | 背景最低 Hz | Scene変化 Hz | CPU % | PSS MiB |',
             '|---|---|---:|---:|---:|---:|---:|']
    camera_table = ['| 表示 | app | focus lag 中央値 / P95 ms | 最も遅い背景 lag P95 ms |',
                    '|---|---|---:|---:|']
    for case in CASES:
        for app in ['native', 'lichtblick']:
            results = groups[case, app]
            summaries = [r['summary'] for r in results]
            aggregate = {'case': case, 'app': app, 'kind': results[0]['kind'],
                         'cpu_one_core_percent': spread([s['cpu_one_core_percent'] for s in summaries]),
                         'peak_sampled_pss_mib': spread([s['peak_sampled_pss_mib'] for s in summaries]),
                         'paused_pss_mib': spread([s['paused_pss_mib'] for s in summaries]),
                         'peak_sampled_summed_rss_mib': spread([s['peak_sampled_summed_rss_mib'] for s in summaries]),
                         'pss_sample_count': spread([s['pss_sample_count'] for s in summaries]),
                         'pss_read_p95_ms': spread([s['pss_read_ms']['p95'] for s in summaries]),
                         'observer_median_ms': spread([s['observer_work_ms']['median'] for s in summaries]),
                         'observer_p95_ms': spread([s['observer_work_ms']['p95'] for s in summaries])}
            cameras = summaries[0]['cameras']
            if cameras:
                aggregate['cameras'] = [{
                    'camera_id': i,
                    'updates_hz': spread([s['cameras'][i]['updates_hz'] for s in summaries]),
                    'progress_hz': spread([s['cameras'][i]['progress_hz'] for s in summaries]),
                    'lag_median_ms': spread([s['cameras'][i]['lag_ms']['median'] for s in summaries]),
                    'lag_p95_ms': spread([s['cameras'][i]['lag_ms']['p95'] for s in summaries]),
                    'rejected_samples': sum(s['cameras'][i]['rejected_samples'] for s in summaries),
                } for i in range(len(cameras))]
                aggregate['minimum_background_hz'] = spread([min(c['updates_hz'] for c in s['cameras'][1:]) for s in summaries]) if len(cameras) > 1 else None
                aggregate['worst_background_p95_ms'] = spread([max(c['lag_ms']['p95'] for c in s['cameras'][1:]) for s in summaries]) if len(cameras) > 1 else None
                focus = aggregate['cameras'][0]
                background = aggregate['worst_background_p95_ms']
                camera_table.append(f"| {case} | {app} | {focus['lag_median_ms']['median']:.0f} / {focus['lag_p95_ms']['median']:.0f} | " + (f"{background['median']:.0f}" if background else '—') + ' |')
            if aggregate['kind'] == 'scene':
                aggregate['scene_changes_hz'] = spread([s['scene_changes_hz'] for s in summaries])
            if aggregate['kind'] == 'raw':
                aggregate['raw_progress_hz'] = spread([s['raw_progress_hz'] for s in summaries])
            aggregates.append(aggregate)
            focus = f"{aggregate['cameras'][0]['updates_hz']['median']:.2f}" if cameras else '—'
            background = aggregate.get('minimum_background_hz')
            background = f"{background['median']:.2f}" if background else '—'
            scene = aggregate.get('scene_changes_hz')
            scene = f"{scene['median']:.2f}" if scene else '—'
            table.append(f"| {case} | {app} | {focus} | {background} | {scene} | {aggregate['cpu_one_core_percent']['median']:.1f} | {aggregate['peak_sampled_pss_mib']['median']:.1f} |")

    for case in CASES:
        for app in ['native', 'lichtblick']:
            directory = args.input / f'calibrate-{case}-{app}'
            result = json.loads((directory / 'result.json').read_text())
            assert result['status'] == 'ok'
            compressed(directory / 'result.json', calibration / (case + '-' + app + '.json.gz'))
            reference = directory / 'references.json'
            if reference.exists():
                compressed(reference, calibration / (case + '-' + app + '-references.json.gz'))
            if result['kind'] == 'scene':
                shutil.copyfile(directory / 'calibrated.png', calibration / (case + '-' + app + '.png'))
    for stage in ['calibrate', 'smoke', 'measure']:
        shutil.copyfile(args.input / (stage + '-manifest.json'), REPORT / (stage + '-manifest.json'))
    excluded = REPORT / 'excluded'
    excluded.mkdir(exist_ok=True)
    for directory in args.input.glob('excluded-*'):
        compressed(directory / 'result.json', excluded / (directory.name + '.json.gz'))
        compressed(directory / 'app.log', excluded / (directory.name + '.log.gz'))
        image = directory / 'failed.png'
        if image.exists():
            shutil.copyfile(image, excluded / (directory.name + '.png'))
    check_cloud_pixels(args.input)
    (REPORT / 'summary.json').write_text(json.dumps({'aggregates': aggregates, 'runs': all_runs}, indent=2) + '\n')
    (REPORT / 'summary.md').write_text('各条件3反復の中央値。CPUは1 core=100%。PSSはsample最大の中央値。背景最低Hzは各run内の背景カメラ最小値の中央値。\n\n' + '\n'.join(table) + '\n\nP95はrunごとのP95の中央値で、sampleをpoolしたP95ではない。\n\n' + '\n'.join(camera_table) + '\n')
    print('\n'.join(table))


if __name__ == '__main__':
    main()
