#!/usr/bin/env python3
"""Exercise actual 4K/8-camera Web paints and seek recovery using fixture image identities."""
import argparse
import base64
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request

from browser_cdp import CDP
from measure_viewer_pilot import process_sample, ROOT


@contextmanager
def browser(port, *, extra_flags=()):
    with socket.socket() as check:
        check.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        check.bind(('127.0.0.1', port))
    with tempfile.TemporaryDirectory(prefix='mcap-camera-stress-') as profile, tempfile.TemporaryFile() as log:
        command=['google-chrome',*extra_flags,'--no-sandbox','--enable-unsafe-webgpu',
                 '--ozone-platform=x11','--no-first-run','--no-default-browser-check','--password-store=basic',
                 f'--remote-debugging-port={port}',f'--user-data-dir={profile}','about:blank']
        process=subprocess.Popen(command,stdout=log,stderr=log,env=__import__('os').environ|{'DISPLAY':':1','XAUTHORITY':'/run/user/1000/gdm/Xauthority','VK_DRIVER_FILES':'/usr/share/vulkan/icd.d/nvidia_icd.json'})
        host=page=None
        try:
            for _ in range(200):
                try:
                    version=json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/version',timeout=1))
                    pages=json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list',timeout=1))
                    if not any(p['type']=='page'for p in pages):time.sleep(.05);continue
                    break
                except OSError:
                    if process.poll() is not None: raise RuntimeError('Chrome exited')
                    time.sleep(.05)
            else: raise RuntimeError('Chrome startup timeout')
            host=CDP(version['webSocketDebuggerUrl'])
            page=CDP(next(p['webSocketDebuggerUrl'] for p in pages if p['type']=='page'))
            page.call('Page.enable')
            page.call('Emulation.setDeviceMetricsOverride',{'width':1920,'height':1080,'deviceScaleFactor':1,'mobile':False})
            yield process,page,{'version':version,'gpu':host.call('SystemInfo.getInfo'),'command':command}
        finally:
            log.seek(0);Path('/tmp/recorded-hardware-chrome.log').write_bytes(log.read())
            if page: page.close()
            if host:
                try: host.call('Browser.close')
                except (RuntimeError,OSError): pass
                host.close()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate();process.wait(timeout=10)
