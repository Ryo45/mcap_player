#!/usr/bin/env python3
"""Repeat the local Web viewer pilot, alternating run order without concurrent viewers."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--data', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
if args.output.exists():
    parser.error('use a new output directory')
args.output.mkdir(parents=True)
environment = {'platform': platform.platform(), 'python': sys.version,
               'cpuinfo': Path('/proc/cpuinfo').read_text().split('\n\n')[0],
               'meminfo': Path('/proc/meminfo').read_text(),
               'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
               'sources_sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in
                                  [Path(__file__), Path(__file__).with_name('measure_viewer_pilot.py'),
                                   Path(__file__).with_name('browser_cdp.py')]},
               'lichtblick_source_commit': '95713edb309d77589b130286642ab149995a5f7b',
               'lichtblick_image': 'ghcr.io/lichtblick-suite/lichtblick@sha256:230ed53813f11ab5d23c47380f94c6d358438ecf15c3a7e7de9625b07c437611',
               'notes': ['client-only process-tree PSS; servers excluded', 'forced SwiftShader; not hardware GPU throughput',
                         '960x540 focus + 112x63 thumbnail; viewport 1920x1080 DPR 1',
                         'cache-allowed; dataset hashed immediately before every run',
                         'CPU/I/O raw counters include live processes only; omit aggregate comparisons when processes exit']}
environment['web_assets_sha256'] = {p.name: hashlib.file_digest(p.open('rb'), 'sha256').hexdigest()
                                    for p in sorted((ROOT/'apps/viewer-web/dist').iterdir()) if p.is_file()}
(args.output/'environment.json').write_text(json.dumps(environment, indent=2)+'\n')
rows = []
for repetition in range(3):
    # Alternate order to avoid giving one product the same order position every time.
    apps = ['mcap', 'lichtblick'] if repetition % 2 == 0 else ['lichtblick', 'mcap']
    for app in apps:
        output = args.output/f'{app}-{repetition}.json'
        print(f'run {repetition}: {app}', flush=True)
        subprocess.run([sys.executable, str(Path(__file__).with_name('measure_viewer_pilot.py')),
                        '--app', app, '--data', str(args.data.resolve()), '--output', str(output)], check=True, timeout=120)
        row = json.loads(output.read_text())
        if row['status'] != 'ok':
            raise RuntimeError(row.get('error', 'probe failed'))
        rows.append(row)
lines = ['# Web viewer pilot', '', 'Three fresh Chrome processes/profiles per app. Medians, MiB = 2^20 bytes.', '',
         '| App | Before open PSS MiB | Before play PSS MiB | Peak sampled playback PSS MiB | Settled PSS MiB | Cursor run s |',
         '|---|---:|---:|---:|---:|---:|']
for app in ['mcap', 'lichtblick']:
    group = [r for r in rows if r['app'] == app]
    median = lambda f: statistics.median(f(r) for r in group)
    values = [median(lambda r: r[k]['pss_kib']/1024) for k in ['before_open', 'before_play']]
    values += [median(lambda r: r['peak_sampled_pss_kib']/1024), median(lambda r: r['settled']['pss_kib']/1024),
               median(lambda r: r['cursor_run_ms']/1000)]
    lines.append('| '+app+' | '+' | '.join(f'{v:.2f}' for v in values)+' |')
lines += ['', 'Cursor completion is not displayed FPS. UI-ready times have different predicates; see raw runs.',
          '200 ms target sampling can miss peaks. CPU/I/O live-process counters are diagnostic only.',
          'Dataset includes LaserScan: mcap also materializes it; Lichtblick subscribes only camera 0.',
          'mcap Scene is hidden via display:none (zero CSS size skips surface rendering); initial GPU resources remain.',
          'This pilot does not measure selective-topic savings, long-history growth, cold disk, seek latency or hardware GPU throughput.']
(args.output/'summary.md').write_text('\n'.join(lines)+'\n')
print('\n'.join(lines), flush=True)
