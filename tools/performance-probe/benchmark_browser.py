"""Dedicated Chrome/CDP lifetime and local HTTP server for repeatable probes."""
from contextlib import contextmanager
import functools
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request

from browser_cdp import CDP


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


@contextmanager
def serve(directory):
    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass
    handler = functools.partial(Handler, directory=str(directory))
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/'
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@contextmanager
def browser(mode, log_path, *, wake_display=False, vulkan_icd=None, extra_flags=()):
    """Never attach to another Chrome. Hardware mode refuses software fallback."""
    env = os.environ.copy()
    if vulkan_icd:
        env['VK_DRIVER_FILES'] = str(Path(vulkan_icd).resolve(strict=True))
    env.setdefault('DISPLAY', ':1')
    env.setdefault('XAUTHORITY', '/run/user/1000/gdm/Xauthority')
    initial_display = None
    if mode == 'hardware':
        initial_display = subprocess.check_output(['xset', 'q'], env=env, text=True, timeout=5)
        if wake_display:
            subprocess.run(['xset', 'dpms', 'force', 'on'], env=env, check=True, timeout=5)
            subprocess.run(['xset', 's', 'reset'], env=env, check=True, timeout=5)
        if 'Monitor is On' not in subprocess.check_output(['xset', 'q'], env=env, text=True, timeout=5):
            raise RuntimeError('Display is asleep; explicitly use --wake-display')
    port = free_port()
    with tempfile.TemporaryDirectory(prefix='mcap-regression-chrome-') as profile, Path(log_path).open('w') as log:
        flags = (['--ozone-platform=x11', '--enable-features=Vulkan', '--use-vulkan=native'] if mode == 'hardware' else
                 ['--headless', '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader',
                  '--disable-accelerated-2d-canvas'])
        command = ['google-chrome', '--no-sandbox', '--enable-unsafe-webgpu', '--no-first-run',
                   '--no-default-browser-check', '--password-store=basic',
                   '--disable-background-timer-throttling', '--disable-renderer-backgrounding',
                   '--disable-backgrounding-occluded-windows', *flags, *extra_flags,
                   f'--remote-debugging-port={port}', f'--user-data-dir={profile}', 'about:blank']
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=True)
        host = page = None
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                try:
                    version = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/version', timeout=1))
                    pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list', timeout=1))
                    target = next(p for p in pages if p['type'] == 'page')
                    break
                except (OSError, StopIteration):
                    if process.poll() is not None:
                        raise RuntimeError('Chrome exited before CDP startup')
                    time.sleep(.05)
            else:
                raise RuntimeError('Chrome CDP startup timeout')
            host = CDP(version['webSocketDebuggerUrl'])
            page = CDP(target['webSocketDebuggerUrl'])
            page.sock.settimeout(45)
            page.call('Page.enable')
            page.call('Page.bringToFront')
            page.call('Emulation.setDeviceMetricsOverride', {
                'width': 1920, 'height': 1080, 'deviceScaleFactor': 1, 'mobile': False})
            info = host.call('SystemInfo.getInfo')
            if mode == 'hardware':
                renderer = json.dumps(info['gpu']).lower()
                if any(word in renderer for word in ['swiftshader', 'llvmpipe']):
                    raise RuntimeError('Hardware benchmark fell back to software GPU')
            yield process, page, {
                'version': version['Browser'], 'gpu': info['gpu'], 'command': command,
                'mode': mode, 'viewport': [1920, 1080], 'dpr': 1,
                'display_initial': initial_display,
            }
            if mode == 'hardware' and 'Monitor is On' not in subprocess.check_output(
                    ['xset', 'q'], env=env, text=True, timeout=5):
                raise RuntimeError('Display slept during benchmark; invalidate this run')
        finally:
            if page:
                page.close()
            if host:
                try:
                    host.call('Browser.close')
                except (RuntimeError, OSError):
                    pass
                host.close()
            if process.poll() is None:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, 15)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, 9)
                        process.wait(timeout=5)
            if wake_display and initial_display and 'Monitor is Off' in initial_display:
                subprocess.run(['xset', 'dpms', 'force', 'off'], env=env, check=True, timeout=5)
