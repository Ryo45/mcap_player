#!/usr/bin/env python3
"""Summarize the two repeated diagnosis runs, excluding the initial preflight."""
import json
from pathlib import Path
import statistics
import sys

folder=Path(sys.argv[1])
median=statistics.median
rows=[]
for path in sorted(folder.glob('*-r[12].json')):
    data=json.loads(path.read_text())
    jobs=[j for j in data['jobs'] if j['phase']=='play' and j.get('decode') and 'received' in j]
    row={'run':path.stem,'status':data['status'],
         'peak_pss_mib':max(s['process']['pss_kib'] for s in data['samples'])/1024,
         'max_wasm_mib':max(s.get('wasm_bytes',0) for s in data['samples'])/2**20,
         'play_ms':data.get('play_ms'),
         'painted_during_play':len([p for p in data['paints'] if p['phase']=='play'])}
    if jobs:
        row|={'decode_p50_ms':median(j['decode']['end']-j['decode']['start'] for j in jobs),
              'post_to_decode_p50_ms':median(j['decode']['start']-j['post'] for j in jobs),
              'decode_to_receive_p50_ms':median(j['received']-j['decode']['end'] for j in jobs),
              'draw_p50_ms':median(j['draw_ms'] for j in jobs if 'draw_ms' in j),
              'decode_focus_p50_ms':median(j['decode']['end']-j['decode']['start'] for j in jobs if j['width']==960),
              'decode_thumb_p50_ms':median(j['decode']['end']-j['decode']['start'] for j in jobs if j['width']==112)}
    for phase in ['pre_gc','post_gc']:
        if phase in data:
            state=data[phase]
            row[phase]={'pss_mib':state['process']['pss_kib']/1024,
                        'wasm_mib':state['wasm_bytes']/2**20,
                        'main_heap_used_mib':state['heap']['usedSize']/2**20,
                        'main_backing_mib':state['heap'].get('backingStorageSize',0)/2**20}
    rows.append(row)
(folder/'summary.json').write_text(json.dumps(rows,indent=2)+'\n')
print('| Run | Result | Peak PSS MiB | Max WASM MiB | Worker decode p50 ms | Play paints |')
print('|---|---|---:|---:|---:|---:|')
for r in rows:
    decode=f"{r['decode_p50_ms']:.1f}" if 'decode_p50_ms' in r else '—'
    paints=str(r['painted_during_play']) if 'bare' not in r['run'] else '—'
    print(f"| {r['run']} | {r['status']} | {r['peak_pss_mib']:.1f} | {r['max_wasm_mib']:.1f} | {decode} | {paints} |")
