#!/usr/bin/env python3
"""Pilot GUI comparison: one 4K camera, focus 960x540 + thumbnail 112x63.
A fresh Chrome process/profile per run; local file input; client process-tree PSS/CPU.
This checks cursor progression, not presentation FPS or per-image latency.
"""
import argparse
import base64
import hashlib
import json
import os
import socket
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request

from browser_cdp import CDP

ROOT = Path(__file__).resolve().parents[2]


def process_sample(root):
    pids, pending = set(), [root]
    while pending:
        pid = pending.pop()
        if pid in pids:
            continue
        pids.add(pid)
        try:
            pending.extend(map(int, Path(f'/proc/{pid}/task/{pid}/children').read_text().split()))
        except (FileNotFoundError, ProcessLookupError):
            pass
    pss = rss = ticks = rchar = read_bytes = 0
    for pid in pids:
        try:
            values = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
            ticks += int(values[11]) + int(values[12])
            memory = {line.split(':')[0]: int(line.split()[1]) for line in
                      Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines()[1:] if ':' in line}
            pss += memory['Pss']
            rss += memory['Rss']
            io = dict(line.split(': ') for line in Path(f'/proc/{pid}/io').read_text().splitlines())
            rchar += int(io['rchar'])
            read_bytes += int(io['read_bytes'])
        except (FileNotFoundError, ProcessLookupError):
            pass
    return {'pss_kib': pss, 'summed_rss_kib': rss, 'cpu_seconds_live_processes': ticks / os.sysconf('SC_CLK_TCK'),
            'rchar_live_processes': rchar, 'storage_read_bytes_live_processes': read_bytes, 'pids': sorted(pids)}


