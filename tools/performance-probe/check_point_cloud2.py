#!/usr/bin/env python3
"""Bounded PointCloud2 pixel smoke: actual NVIDIA Native and WebGPU Web."""
import argparse
import base64
from functools import partial
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request

from PIL import Image
from browser_cdp import CDP
from check_native_gpu import X11
from compare_native_desktop import capture, wait_for, window_for

ROOT = Path(__file__).resolve().parents[2]


def histogram(image):
    colors = {'red': 0, 'green': 0, 'blue': 0, 'cyan': 0}
    for r, g, b in image.convert('RGB').getdata():
        colors['red'] += r > 180 and g < 130 and b < 130
        colors['green'] += g > 180 and r < 130 and b < 160
        colors['blue'] += b > 180 and r < 130 and g < 150
        colors['cyan'] += g > 180 and b > 180 and r < 160
    return colors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('new output directory required')
    args.output.mkdir(parents=True)
    env = os.environ | {'DISPLAY': ':1', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority',
                        'VK_DRIVER_FILES': '/usr/share/vulkan/icd.d/nvidia_icd.json',
                        'RUST_LOG': 'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info'}
    result = {'status': 'running', 'data': str(args.data), 'data_sha256': hashlib.sha256(args.data.read_bytes()).hexdigest(),
              'native_gpu': 'NVIDIA Vulkan required', 'web_gpu': 'headed Chrome; actual WebGPU adapter recorded below; functional smoke only'}
    processes = []
    server = browser = cdp = x11 = None
    with tempfile.TemporaryDirectory(prefix='mcap-point-cloud2-chrome-') as profile:
        try:
            for port in [9360, 8198]:
                with socket.socket() as probe:
                    probe.bind(('127.0.0.1', port))
            subprocess.run(['xset', 'dpms', 'force', 'on'], env=env, check=True, timeout=5)
            binary = ROOT/'target/gpu-native/viewer-native-point-cloud2'
            result['native_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
            with (args.output/'native.log').open('w') as log:
                command = [str(binary), '--mcap', str(args.data.resolve()), '--layout', 'scene',
                           '--point-cloud-topic', '/points', '--fixed-frame', 'base_scan', '--no-platform-geometry']
                native = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=log, start_new_session=True)
                processes.append(native)
                window = wait_for(lambda: window_for(native.pid, env), 15, 'Native owned window')
                x11 = X11(':1')
                x11.x.XMoveResizeWindow(x11.display, window, 40, 60, 1920, 1080)
                x11.x.XFlush(x11.display)
                time.sleep(3)
                text = (args.output/'native.log').read_text()
                if 'NVIDIA' not in text or 'DiscreteGpu' not in text or 'Vulkan' not in text or 'ERROR' in text:
                    raise RuntimeError('Native GPU selection or shader failed')
                raw, stride = capture(x11, window, [0, 0, 1920, 1080])
                image = Image.frombytes('RGB', (1920, 1080), raw, 'raw', 'BGRX', stride, 1)
                image.save(args.output/'native-rgb.png')
                result['native_colors'] = histogram(image.crop((320, 150, 1920, 1000)))
                palette = [(255,48,32),(32,255,64),(48,80,255)]
                scene_pixels = list(image.crop((320,150,1920,1000)).getdata())
                result['native_source_rgb_hits'] = [sum(max(abs(a-b) for a,b in zip(pixel,expected)) <= 3 for pixel in scene_pixels) for expected in palette]
                if min(result['native_source_rgb_hits']) < 20:
                    raise RuntimeError('Native source RGB values changed in presentation')
                if min(result['native_colors'][k] for k in ['red', 'green', 'blue']) < 20:
                    raise RuntimeError('Native colored cloud pixels absent')
                native.terminate()
                native.wait(timeout=5)
            server = ThreadingHTTPServer(('127.0.0.1', 8198), partial(SimpleHTTPRequestHandler, directory=str(ROOT/'apps/viewer-web/dist')))
            threading.Thread(target=server.serve_forever, daemon=True).start()
            with (args.output/'chrome.log').open('w') as log:
                chrome = subprocess.Popen(['google-chrome', '--no-sandbox', '--enable-unsafe-webgpu', '--enable-gpu',
                                           '--ozone-platform=x11', '--use-gl=angle', '--use-angle=gl', '--no-first-run', '--no-default-browser-check', '--force-color-profile=srgb',
                                           '--remote-debugging-port=9360', '--user-data-dir='+profile, 'about:blank'],
                                          env=env, stdout=log, stderr=log, start_new_session=True)
                processes.append(chrome)
                def page():
                    try:
                        return next((p for p in json.load(urllib.request.urlopen('http://127.0.0.1:9360/json')) if p['type']=='page'), None)
                    except OSError:
                        return None
                cdp = CDP(wait_for(page, 15, 'Chrome page')['webSocketDebuggerUrl'])
                version = json.load(urllib.request.urlopen('http://127.0.0.1:9360/json/version'))
                browser = CDP(version['webSocketDebuggerUrl'])
                result['web_gpu_info'] = browser.call('SystemInfo.getInfo')['gpu']
                cdp.call('Emulation.setDeviceMetricsOverride', {'width': 1920, 'height': 1080, 'deviceScaleFactor': 1, 'mobile': False})
                cdp.call('Page.enable')
                cdp.call('Page.addScriptToEvaluateOnNewDocument', {'source': """
window.pointCloudGpuErrors=[];window.pointCloudAdapter=null;
window.addEventListener('error',e=>pointCloudGpuErrors.push(e.message));
const original=navigator.gpu.requestAdapter.bind(navigator.gpu);
navigator.gpu.requestAdapter=async(...args)=>{const adapter=await original(...args);if(adapter){
window.pointCloudAdapter={vendor:adapter.info.vendor,device:adapter.info.device,description:adapter.info.description,architecture:adapter.info.architecture};
const request=adapter.requestDevice.bind(adapter);adapter.requestDevice=async(...args)=>{const device=await request(...args);device.addEventListener('uncapturederror',e=>pointCloudGpuErrors.push(e.error.message));return device;};
}return adapter;};
"""})
                cdp.call('Page.navigate', {'url': 'http://127.0.0.1:8198/'})
                wait_for(lambda: cdp.evaluate("document.querySelector('#bev-status')?.innerText.includes('ready')"), 20, 'WebGPU ready')
                cdp.evaluate("document.querySelector('#point-cloud-topic').value='/points';document.querySelector('#fixed-frame').value='base_scan';document.querySelector('#fixed-frame').dispatchEvent(new Event('change'));document.querySelector('#platform-enabled').click()")
                document = cdp.call('DOM.getDocument')['root']['nodeId']
                node = cdp.call('DOM.querySelector', {'nodeId': document, 'selector': '#mcap-file'})['nodeId']
                cdp.call('DOM.setFileInputFiles', {'nodeId': node, 'files': [str(args.data.resolve())]})
                wait_for(lambda: cdp.evaluate("document.querySelector('#scene-info').innerText.includes('1000 visible points')"), 20, 'Web point cloud materialization')
                info = cdp.call('Runtime.evaluate', {'expression': "(async()=>{const a=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});return {vendor:a.info.vendor,device:a.info.device,description:a.info.description,architecture:a.info.architecture}})()", 'awaitPromise':True,'returnByValue':True})
                result['web_adapter'] = info['result']['value']
                result['web_gpu'] = 'WebGPU '+str(result['web_adapter'])+'; ANGLE compositor GPU is separate; functional smoke only'
                result['web_modes'] = {}
                for mode in ['rgb', 'intensity', 'solid']:
                    cdp.evaluate(f"document.querySelector('#point-cloud-color').value='{mode}'")
                    time.sleep(.5)
                    screenshot = cdp.call('Page.captureScreenshot', {'format': 'png'})['data']
                    path = args.output/f'web-{mode}.png'
                    path.write_bytes(base64.b64decode(screenshot))
                    result['web_errors'] = cdp.evaluate('window.pointCloudGpuErrors')
                    if result['web_errors'] is None or result['web_errors']: raise RuntimeError(str(result['web_errors']))
                    rect = cdp.evaluate("document.querySelector('#bev').getBoundingClientRect().toJSON()")
                    image = Image.open(path).convert('RGB').crop((rect['x'], rect['y'], rect['right'], rect['bottom']))
                    colors = histogram(image)
                    if mode=='rgb':
                        pixels = list(image.getdata())
                        hits = [sum(max(abs(a-b) for a,b in zip(pixel,expected)) <= 3 for pixel in pixels) for expected in palette]
                        result['web_source_rgb_hits'] = hits
                        if min(hits)<20: raise RuntimeError('Web source RGB values changed in presentation: '+str(hits))
                    result['web_modes'][mode] = {'colors': colors, 'scene': cdp.evaluate("document.querySelector('#scene-info').innerText")}
                    required = ['cyan'] if mode=='solid' else ['red', 'green', 'blue']
                    if min(colors[k] for k in required) < 20:
                        raise RuntimeError(f'Web {mode} pixels absent: {colors}')
                result['status'] = 'ok'
        except Exception as error:
            result.update(status='failed', error=repr(error))
        finally:
            if browser:
                try:
                    browser.sock.settimeout(2)
                    browser.call('Browser.close')
                except (OSError, RuntimeError):
                    pass
            for connection in [browser, cdp]:
                if connection:
                    connection.close()
            for process in processes:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)
            if x11:
                x11.x.XCloseDisplay(x11.display)
            if server:
                server.shutdown()
                server.server_close()
            result['owned_pids'] = [p.pid for p in processes]
            result['exit_codes'] = [p.poll() for p in processes]
            (args.output/'result.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps({'status': result['status'], 'error': result.get('error')}))
    if result['status'] != 'ok':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
