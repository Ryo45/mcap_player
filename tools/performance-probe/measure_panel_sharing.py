#!/usr/bin/env python3
"""Real-GPU panel-sharing regression probe. CPU/PSS and actual Web uploads, not renderer FPS."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time

from benchmark_browser import browser, serve
from check_native_gpu import X11
from check_web_egui import click, wait
from compare_native_desktop import capture, window_for
from measure_viewer_pilot import process_sample


def sample_interval(pid, seconds, observer=None):
    before = process_sample(pid)
    started = time.monotonic()
    samples = [before]
    observations = []
    while time.monotonic()-started < seconds:
        time.sleep(.2)
        samples.append(process_sample(pid))
        if observer:
            observations.append(observer())
    elapsed = time.monotonic()-started
    return {
        'elapsed_seconds': elapsed,
        'cpu_core_seconds_per_second': (samples[-1]['cpu_seconds_live_processes']-before['cpu_seconds_live_processes'])/elapsed,
        'pss_mib_median': statistics.median(s['pss_kib']/1024 for s in samples),
        'pss_mib_peak': max(s['pss_kib']/1024 for s in samples),
        'samples': samples,
        'observations': observations,
    }


def native(binary, data, output, seconds):
    env = os.environ | {'DISPLAY': ':1', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority',
                        'VK_DRIVER_FILES': '/usr/share/vulkan/icd.d/nvidia_icd.json',
                        'RUST_LOG': 'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info'}
    initial_display = subprocess.check_output(['xset','q'],env=env,text=True)
    subprocess.run(['xset','dpms','force','on'],env=env,check=True)
    subprocess.run(['xset','s','reset'],env=env,check=True)
    command = [str(binary.resolve()), '--mcap', str(data.resolve()), '--layout', 'standard']
    with (output/'native.log').open('w') as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        x11 = None
        try:
            deadline = time.monotonic()+20
            window = None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError('Native exited before initialization')
                window = window_for(process.pid, env)
                if window:
                    break
                time.sleep(.1)
            if not window:
                raise RuntimeError('Native window not found')
            x11 = X11(':1')
            x11.resize(window, 1920, 1080)
            time.sleep(1)
            # Native starts playback automatically in ViewerSession::from_playback.
            # Do not toggle the first focused Play/Pause button here.
            first = capture(x11, window, [0, 0, 1920, 980])[0]
            result = sample_interval(process.pid, seconds)
            last = capture(x11, window, [0, 0, 1920, 980])[0]
            result['window_pixels_changed'] = hashlib.sha256(first).digest() != hashlib.sha256(last).digest()
            x11.capture(window, 1920, 1080, output/'native.png')
            text = (output/'native.log').read_text()
            assert 'NVIDIA' in text and 'Vulkan' in text, 'hardware adapter not confirmed'
            assert result['window_pixels_changed'], 'no observable playback change; inspect screenshot'
            result['display_initial'] = initial_display
            assert 'Monitor is On' in subprocess.check_output(['xset','q'],env=env,text=True), 'display slept during measurement'
            result['command'] = command
            return result
        finally:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=15)
            if x11:
                x11.x.XCloseDisplay(x11.display)
            if 'Monitor is Off' in initial_display:
                subprocess.run(['xset','dpms','force','off'],env=env,check=True)


def web(dist, data, output, seconds, plot):
    snapshot = 'JSON.parse(wasmBindings.egui_status())'
    with serve(dist.resolve()) as url, browser('hardware', output/'chrome.log', wake_display=True,
            vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (process, page, environment):
        page.call('Page.addScriptToEvaluateOnNewDocument', {'source': """
            window.failures=[];const error=console.error;
            console.error=(...a)=>{failures.push(a.map(String).join(' '));error(...a)};
            addEventListener('error',e=>failures.push(e.message));
            addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));
            window.textureSizes={};
            if(globalThis.GPUDevice){const create=GPUDevice.prototype.createTexture;
              GPUDevice.prototype.createTexture=function(d){textureSizes[d.label||'']=d.size;return create.call(this,d)}};
        """})
        page.call('Page.navigate', {'url': url})
        wait(page, '!!window.wasmBindings && !document.querySelector("#startup")', 'startup')
        root = page.call('DOM.getDocument')['root']['nodeId']
        node = page.call('DOM.querySelector', {'nodeId': root, 'selector': '#mcap-file'})['nodeId']
        opened = time.monotonic()
        page.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(data.resolve())]})
        wait(page, f'{snapshot}.source && {snapshot}.load=="Ready"', 'open')
        ready_ms = (time.monotonic()-opened)*1000
        if plot:
            r = page.evaluate(snapshot)['plot_checkbox']
            start = time.monotonic()
            click(page, (r[0]+r[2])/2, (r[1]+r[3])/2)
            wait(page, f'!{snapshot}.plot_loading && ({snapshot}.plot_points>0 || {snapshot}.plot_error)', 'plot')
            query_ms = (time.monotonic()-start)*1000
            assert not page.evaluate(snapshot)['plot_error']
        else:
            query_ms = None
        r = page.evaluate(snapshot)['play']
        click(page, (r[0]+r[2])/2, (r[1]+r[3])/2)
        wait(page, f'{snapshot}.uploads>0', 'camera upload')
        time.sleep(.2)
        before = page.evaluate(snapshot)
        result = sample_interval(process.pid, seconds, lambda: page.evaluate(snapshot))
        after = page.evaluate(snapshot)
        result.update(environment=environment, before=before, after=after, ready_ms=ready_ms, query_ms=query_ms,
                      texture_sizes=page.evaluate('textureSizes'), errors=page.evaluate('failures'))
        result['uploads_per_second'] = (after['uploads']-before['uploads'])/result['elapsed_seconds']
        result['cursor_seconds_per_second'] = (int(after['cursor'])-int(before['cursor']))/1e9/result['elapsed_seconds']
        assert result['cursor_seconds_per_second'] > .8, 'playback stalled or EOF during measurement'
        assert not result['errors'] and not after['error'], result
        (output/'web.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot', {'format': 'png'})['data']))
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kind', choices=['native', 'web'], required=True)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seconds', type=float, default=5)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--plot', action='store_true', help='Enable the independent speed Plot for after runs only')
    args = parser.parse_args()
    if not 1 <= args.seconds <= 7 or not 1 <= args.repeats <= 5:
        parser.error('bounded runs require 1..7 seconds and 1..5 repetitions')
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'kind': args.kind, 'data': str(args.data), 'data_sha256': hashlib.file_digest(args.data.open('rb'), 'sha256').hexdigest(),
              'plot': args.plot, 'runs': []}
    try:
        for repeat in range(args.repeats):
            for label in (['before', 'after'] if repeat % 2 == 0 else ['after', 'before']):
                output = args.output/f'{repeat}-{label}'
                output.mkdir()
                source = getattr(args, label)
                result = native(source, args.data, output, args.seconds) if args.kind == 'native' else web(source, args.data, output, args.seconds, args.plot and label == 'after')
                report['runs'].append({'variant': label, 'repeat': repeat, **result})
                print(label, {k: result[k] for k in ['cpu_core_seconds_per_second', 'pss_mib_median']}, flush=True)
        report['status'] = 'ok'
    finally:
        (args.output/'result.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