STATE = {
    'mcap': '''({ready:document.querySelectorAll('.camera-card').length===1 &&
        document.querySelector('#status').textContent.startsWith('Ready'),
        text:document.querySelector('#status').textContent,
        performance:document.querySelector('#performance').textContent,
        position:Number(document.querySelector('#timeline').value),
        playing:document.querySelector('#play').textContent.includes('Pause')})''',
    'lichtblick': '''({ready:document.querySelectorAll('canvas').length===2 &&
        !!document.querySelector('[data-testid="play-button"]') &&
        !document.body.innerText.includes('Waiting for image'),text:document.body.innerText,
        position:document.querySelector('[data-testid="PlaybackTime-text"] input')?.value,
        playing:document.querySelector('[data-testid="play-button"]')?.title==='Pause'})''',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', choices=STATE, required=True)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--port', type=int, default=9238)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('output must be new')
    # Refuse an occupied CDP port rather than accidentally driving another browser.
    with socket.socket() as port_check:
        port_check.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        port_check.bind(('127.0.0.1', args.port))
    data = args.data.resolve()
    # Hashing explicitly warms the file cache: these are cache-allowed results.
    with data.open('rb') as source:
        sha = hashlib.file_digest(source, 'sha256').hexdigest()
    url = f'http://127.0.0.1:{8094 if args.app == "mcap" else 8095}/'
    result = {'app': args.app, 'dataset': str(data), 'dataset_bytes': data.stat().st_size,
              'dataset_sha256': sha, 'url': url, 'viewport': [1920, 1080], 'dpr': 1,
              'focus': [960, 540], 'thumbnail': [112, 63], 'samples': [],
              'sampling': '200 ms target; observed intervals stored; PSS includes dedicated Chrome tree',
              'cache': 'cache-allowed; SHA256 read immediately before each run; fresh profile/process',
              'sources_sha256': {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                                 for name in ['measure_viewer_pilot.py', 'browser_cdp.py']},
              'git_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()}
    with tempfile.TemporaryDirectory(prefix='mcap-viewer-pilot-') as profile:
        command = ['google-chrome', '--headless', '--no-sandbox', '--enable-unsafe-webgpu',
                   '--enable-unsafe-swiftshader', '--use-gl=angle', '--use-angle=swiftshader',
                   f'--remote-debugging-port={args.port}', f'--user-data-dir={profile}', 'about:blank']
        result['chrome_command'] = command
        with tempfile.TemporaryFile() as log:
            chrome = subprocess.Popen(command, stdout=log, stderr=log)
            cdp = browser = None
            try:
                for _ in range(200):
                    try:
                        version = json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json/version', timeout=1))
                        pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json/list', timeout=1))
                        break
                    except OSError:
                        if chrome.poll() is not None:
                            raise RuntimeError('Chrome exited')
                        time.sleep(.05)
                else:
                    raise RuntimeError('Chrome did not start')
                result['browser_version'] = version
                browser = CDP(version['webSocketDebuggerUrl'])
                result['gpu'] = browser.call('SystemInfo.getInfo')
                cdp = CDP(next(p['webSocketDebuggerUrl'] for p in pages if p['type'] == 'page'))
                cdp.call('Page.enable')
                cdp.call('Emulation.setDeviceMetricsOverride', {'width': 1920, 'height': 1080, 'deviceScaleFactor': 1, 'mobile': False})
                cdp.call('Page.addScriptToEvaluateOnNewDocument', {'source': '''window.pilotErrors=[];
                    addEventListener('error',e=>pilotErrors.push(e.message));
                    addEventListener('unhandledrejection',e=>pilotErrors.push(String(e.reason)));'''})
                cdp.call('Page.navigate', {'url': url})
                file_selector = '#mcap-file' if args.app == 'mcap' else 'input[type=file]'
                for _ in range(200):
                    if cdp.evaluate(f'!!document.querySelector({json.dumps(file_selector)})'):
                        break
                    time.sleep(.05)
                else:
                    raise RuntimeError('app did not initialize')
                if args.app == 'mcap':
                    cdp.evaluate('''{let s=document.createElement('style');s.textContent=
                      '.camera-panel{position:fixed;top:140px;left:330px;width:1000px;height:760px;z-index:10}' +
                      '.camera-frame{width:960px!important;height:540px!important}' +
                      '.scene-panel{display:none}';document.head.append(s);}''')
                else:
                    # The source modal hides the layout until a file opens.
                    pass
                time.sleep(.5)
                result['before_open'] = process_sample(chrome.pid)
                document = cdp.call('DOM.getDocument')['root']['nodeId']
                node = cdp.call('DOM.querySelector', {'nodeId': document, 'selector': file_selector})['nodeId']
                start = time.monotonic()
                cdp.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(data)]})
                for _ in range(300):
                    state = cdp.evaluate(STATE[args.app])
                    if state['ready']:
                        break
                    time.sleep(.05)
                else:
                    raise RuntimeError(f'file did not open: {state}')
                result['ready_ui_ms'] = (time.monotonic() - start) * 1000
                if args.app == 'lichtblick':
                    time.sleep(2)
                    cdp.evaluate('''[...document.querySelectorAll('canvas')].forEach((c,i)=>{
                        c.parentElement.style.width=(i?112:960)+'px';
                        c.parentElement.style.height=(i?63:540)+'px';});''')
                time.sleep(2)
                result['opened_state'] = cdp.evaluate(STATE[args.app])
                result['canvases'] = cdp.evaluate('''[...document.querySelectorAll('canvas')].map(c=>({
                  id:c.id,width:c.width,height:c.height,css:[c.clientWidth,c.clientHeight],
                  rect:[c.getBoundingClientRect().x,c.getBoundingClientRect().y]}))''')
                expected = [[960, 540], [112, 63]]
                images = [c for c in result['canvases'] if args.app != 'mcap' or c['id'] != 'bev']
                if [[c['width'], c['height']] for c in images] != expected:
                    raise RuntimeError(f'image dimensions differ: {images}')
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.with_suffix('.png').write_bytes(base64.b64decode(cdp.call('Page.captureScreenshot', {'format': 'png'})['data']))
                result['before_play'] = process_sample(chrome.pid)
                button = '#play' if args.app == 'mcap' else '[data-testid="play-button"]'
                start = time.monotonic()
                cdp.evaluate(f'document.querySelector({json.dumps(button)}).click()')
                seen_playing = False
                while True:
                    elapsed = time.monotonic() - start
                    state = cdp.evaluate(STATE[args.app])
                    sample = process_sample(chrome.pid)
                    result['samples'].append({'wall_seconds': elapsed, 'state': state, **sample})
                    seen_playing |= state['playing']
                    if seen_playing and (not state['playing'] or (args.app == 'mcap' and state['position'] >= 1)):
                        result['cursor_run_ms'] = elapsed * 1000
                        result['end_state'] = state
                        if state['playing']:
                            cdp.evaluate(f'document.querySelector({json.dumps(button)}).click()')
                        break
                    if elapsed > 60:
                        raise RuntimeError(f'playback did not finish: {state}')
                    time.sleep(.2)
                result['after_play'] = process_sample(chrome.pid)
                time.sleep(3)
                result['settled'] = process_sample(chrome.pid)
                result['errors'] = cdp.evaluate('window.pilotErrors')
                if result['errors']:
                    raise RuntimeError(result['errors'])
                result['peak_sampled_pss_kib'] = max(s['pss_kib'] for s in result['samples'])
                result['status'] = 'ok'
            except Exception as e:
                result['status'] = 'error'
                result['error'] = repr(e)
                raise
            finally:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(result, indent=2) + '\n')
                if cdp:
                    cdp.close()
                if browser:
                    try:
                        browser.call('Browser.close')
                    except (RuntimeError, OSError):
                        pass
                    browser.close()
                try:
                    chrome.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    chrome.terminate()
                    chrome.wait(timeout=10)
    print(json.dumps({k: result[k] for k in ['app', 'ready_ui_ms', 'cursor_run_ms', 'peak_sampled_pss_kib', 'status']}))


if __name__ == '__main__':
    main()
