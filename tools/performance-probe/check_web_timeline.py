#!/usr/bin/env python3
"""Functional timeline event regressions in isolated software Chrome; not a benchmark."""
import argparse
from contextlib import nullcontext
import json
from pathlib import Path
import time

from benchmark_browser import free_port, serve
from measure_web_camera_stress import browser


def wait(page, expression, label):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        if page.evaluate(expression):
            return
        time.sleep(.05)
    raise RuntimeError(f"{label}: {page.evaluate('document.body.innerText')}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--url')
    parser.add_argument('--dist', type=Path, default=Path('apps/viewer-web/dist'))
    args = parser.parse_args()
    if args.output.exists():
        parser.error('a new output path is required')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    result = {'status': 'failed', 'scope': __doc__}
    try:
        with nullcontext(args.url) if args.url else serve(args.dist.resolve()) as url:
            result['url'] = url
            with browser(free_port()) as (_, page, _):
                page.call('Page.navigate', {'url': url})
                wait(page, '!!window.wasmBindings', 'Wasm startup')
                root = page.call('DOM.getDocument')['root']['nodeId']
                node = page.call('DOM.querySelector', {'nodeId': root, 'selector': '#mcap-file'})['nodeId']
                page.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(args.data.resolve())]})
                ready = "document.querySelector('#status').textContent.startsWith('Ready')"
                wait(page, ready, 'recording open')
                page.evaluate("""window.timelineReads=0;
                  const read=Blob.prototype.arrayBuffer;
                  Blob.prototype.arrayBuffer=function(){timelineReads++;return read.call(this)};
                  const slider=document.querySelector('#timeline');
                  slider.value='.3';slider.dispatchEvent(new Event('input',{bubbles:true}));""")
                wait(page, ready + " && Math.abs(Number(document.querySelector('#timeline').value)-.3)<.001", 'seek')
                # Allow the required post-seek window to finish before observing duplicate work.
                deadline = time.monotonic() + 25
                previous = None
                stable = time.monotonic()
                while True:
                    count = page.evaluate('timelineReads')
                    if count != previous:
                        previous, stable = count, time.monotonic()
                    if time.monotonic() - stable > .4:
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError('file reads did not settle')
                    time.sleep(.05)
                page.evaluate("document.querySelector('#timeline').dispatchEvent(new Event('change',{bubbles:true}))")
                time.sleep(.4)
                result['unchanged_change_extra_reads'] = page.evaluate('timelineReads') - previous
                assert result['unchanged_change_extra_reads'] == 0, result
                page.evaluate("""(()=>{const s=document.querySelector('#timeline');
                  s.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerId:11,button:0}));
                  s.value='.6';})()""")
                time.sleep(.1)
                page.evaluate("document.body.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,pointerId:22}))")
                time.sleep(.1)
                value = "Number(document.querySelector('#timeline').value)"
                result['slider_after_unrelated_pointer_up'] = page.evaluate(value)
                assert abs(result['slider_after_unrelated_pointer_up'] - .6) < .001, result
                page.evaluate("document.querySelector('#timeline').dispatchEvent(new PointerEvent('pointercancel',{bubbles:true,pointerId:11}))")
                wait(page, f'Math.abs({value}-.3)<.001', 'active pointer cancellation')
                page.evaluate("""(()=>{const s=document.querySelector('#timeline');
                  s.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerId:33,button:0}));
                  s.value='.6';window.dispatchEvent(new Event('blur'));})()""")
                wait(page, f'Math.abs({value}-.3)<.001', 'window blur')
                result['status'] = 'ok'
    except Exception as error:
        result['error'] = repr(error)
        raise
    finally:
        args.output.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result))


if __name__ == '__main__':
    main()
