#!/usr/bin/env python3
"""Exercise the production camera dropdown through Chrome/CDP with real MCAP fixtures."""
import argparse
import base64
import json
from pathlib import Path
import re
import time

from benchmark_browser import browser, serve

ROOT = Path(__file__).resolve().parents[2]


def check(args):
    args.output.mkdir(parents=True, exist_ok=False)
    results = []
    with serve(args.dist) as url, browser(
        args.gpu, args.output / 'chrome.log', wake_display=args.wake_display,
        vulkan_icd=args.vulkan_icd,
    ) as (_, page, environment):
        page.call('Page.navigate', {'url': url})

        def wait(predicate, timeout=45):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                if predicate():
                    return
                time.sleep(.05)
            raise RuntimeError('Camera UI timeout: ' + json.dumps(state()))

        def state():
            return page.evaluate('''(()=>{
                const select=document.querySelector('#camera-topic');
                const canvas=document.querySelector('#camera');
                return {disabled:select?.disabled,options:Array.from(select?.options||[],o=>o.textContent),
                  selected:select?.selectedOptions[0]?.textContent,
                  label:document.querySelector('#camera-focus-label')?.textContent,
                  status:document.querySelector('#status')?.textContent,
                  pixel:canvas?.width ? Array.from(canvas.getContext('2d').getImageData(
                    Math.floor(canvas.width/2),Math.floor(canvas.height/2),1,1).data) : null,
                  cursor:document.querySelector('#timeline')?.value};
            })()''')

        def open_file(path, cameras, seek=False):
            node = page.call('DOM.querySelector', {
                'nodeId': page.call('DOM.getDocument')['root']['nodeId'], 'selector': '#mcap-file',
            })['nodeId']
            page.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(path.resolve(strict=True))]})
            wait(lambda: len(state()['options']) == cameras and (state()['status'] or '').startswith('Ready'))
            assert not state()['disabled']
            if seek:
                page.evaluate('''{const slider=document.querySelector('#timeline');slider.value='0.5';
                    slider.dispatchEvent(new Event('input',{bubbles:true}));}''')
            wait(lambda: state()['pixel'] is not None and state()['pixel'][3] > 0)

        def choose(topic):
            page.evaluate('''(()=>{
                const select=document.querySelector('#camera-topic');
                const option=Array.from(select.options).find(o=>o.textContent === %s);
                if(!option) throw Error('Missing image topic');
                select.value=option.value;select.dispatchEvent(new Event('change',{bubbles:true}));
            })()''' % json.dumps(topic))
            wait(lambda: state()['selected'] == topic and topic in (state()['label'] or ''))

        def capture(name):
            data = page.call('Page.captureScreenshot', {'format': 'png'})['data']
            (args.output / name).write_bytes(base64.b64decode(data))

        wait(lambda: page.evaluate('!!document.querySelector("#camera-topic")'))
        assert state()['disabled']
        wait(lambda: page.evaluate('document.querySelector("#bev-status")?.textContent.includes("ready")'))
        open_file(ROOT / 'tests/fixtures/camera-formats/camera_formats.mcap', 7)
        assert set(state()['options']) == {'/camera/' + name for name in
                                          ['rgb8', 'bgr8', 'rgba8', 'bgra8', 'mono8', 'png', 'jpeg']}
        page.evaluate('''{const slider=document.querySelector('#timeline');slider.value='0.5';
            slider.dispatchEvent(new Event('input',{bubbles:true}));}''')
        wait(lambda: .499 <= float(state()['cursor']) <= .501)
        for topic, expected in [('/camera/mono8', [62, 62, 62]), ('/camera/png', [62, 80, 160]),
                                ('/camera/rgb8', [62, 80, 160]), ('/camera/jpeg', [62, 80, 160])]:
            choose(topic)
            wait(lambda: state()['pixel'] is not None and
                 all(abs(a - b) <= 3 for a, b in zip(state()['pixel'][:3], expected)))
            assert .499 <= float(state()['cursor']) <= .501
            assert page.evaluate('document.querySelector("#play").textContent') == 'Play'
            results.append(state())
        capture('web-camera-topic.png')

        # Same numeric CameraIds now identify different topics. Options and pixels must reset.
        open_file(ROOT / 'tests/fixtures/camera-formats/camera_alpha.mcap', 3)
        assert set(state()['options']) == {'/camera/rgba8', '/camera/bgra8', '/camera/png'}
        results.append(state())

        # This fixture also contains Path, Scan, Odom and TF. None belongs in the image menu.
        open_file(ROOT / 'tests/fixtures/camera-jpeg/camera_7_5s.mcap', 7, seek=True)
        assert all(topic.endswith('/image/compressed') for topic in state()['options'])
        choose('/camera/rear/image/compressed')
        wait(lambda: state()['pixel'] is not None and state()['pixel'][3] > 0)
        wait(lambda: all(re.search(pattern, state()['status'] or '') for pattern in
                         [r'path [1-9][0-9]* pts', r'scan [1-9][0-9]* pts', r'\b0 errors\b']))
        assert 'TF missing' not in page.evaluate('document.querySelector("#scene-info").textContent')
        results.append(state())
        capture('web-existing-scene.png')
        if args.recorded:
            open_file(args.recorded, 1, seek=True)
            assert state()['options'] == ['/cam/camera/image_raw']
            results.append(state())
        (args.output / 'result.json').write_text(json.dumps({
            'status': 'ok', 'environment': environment, 'checks': results,
        }, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, default=ROOT / 'apps/viewer-web/dist')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--gpu', choices=['hardware', 'software'], default='hardware')
    parser.add_argument('--wake-display', action='store_true')
    parser.add_argument('--vulkan-icd', type=Path)
    parser.add_argument('--recorded', type=Path, help='optional recorded /cam/camera/image_raw MCAP')
    check(parser.parse_args())
