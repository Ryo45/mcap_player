#!/usr/bin/env python3
"""Production Web recording benchmark. No dated report, source patch, or fixed port."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import threading
import time

from benchmark_browser import browser, serve
from measure_viewer_pilot import ROOT, process_sample


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def percentile(values, fraction):
    return sorted(values)[min(len(values) - 1, int((len(values) - 1) * fraction))] if values else None


def measure(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {
        'status': 'starting', 'dataset': str(args.data.resolve()), 'dataset_sha256': digest(args.data),
        'dataset_bytes': args.data.stat().st_size, 'duration_seconds': args.duration_seconds,
        'cache': 'cache allowed; full input SHA256 before process startup',
        'web_assets': {p.name: digest(p) for p in sorted(args.dist.iterdir()) if p.suffix in ['.html', '.js', '.wasm', '.css']},
        'sources': {name: digest(Path(__file__).with_name(name)) for name in [
            'measure_web_recording.py', 'web_recording_probe.js', 'benchmark_browser.py', 'browser_cdp.py', 'measure_viewer_pilot.py']},
        'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'samples': [], 'memory': [], 'seeks': [],
        'pss_limit_mib': args.pss_limit_mib, 'seconds': args.seconds,
        'observer': '16x9 canvas content hash on production composition; readback overhead included',
    }
    checkpoint = lambda: (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    checkpoint()
    try:
        with serve(args.dist) as url, browser(args.gpu, output / 'chrome.log', wake_display=args.wake_display, vulkan_icd=args.vulkan_icd) as (process, page, environment):
            result['environment'] = environment
            stop = threading.Event()
            monitor_started = time.monotonic()

            def monitor():
                while not stop.wait(.2):
                    try:
                        sample = process_sample(process.pid)
                        sample['wall_seconds'] = time.monotonic() - monitor_started
                        result['memory'].append(sample)
                        if sample['pss_kib'] > args.pss_limit_mib * 1024 or sample['wall_seconds'] > args.timeout:
                            result['status'] = 'external-pss-limit' if sample['pss_kib'] > args.pss_limit_mib * 1024 else 'external-timeout'
                            # Only the process group this harness created. Independent of CDP/main-thread responsiveness.
                            if process.poll() is None:
                                os.killpg(process.pid, 15)
                            return
                    except (OSError, ProcessLookupError):
                        pass

            watcher = threading.Thread(target=monitor, daemon=True)
            watcher.start()
            try:
                page.call('Page.addScriptToEvaluateOnNewDocument', {
                    'source': Path(__file__).with_name('web_recording_probe.js').read_text()})
                page.call('Page.navigate', {'url': url})

                def wait(predicate, seconds):
                    deadline = time.monotonic() + seconds
                    while True:
                        state = page.evaluate('window.recordingProbe?.state()')
                        if state:
                            result['samples'].append(state)
                            if state['errors']:
                                raise RuntimeError(state['errors'])
                            if state['visibility'] != 'visible':
                                raise RuntimeError('Benchmark page became hidden')
                            if predicate(state):
                                return state
                        if time.monotonic() > deadline:
                            raise RuntimeError(f'condition timed out: {state}')
                        time.sleep(.05)

                wait(lambda _: page.evaluate('document.querySelector("#bev-status")?.textContent.includes("ready")'), 30)
                if args.gpu == 'hardware':
                    adapter = page.evaluate('recordingProbe.adapter')
                    if not adapter or adapter.get('fallback') or any(
                            name in json.dumps(adapter).lower() for name in ['swiftshader', 'llvmpipe']):
                        raise RuntimeError(f'Hardware WebGPU adapter unavailable: {adapter}')
                page.evaluate("""{const style=document.createElement('style');style.textContent=
                    '.camera-panel{position:fixed;top:140px;left:330px;width:1200px;height:850px;z-index:10}' +
                    '.camera-frame{width:960px!important;height:540px!important}.scene-panel{display:none}';
                    document.head.append(style);} """)
                node = page.call('DOM.querySelector', {
                    'nodeId': page.call('DOM.getDocument')['root']['nodeId'], 'selector': '#mcap-file'})['nodeId']
                opened = time.monotonic()
                page.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(args.data.resolve())]})
                initial = wait(lambda s: (s['status'] or '').startswith('Ready') and s['cards'] == args.cameras and s['changes'] > 0, 60)
                result['open_ms'] = (time.monotonic() - opened) * 1000
                result['first_focus'] = initial
                initial = wait(lambda state: len(state['presented']) == args.cameras + 1, 15)
                result['initial'] = initial
                page.call('Page.bringToFront')
                page.evaluate("document.querySelector('#play').focus()")
                if any(s['width'] == 0 or s['height'] == 0 for s in initial['canvases']):
                    raise RuntimeError('Camera output has zero size')
                checkpoint()
                start = page.evaluate("(()=>{recordingProbe.phase='play';const now=performance.now();document.querySelector('#play').click();return now;})()")
                wall_start = time.monotonic()
                ended = wait(lambda s: s['cursor'] >= 1 or time.monotonic() - wall_start >= args.seconds, args.seconds + 45)
                end = page.evaluate("(()=>{recordingProbe.phase='drain';return performance.now();})()")
                # Pause only when still playing, avoiding the stale EOF button state.
                if ended['cursor'] < 1:
                    page.evaluate("document.querySelector('#play').click()")
                result.update(play_start_ms=start, play_end_ms=end, final=ended)
                for target in ([.7, .2, .6, .3] if args.seeks else []):
                    issued = page.evaluate(f"(()=>{{recordingProbe.phase='seek';recordingProbe.seekToken++;const t=document.querySelector('#timeline');t.value='{target}';t.dispatchEvent(new Event('input',{{bubbles:true}}));return {{time:performance.now(), token:recordingProbe.seekToken}};}})()")
                    settled = wait(lambda state: abs(state['cursor'] - target) < .0002 and page.evaluate(
                        f'recordingProbe.presentations.some(event=>event.id==="camera" && event.seekToken==={issued["token"]} && event.t>={issued["time"]})'), 30)
                    result['seeks'].append({'target': target, 'focus_changed_ms': settled['now'] - issued['time']})
                result['changes'] = page.evaluate('recordingProbe.changes')
                result['presentations'] = page.evaluate('recordingProbe.presentations')
                result['beats'] = page.evaluate('recordingProbe.beats')
                span = (end - start) / 1000
                changes = [e for e in result['changes'] if e['phase'] == 'play' and start <= e['t'] <= end]
                counts = {name: sum(e['id'] == name for e in changes) for name in ['camera'] + [f'camera-thumb-{i}' for i in range(1, args.cameras)]}
                beats = [e['t'] for e in result['beats'] if start <= e['t'] <= end]
                gaps = [b - a for a, b in zip([start] + beats, beats + [end])]
                result['metrics'] = {
                    'open_ms': result['open_ms'], 'focus_content_hz': counts['camera'] / span,
                    'background_min_content_hz': min((n / span for name, n in counts.items() if name != 'camera'), default=0),
                    'log_seconds_per_wall_second': ended['cursor'] * args.duration_seconds / span,
                    'heartbeat_max_ms': max(gaps), 'heartbeat_p95_ms': percentile(gaps, .95),
                    'peak_pss_mib': max(s['pss_kib'] for s in result['memory']) / 1024,
                    'seek_focus_ms': statistics.median(s['focus_changed_ms'] for s in result['seeks']) if result['seeks'] else None,
                }
                if '0 errors' not in ended['status'] or any(count == 0 for count in counts.values()):
                    raise RuntimeError('Playback produced errors or no visible content changes')
                result['status'] = 'ok'
            finally:
                stop.set()
                watcher.join(timeout=5)
    except Exception as error:
        if result['status'] in ['starting', 'ok']:
            result['status'] = 'failed'
        result['error'] = repr(error)
    finally:
        checkpoint()
    print(json.dumps({'output': str(output), 'status': result['status'], 'metrics': result.get('metrics'), 'error': result.get('error')}), flush=True)
    return result['status'] == 'ok'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--dist', type=Path, default=ROOT / 'apps/viewer-web/dist')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--gpu', choices=['hardware', 'software'], default='hardware')
    parser.add_argument('--wake-display', action='store_true')
    parser.add_argument('--vulkan-icd', type=Path, help='explicit hardware Vulkan driver; inherited if omitted')
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--duration-seconds', type=float, required=True, help='exact log-time span of fixture')
    parser.add_argument('--cameras', type=int, default=8)
    parser.add_argument('--seeks', action='store_true')
    parser.add_argument('--pss-limit-mib', type=int, default=4500)
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    if args.output.exists() or args.seconds <= 0 or args.duration_seconds <= 0 or args.cameras < 1 or args.pss_limit_mib <= 0 or args.timeout <= 0:
        parser.error('use a new output directory and positive limits')
    raise SystemExit(0 if measure(args) else 1)
