#!/usr/bin/env python3
"""Repeat the bounded 4K/8-camera Web playback and seek workload, preserving failures."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys

from measure_viewer_pilot import ROOT


def summarize(output):
    rows=[json.loads(p.read_text()) for p in sorted(output.glob('run-*.json'))]
    lines=['# Web 4K/8-camera playback and seek', '', 'Three fresh browser processes per condition. Medians of successful runs; failures are counted separately.', '',
           '| Dataset | OK / attempts | Playback ms | Focus content changes | Background changes / camera | Individual seek ms | Burst recovery ms | Peak PSS MiB |',
           '|---|---:|---:|---:|---:|---:|---:|---:|']
    for case in ['none-8MiB','zstd-64MiB']:
        attempts=[r for r in rows if r['case']==case]
        group=[r for r in attempts if r['status']=='ok']
        if not group:
            lines.append(f'| {case} | 0 / {len(attempts)} | — | — | — | — | — | — |');continue
        def median(f):return statistics.median(f(r) for r in group)
        def changes(r,cam):return sum(e['phase']=='play' and e['cam']==cam for e in r['paint_events'])
        values=[median(lambda r:r['play_ms']),median(lambda r:changes(r,0)),
                median(lambda r:statistics.mean(changes(r,cam) for cam in range(1,8))),
                median(lambda r:statistics.median(s['all_images_ms'] for s in r['seeks'])),
                median(lambda r:r['burst_recovery_ms']),median(lambda r:r['peak_sampled_pss_kib']/1024)]
        lines.append(f'| {case} | {len(group)} / {len(attempts)} | '+' | '.join(f'{v:.1f}' for v in values)+' |')
    lines+=['', 'Content changes are actual visible canvas image changes during playback; the initial image is excluded.',
            'Source input is 10 Hz per camera. Product admission is focus 10 Hz / background at most 5 Hz; these are not achieved display rates.',
            'Seek completion requires correct image fingerprints on all eight cameras plus committed cursor. Burst ends with synchronous pause.',
            'PSS includes dedicated Chrome descendants, not total system memory or GPU VRAM. Target sampling interval is 200 ms; peaks can be missed.',
            'Software rendering, instrumented paints, cache allowed, 4-second synthetic input. No long-history memory guarantee or hardware GPU claim.']
    (output/'summary.md').write_text('\n'.join(lines)+'\n')
    print('\n'.join(lines),flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data-dir',type=Path)
    parser.add_argument('--references',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--summarize-only',action='store_true')
    args=parser.parse_args()
    if args.summarize_only:summarize(args.output);return
    if args.output.exists():parser.error('use a new output directory')
    if args.data_dir is None or args.references is None:parser.error('--data-dir and --references required')
    args.output.mkdir(parents=True)
    names={f'3840x2160-8cam-10hz-{case}-jpeg':case for case in ['none-8MiB','zstd-64MiB']}
    datasets=[d for d in json.loads((ROOT/'docs/performance/2026-10-14-4k-large-chunks/datasets.json').read_text()) if d['name'] in names]
    environment={'platform':platform.platform(),'python':sys.version,'meminfo':Path('/proc/meminfo').read_text(),
                 'git_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                 'datasets':datasets,'pss_limit_kib':2*1024*1024,
                 'sources_sha256':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in
                                   [Path(__file__),Path(__file__).with_name('measure_web_camera_stress.py'),Path(__file__).with_name('web_camera_stress.js')]},
                 'web_assets_sha256':{p.name:hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in sorted((ROOT/'apps/viewer-web/dist').iterdir())
                                      if p.is_file() and p.suffix in ['.js','.css','.wasm','.html']}}
    (args.output/'environment.json').write_text(json.dumps(environment,indent=2)+'\n')
    shutil.copyfile(args.references,args.output/'references.json')
    for repeat in range(3):
        for dataset in (datasets if repeat%2==0 else list(reversed(datasets))):
            case=names[dataset['name']];output=args.output/f'run-{case}-{repeat}.json'
            print('run',repeat,case,flush=True)
            command=[sys.executable,str(Path(__file__).with_name('measure_web_camera_stress.py')),
                     '--data',str((args.data_dir/(dataset['name']+'.mcap')).resolve()),
                     '--references',str(args.references.resolve()),'--output',str(output)]
            completed=subprocess.run(command,timeout=90,check=False)
            row=json.loads(output.read_text());row['case']=case;row['repeat']=repeat;row['returncode']=completed.returncode
            if row['dataset_sha256']!=dataset['sha256']:raise RuntimeError('dataset differs from baseline')
            output.write_text(json.dumps(row,indent=2)+'\n')
    summarize(args.output)


if __name__=='__main__':main()
