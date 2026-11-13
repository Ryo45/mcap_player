#!/usr/bin/env python3
"""Exercise the experimental egui canvas on a real GPU; not a comparative benchmark."""
import argparse
import base64
import json
from pathlib import Path
import time

from benchmark_browser import browser, serve


def wait(page, expression, label, timeout=25):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if page.evaluate('failures.length'):
            raise RuntimeError(page.evaluate('failures'))
        if page.evaluate(expression):
            return
        time.sleep(.05)
    raise RuntimeError(f'{label}: {page.evaluate("document.body.innerText")}')


def click(page, x, y):
    for kind in ('mousePressed', 'mouseReleased'):
        page.call('Input.dispatchMouseEvent', {'type': kind, 'x': x, 'y': y,
                                            'button': 'left', 'clickCount': 1})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--replacement', type=Path, required=True)
    parser.add_argument('--dist', type=Path, default=Path('target/web-egui-ui/web'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--wake-display', action='store_true')
    parser.add_argument('--vulkan-icd', type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'failed', 'scope': __doc__, 'data': str(args.data)}
    snapshot = 'JSON.parse(wasmBindings.egui_status())'
    try:
        with serve(args.dist.resolve()) as url, browser(
                'hardware', args.output/'chrome.log', wake_display=args.wake_display,
                vulkan_icd=args.vulkan_icd) as (_, page, environment):
            report['environment'] = environment
            page.call('Page.addScriptToEvaluateOnNewDocument', {'source': """
                window.failures=[]; const error=console.error;
                console.error=(...args)=>{failures.push(args.map(String).join(' '));error(...args)};
                addEventListener('error',e=>failures.push(e.message));
                addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));
            """})
            page.call('Page.navigate', {'url': url})
            wait(page, '!!window.wasmBindings && !document.querySelector("#startup")', 'egui startup')

            def status():
                return page.evaluate(snapshot)

            def open_file(path):
                root = page.call('DOM.getDocument')['root']['nodeId']
                node = page.call('DOM.querySelector', {'nodeId': root, 'selector': '#mcap-file'})['nodeId']
                page.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(path.resolve())]})
                wait(page, f'{snapshot}.source=={json.dumps(path.name)} && {snapshot}.load=="Ready"', 'open recording')

            def play():
                r = status()['play']
                click(page, (r[0]+r[2])/2, (r[1]+r[3])/2)

            def seek(ratio, touch=False):
                r = status()['timeline']
                # egui's response includes the trailing label (55 logical pixels).
                width = r[2]-r[0]-55
                x, y = r[0]+width*ratio, (r[1]+r[3])/2
                if touch:
                    page.call('Input.dispatchTouchEvent', {'type': 'touchStart', 'touchPoints': [{'x': x, 'y': y}]})
                    page.call('Input.dispatchTouchEvent', {'type': 'touchEnd', 'touchPoints': []})
                else:
                    click(page, x, y)
                wait(page, f'{snapshot}.load=="Ready"', 'seek ready')

            open_file(args.data)
            play()
            wait(page, f'{snapshot}.uploads>=10', 'camera GPU uploads')
            play()
            wait(page, f'!{snapshot}.playing', 'pause')
            # Select a numeric topic through actual egui text/pointer input.
            click(page, 100, 57)
            page.call('Input.insertText', {'text': '/vehicle/speed_mps'})
            time.sleep(.2)
            click(page, 100, 78)
            wait(page, f'{snapshot}.has_message', 'shared lazy Inspector')
            time.sleep(.4)
            settled = status()['uploads']
            time.sleep(.4)
            report['idle_extra_camera_uploads'] = status()['uploads'] - settled
            assert report['idle_extra_camera_uploads'] == 0
            report['desktop'] = status()
            (args.output/'desktop.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot', {'format': 'png'})['data']))
            seek(.98)
            play()
            wait(page, f'!{snapshot}.playing && BigInt({snapshot}.cursor)>BigInt({snapshot}.end)-100000000n', 'natural EOF')
            page.call('Emulation.setDeviceMetricsOverride', {'width': 390, 'height': 844, 'deviceScaleFactor': 1, 'mobile': True})
            page.call('Emulation.setTouchEmulationEnabled', {'enabled': True, 'maxTouchPoints': 2})
            wait(page, f'{snapshot}.timeline[2]<400', 'mobile layout')
            seek(.3, touch=True)
            time.sleep(.5)
            current = status()
            ratio = (int(current['cursor'])-int(current['start']))/(int(current['end'])-int(current['start']))
            assert .15 < ratio < .5, current
            report['mobile_seek_after_eof'] = current
            play()
            wait(page, f'{snapshot}.playing', 'replay after touch seek')
            before = int(status()['cursor'])
            wait(page, f'BigInt({snapshot}.cursor)>{before}n+100000000n', 'replay progresses')
            play()
            open_file(args.replacement)
            current = status()
            assert not current['has_message'] and not current['camera_texture'], current
            report['replacement'] = current
            assert not page.evaluate('failures'), page.evaluate('failures')
            report['status'] = 'ok'
    finally:
        (args.output/'result.json').write_text(json.dumps(report, indent=2)+'\n')
        print(json.dumps({key: value for key, value in report.items() if key != 'environment'}))


if __name__ == '__main__':
    main()
