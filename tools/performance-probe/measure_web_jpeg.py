#!/usr/bin/env python3
"""Measure a served jpeg-benchmark Web build through an existing Chrome CDP port.
Run from the repo root after extracting 32 JPEGs into the dist/samples directory.
Usage: python3 tools/performance-probe/measure_web_jpeg.py PORT OUTPUT_JSON [--camera-check | --queue-check]
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import sys
import time
import urllib.request
from urllib.parse import urlparse

port = sys.argv[1] if len(sys.argv) > 1 else '9235'
page = next(page for page in json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list')) if page['type'] == 'page')
url = urlparse(page['webSocketDebuggerUrl'])
sock = socket.create_connection((url.hostname, url.port), timeout=30)
key = base64.b64encode(os.urandom(16)).decode()
sock.sendall(f'GET {url.path} HTTP/1.1\r\nHost: {url.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode())
header = bytearray()
while not header.endswith(b'\r\n\r\n'):
    header += sock.recv(1)
assert header.startswith(b'HTTP/1.1 101 '), header
assert base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()) in header

def recv_exact(length):
    data = bytearray()
    while len(data) < length:
        part = sock.recv(length-len(data))
        if not part: raise RuntimeError('WebSocket closed')
        data += part
    return bytes(data)

def send_frame(opcode, payload):
    mask = os.urandom(4)
    length = len(payload)
    head = bytes([0x80 | opcode, 0x80 | length]) if length < 126 else bytes([0x80 | opcode, 0xfe]) + struct.pack('!H', length) if length <= 65535 else bytes([0x80 | opcode, 0xff]) + struct.pack('!Q', length)
    sock.sendall(head + mask + bytes(value ^ mask[i % 4] for i, value in enumerate(payload)))

request_id = 0
def call(method, params=None):
    global request_id
    request_id += 1
    send_frame(1, json.dumps({'id': request_id, 'method': method, 'params': params or {}}).encode())
    fragments = bytearray()
    while True:
        header = recv_exact(2)
        length = header[1] & 127
        if length == 126: length = struct.unpack('!H', recv_exact(2))[0]
        if length == 127: length = struct.unpack('!Q', recv_exact(8))[0]
        if header[1] & 128: raise RuntimeError('Unexpected masked server frame')
        payload = recv_exact(length)
        opcode = header[0] & 15
        if opcode == 9: send_frame(10, payload); continue
        if opcode == 8: raise RuntimeError('WebSocket closed')
        fragments += payload
        if not header[0] & 128: continue
        message = json.loads(fragments)
        fragments.clear()
        if message.get('id') == request_id:
            if 'error' in message: raise RuntimeError(message['error'])
            return message.get('result', {})


camera_check='--camera-check' in sys.argv
queue_check='--queue-check' in sys.argv
if camera_check and queue_check: raise ValueError('choose one check')
call('Page.enable')
call('Emulation.setDeviceMetricsOverride', {'width':1920, 'height':1080, 'deviceScaleFactor':1, 'mobile':False})
call('Page.navigate', {'url':'http://127.0.0.1:8094/index.html'})
for _ in range(300):
    state=call('Runtime.evaluate', {'expression':'Boolean(window.wasmBindings)' if camera_check else 'Boolean(window.wasmBindings?.benchmark_jpeg_canvas)', 'returnByValue':True})
    if state.get('result',{}).get('value'):break
    time.sleep(.1)
else:raise RuntimeError('benchmark build did not load')
sock.settimeout(300)
expression=Path('tools/performance-probe/check_web_camera.js' if camera_check else 'tools/performance-probe/web_jpeg_benchmark.js').read_text()
if queue_check:
    expression = '''(async () => {
        const bytes = new Uint8Array(await (await fetch('/samples/00.jpg')).arrayBuffer());
        await window.wasmBindings.check_camera_bitmap_queue(bytes);
        return {queueCheck: 'passed', saturatedEncodedBytes: 64 * 1024 * 1024,
                checks: ['coalescing', 'seek generation', 'two job limit',
                         'metadata identity', 'budget release', 'post-budget admission']};
    })()'''
result=call('Runtime.evaluate', {'expression':expression,'awaitPromise':True,'returnByValue':True,'timeout':240000})
if 'exceptionDetails' in result:raise RuntimeError(result['exceptionDetails'])
data=result['result']['value']
Path(sys.argv[2]).write_text(json.dumps(data,indent=2)+'\n')
print(json.dumps(data if camera_check or queue_check else data['summary'],indent=2))
if camera_check:
    screenshot=call('Page.captureScreenshot',{'format':'png'})
    Path(sys.argv[2]).with_suffix('.png').write_bytes(base64.b64decode(screenshot['data']))
