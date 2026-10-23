#!/usr/bin/env python3
"""Join per-operation Native stage timestamps with independent pixel observations."""
import argparse
import json
from pathlib import Path
import re
import statistics


def distribution(values):
    values = sorted(values)
    return {'median': statistics.median(values), 'min': values[0], 'max': values[-1], 'count': len(values)}


def parse_trace(text):
    return [{'stage': s, 'ns': int(ns), 'op': int(op), 'arrival': int(a)} for s, ns, op, a in
            re.findall(r'SEEK_TRACE stage=(\w+) ns=(\d+) op=(\d+) arrival=(-?\d+)', text)]


def summarize_run(path):
    result = json.loads((path / 'result.json').read_text())
    if result['status'] != 'ok':
        raise ValueError('failed trial: ' + str(path))
    # Same-target adjacent seeks can pass the pixel test before another draw.
    seeks = [(i, v) for i, v in enumerate(result['seeks'])
             if not i or v['offset_ns'] != result['seeks'][i - 1]['offset_ns']]
    # Second lap of the seven-operation scrub: all relevant Chunks have been visited.
    revisits = [(i, v) for i, v in seeks if i >= 7]
    summary = {'pixel_revisit_ms': distribution([v['latency_ms'] for _, v in revisits]),
               'control_revisit_ms': distribution([v['control_completed_ms'] for _, v in revisits]),
               'excluded_same_target_operations': len(result['seeks']) - len(seeks)}
    events = parse_trace((path / 'app.log').read_text())
    if not events:
        return summary
    rows = []
    for index, seek in seeks:
        op = index + 2
        group = [e for e in events if e['op'] == op]
        expected = seek['native_texture_log_ns'][0]
        def stage(name, arrival=None):
            found = [e for e in group if e['stage'] == name and (arrival is None or e['arrival'] == arrival)]
            if not found:
                raise ValueError((str(path), op, name))
            return found[0]['ns']
        accepted = stage('command_accept')
        draw = stage('image_draw', expected)
        presented = next(e['ns'] for e in group if e['stage'] == 'present_done' and e['ns'] >= draw)
        frame_begin = max(e['ns'] for e in group if e['stage'] == 'frame_begin' and e['ns'] <= draw)
        acquired = next(e for e in group if e['stage'] == 'acquire_begin' and frame_begin <= e['ns'] <= presented)
        acquire_end = next(e for e in group if e['stage'] == 'acquire_done' and e['ns'] >= acquired['ns'])
        ns = {'restore_total': stage('seek_done') - stage('seek_begin'),
              'restore_read': stage('restore_read_done') - stage('cancel_done'),
              'position': stage('position_ready') - stage('restore_read_done'),
              'feature_restore': stage('feature_restored') - stage('position_ready'),
              'submit_from_restore': stage('decode_submitted', expected) - stage('seek_done'),
              'worker_queue': stage('worker_start', expected) - stage('decode_submitted', expected),
              'pixel_conversion': stage('worker_done', expected) - stage('worker_start', expected),
              'result_wait': stage('result_collected', expected) - stage('worker_done', expected),
              'upload_cpu': stage('upload_done', expected) - stage('upload_begin', expected),
              'surface_acquire': acquire_end['ns'] - acquired['ns'],
              'accept_to_present_call_return': presented - accepted}
        rows.append({'index': index, 'op': op, 'offset_ns': seek['offset_ns'], 'expected_arrival_ns': expected,
                     'revisit': index >= 7, 'stages_ms': {k: v / 1e6 for k, v in ns.items()},
                     'pixel_latency_ms': seek['latency_ms'],
                     'residual_ms': seek['latency_ms'] - ns['accept_to_present_call_return'] / 1e6,
                     'note': 'Residual includes pre-accept wait, FIFO/compositor/pixel observation; not GPU duration.'})
    summary['operations'] = rows
    summary['revisit_stages_ms'] = {k: distribution([v['stages_ms'][k] for v in rows if v['revisit']])
                                    for k in rows[0]['stages_ms']}
    summary['revisit_residual_ms'] = distribution([v['residual_ms'] for v in rows if v['revisit']])
    return summary


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--runs', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--group', action='append', help='Run-name suffix to aggregate; repeat for multiple groups')
    args = p.parse_args()
    runs = {v.parent.name: summarize_run(v.parent) for v in sorted(args.runs.glob('*-*/result.json'))
            if not v.parent.name.startswith('calibrate')}
    groups = {}
    for kind in args.group or ['control', 'trace', 'lichtblick']:
        values = [r for name, r in runs.items() if name.endswith('-' + kind)]
        groups[kind] = {'successful_runs': len(values), 'pixel_revisit_ms': [v['pixel_revisit_ms']['median'] for v in values]}
        if values and all('revisit_stages_ms' in v for v in values):
            groups[kind]['median_of_run_medians_ms'] = {k: statistics.median(v['revisit_stages_ms'][k]['median'] for v in values)
                                                       for k in values[0]['revisit_stages_ms']}
    args.output.write_text(json.dumps({'groups': groups, 'runs': runs}, indent=2) + '\n')
    print(json.dumps(groups, indent=2))


if __name__ == '__main__':
    main()
