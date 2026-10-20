#!/usr/bin/env python3
"""Summarize complete Desktop pilot runs; include every resource sample, even ambiguous images."""
import argparse
import json
from pathlib import Path
import re
import statistics


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory',type=Path)
    args=parser.parse_args()
    manifest=json.loads((args.directory/'manifest.json').read_text())
    if manifest['status']!='ok' or len(manifest['jobs'])!=12:
        raise ValueError('complete 12-run manifest required')
    rows=[]
    for job in manifest['jobs']:
        result=json.loads((args.directory/job['output']/'result.json').read_text())
        if result['status']!='ok':raise ValueError(job['output'])
        summary=result['summary']
        memory=[s['resources']['pss_kib']/1024 for s in result['samples'] if 'resources'in s]
        pids=set(result['after_play']['pids'])
        vram=[]
        for line in result['gpu_after'].splitlines():
            match=re.search(r'\|\s+0\s+N/A\s+N/A\s+(\d+)\s+\S+\s+.*?\s+(\d+)MiB\s*\|',line)
            if match and int(match[1])in pids:vram.append(int(match[2]))
        if not vram:raise ValueError('no owned GPU process: '+job['output'])
        seeks=[s['latency_ms'] for s in result['seeks'][:-1]]
        rows.append({'case':job['case'],'app':job['app'],'repeat':job['repeat'],
                     'observed_updates_hz':summary['observed_updates_hz'],
                     'source_progress_hz':summary['source_progress_hz'],
                     'lag_median_ms':summary['wall_lag_median_ms'],'lag_p95_ms':summary['wall_lag_p95_ms'],
                     'sampled_peak_pss_mib':max(memory),'gpu_vram_after_play_mib':sum(vram),
                     'cpu_one_core_percent':100*summary['cpu_seconds']/summary['observation_seconds'],
                     'seek_ms':seeks,'rejected_samples':summary['rejected_samples'],'samples':summary['samples']})
    groups=[]
    for case in ['1cam-20s','8cam-60s-one-selected']:
        for app in ['native','lichtblick']:
            group=[r for r in rows if r['case']==case and r['app']==app]
            if len(group)!=3:raise ValueError('three repeats required')
            keys=['observed_updates_hz','source_progress_hz','lag_median_ms','lag_p95_ms',
                  'sampled_peak_pss_mib','gpu_vram_after_play_mib','cpu_one_core_percent']
            item={'case':case,'app':app,'runs':3,'medians':{k:statistics.median(r[k]for r in group)for k in keys},
                  'ranges':{k:[min(r[k]for r in group),max(r[k]for r in group)]for k in keys},
                  'rejected_samples':sum(r['rejected_samples']for r in group),'samples':sum(r['samples']for r in group)}
            seeks=[v for r in group for v in r['seek_ms']]
            item['seek_ms']={'median':statistics.median(seeks),'min':min(seeks),'max':max(seeks),'n':len(seeks)}
            groups.append(item)
    (args.directory/'summary.json').write_text(json.dumps({'runs':rows,'groups':groups},indent=2)+'\n')
    lines=['# Results','', 'Three runs per row; medians of per-run metrics. P95 column is the median of per-run P95s.', '',
           '| Recording / selected | App | Observed Hz | Source progress Hz | Lag median / P95 ms | Sampled peak PSS MiB | GPU VRAM after play MiB | CPU % of one core | Seek median [min–max] ms | Rejected samples |',
           '|---|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    for g in groups:
        m=g['medians'];s=g['seek_ms']
        lines.append(f"| {g['case']} / 1 | {g['app']} | {m['observed_updates_hz']:.2f} | {m['source_progress_hz']:.2f} | {m['lag_median_ms']:.0f} / {m['lag_p95_ms']:.0f} | {m['sampled_peak_pss_mib']:.1f} | {m['gpu_vram_after_play_mib']:.0f} | {m['cpu_one_core_percent']:.1f} | {s['median']:.0f} [{s['min']:.0f}–{s['max']:.0f}] | {g['rejected_samples']} / {g['samples']} |")
    (args.directory/'summary.md').write_text('\n'.join(lines)+'\n')
    print('\n'.join(lines))

if __name__=='__main__':main()
