#!/usr/bin/env python3
"""Functional Local/Remote source replacement regression; no performance assertions."""
import argparse
import json
from pathlib import Path
import subprocess
import time
import urllib.request

from benchmark_browser import free_port, serve
from measure_web_camera_stress import browser


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True, help='indexed MCAP containing images')
    parser.add_argument('--server', type=Path, required=True, help='recording-server executable')
    parser.add_argument('--dist', type=Path, default=Path('apps/viewer-web/dist'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('new output path required')
    args.output.mkdir(parents=True)
    result = {'status': 'running', 'purpose': 'functional software rendering; no FPS claim', 'checks': []}

    def wait(operation, label):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            value = operation()
            if value:
                return value
            time.sleep(.05)
        raise RuntimeError(label)

    try:
        with serve(args.dist.resolve()) as url:
            port = free_port()
            config = args.output / 'server.toml'
            config.write_text(f'bind="127.0.0.1:{port}"\nallowed_origins=[{json.dumps(url.rstrip("/"))}]\n'
                              f'[[recordings]]\nid="switch"\ndisplay_name="Source switch fixture"\npath={json.dumps(str(args.data.resolve()))}\n')
            with (args.output / 'server.log').open('w') as log:
                server = subprocess.Popen([str(args.server.resolve()), '--config', str(config.resolve())], stdout=log, stderr=log)
                try:
                    def server_ready():
                        try:
                            with urllib.request.urlopen(f'http://127.0.0.1:{port}/v1/recordings', timeout=1) as response:
                                return response.status == 200
                        except OSError:
                            return False
                    wait(server_ready, 'Recording Server unavailable')
                    with browser(free_port(), extra_flags=['--disable-accelerated-2d-canvas']) as (_, page, environment):
                        result['browser_version'] = environment['version']['Browser']
                        page.call('Page.addScriptToEvaluateOnNewDocument', {'source': "window.smokeErrors=[];addEventListener('error',e=>smokeErrors.push(e.message));addEventListener('unhandledrejection',e=>smokeErrors.push(String(e.reason)))"})
                        page.call('Page.navigate', {'url': url})
                        wait(lambda: page.evaluate('!!window.wasmBindings'), 'WASM application unavailable')
                        page.call('DOM.enable')

                        def local():
                            page.evaluate('document.querySelector("#mcap-file").value=""')
                            root = page.call('DOM.getDocument')['root']['nodeId']
                            node = page.call('DOM.querySelector', {'nodeId': root, 'selector': '#mcap-file'})['nodeId']
                            page.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(args.data.resolve())]})

                        def fresh(label):
                            wait(lambda: page.evaluate('document.querySelector("#status").textContent.startsWith("Ready") && document.querySelector("#mcap-file").disabled===false'), label + ': not ready')
                            controls = page.evaluate('({play:document.querySelector("#play").textContent,speed:document.querySelector("#speed").value,cursor:Number(document.querySelector("#timeline").value),cameras:document.querySelectorAll(".camera-card canvas").length})')
                            assert controls['play'] == 'Play', controls
                            assert controls['speed'] == '1', controls
                            assert controls['cursor'] == 0, controls
                            assert controls['cameras'] > 0, controls
                            result['checks'].append({'source': label, 'controls': controls})

                        def dirty():
                            page.evaluate('(()=>{const e=document.querySelector("#speed");e.value="2";e.dispatchEvent(new Event("change"));document.querySelector("#play").click()})()')
                            wait(lambda: page.evaluate('document.querySelector("#play").textContent==="Pause" && Number(document.querySelector("#timeline").value)>0'), 'old source not playing')

                        def seek():
                            page.evaluate('(()=>{const e=document.querySelector("#timeline");e.value="0.5";e.dispatchEvent(new Event("input"))})()')
                            # Let the application publish pending/committed state. The slider's
                            # input value alone would otherwise pass before any restore runs.
                            page.call('Runtime.evaluate', {
                                'expression': 'new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(true))))',
                                'awaitPromise': True,
                            })
                            wait(lambda: page.evaluate('document.querySelector("#status").textContent.startsWith("Ready") && Math.abs(Number(document.querySelector("#timeline").value)-.5)<.005'), 'seek after replacement failed')

                        local()
                        fresh('Local initial')
                        dirty()
                        local()
                        fresh('Local replacement while playing at 2x')
                        seek()
                        dirty()
                        page.evaluate(f'document.querySelector("#remote-server-url").value="http://127.0.0.1:{port}";document.querySelector("#remote-connect").click()')
                        wait(lambda: page.evaluate('document.querySelector("#remote-recording").options.length>0'), 'remote list unavailable')
                        page.evaluate('document.querySelector("#remote-load-catalog").click()')
                        wait(lambda: page.evaluate('document.querySelector("#remote-output").textContent.includes("recording: switch")'), 'remote catalog unavailable')
                        page.evaluate('document.querySelector("#remote-open-playback").click()')
                        fresh('Local to Remote')
                        seek()
                        dirty()
                        local()
                        fresh('Remote to Local')
                        seek()
                        result['errors'] = page.evaluate('window.smokeErrors')
                        assert not result['errors'], result['errors']
                        result['status'] = 'ok'
                finally:
                    server.terminate()
                    server.wait(timeout=10)
    except Exception as error:
        result.update(status='failed', error=repr(error))
        raise
    finally:
        (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
