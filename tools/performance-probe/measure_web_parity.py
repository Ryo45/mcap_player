#!/usr/bin/env python3
"""Compare main DOM and shared egui UI with matched camera/thumbnail/Scene rasters.

Use prepare_web_parity.py copies, release builds, an idle hardware GPU and short recordings.
Reports process-tree CPU/PSS and distinct camera imports; these are not compositor FPS.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import statistics
import time
from benchmark_browser import browser, serve
from check_web_egui import click, wait
from measure_panel_sharing import sample_interval


def run(kind, dist, data, output, seconds):
    expr = 'JSON.parse(wasmBindings.'+('parity_status' if kind == 'main' else 'egui_status')+'())'
    with serve(dist.resolve()) as url, browser('hardware', output/'chrome.log', wake_display=True,
            vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (process, page, environment):
        page.call('Page.addScriptToEvaluateOnNewDocument', {'source': """
            window.failures=[];const error=console.error;
            console.error=(...a)=>{failures.push(a.map(String).join(' '));error(...a)};
            addEventListener('error',e=>failures.push(e.message));
            addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));
            window.textureSizes={};if(globalThis.GPUDevice){const create=GPUDevice.prototype.createTexture;
                GPUDevice.prototype.createTexture=function(d){textureSizes[d.label||'']=d.size;return create.call(this,d)}};
        """})
        page.call('Page.navigate', {'url': url})
        wait(page, '!!window.wasmBindings'+(' && !document.querySelector("#startup")' if kind == 'egui' else ''), 'startup')
        root = page.call('DOM.getDocument')['root']['nodeId']
        node = page.call('DOM.querySelector', {'nodeId':root, 'selector':'#mcap-file'})['nodeId']
        start = time.monotonic()
        page.call('DOM.setFileInputFiles', {'nodeId':node, 'files':[str(data.resolve())]})
        wait(page, f'{expr}.load=="Ready"', 'source open')
        ready_ms = (time.monotonic()-start)*1000
        if kind == 'main':
            page.evaluate('document.querySelector("#play").click()')
        else:
            r = page.evaluate(expr)['play']
            click(page, (r[0]+r[2])/2, (r[1]+r[3])/2)
        wait(page, f'{expr}.playing && {expr}.camera_count>=7', 'all cameras visible')
        time.sleep(.2)
        before = page.evaluate(expr)
        result = sample_interval(process.pid, seconds)
        after = page.evaluate(expr)
        elapsed = result['elapsed_seconds']
        counts = {key:(value-before['uploads_by_camera'].get(key, 0))/elapsed
                  for key,value in after['uploads_by_camera'].items()}
        focus = str(after['camera'])
        result.update(environment=environment, ready_ms=ready_ms, before=before, after=after,
            focus_hz=counts[focus], background_min_hz=min(v for key,v in counts.items() if key != focus),
            camera_hz=counts, texture_sizes=page.evaluate('textureSizes'), errors=page.evaluate('failures'),
            cursor_rate=(int(after['cursor'])-int(before['cursor']))/1e9/elapsed)
        assert not result['errors'], result['errors']
        assert after['playing'] and result['cursor_rate'] > .8, 'stalled or reached EOF'
        assert not after['error'], after['error']
        result['camera_target_met'] = result['focus_hz'] > 8 and result['background_min_hz'] > 3
        (output/'web.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot', {'format':'png'})['data']))
        return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('main', 'egui', 'data', 'output'):
        p.add_argument('--'+name, type=Path, required=True)
    p.add_argument('--seconds', type=float, default=5)
    p.add_argument('--repeats', type=int, default=3)
    args = p.parse_args()
    if not 1 <= args.seconds <= 7 or not 1 <= args.repeats <= 5:
        p.error('Use 1..7 seconds and 1..5 repetitions')
    args.output.mkdir(parents=True, exist_ok=False)
    with args.data.open('rb') as data:
        digest = hashlib.file_digest(data, 'sha256').hexdigest()
    report = {'data':str(args.data), 'sha256':digest, 'runs':[], 'status':'failed'}
    try:
        for repeat in range(args.repeats):
            for kind in (['main', 'egui'] if repeat % 2 == 0 else ['egui', 'main']):
                out = args.output/f'{repeat}-{kind}'
                out.mkdir()
                result = run(kind, getattr(args, kind), args.data, out, args.seconds)
                report['runs'].append(dict(variant=kind, repeat=repeat, **result))
                print(kind, {k:result[k] for k in ('cpu_core_seconds_per_second', 'pss_mib_median', 'focus_hz', 'background_min_hz')}, flush=True)
        report['summary'] = {kind:{key:statistics.median(r[key] for r in report['runs'] if r['variant']==kind)
            for key in ('cpu_core_seconds_per_second', 'pss_mib_median', 'focus_hz', 'background_min_hz', 'ready_ms')}
            for kind in ('main', 'egui')}
        report['status'] = 'ok'
    finally:
        (args.output/'result.json').write_text(json.dumps(report, indent=2)+'\n')


if __name__ == '__main__':
    main()
