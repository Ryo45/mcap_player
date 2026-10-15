"""Small synchronous Chrome DevTools connection for local performance probes (stdlib only)."""
import base64
import hashlib
import json
import os
import socket
import struct
from urllib.parse import urlparse


class CDP:
    def __init__(self, websocket_url):
        url = urlparse(websocket_url)
        self.sock = socket.create_connection((url.hostname, url.port), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f'GET {url.path} HTTP/1.1\r\nHost: {url.netloc}\r\nUpgrade: websocket\r\n'
                           f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
        header = bytearray()
        while not header.endswith(b'\r\n\r\n'):
            header += self._read(1)
        expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        if not header.startswith(b'HTTP/1.1 101 ') or expected not in header:
            raise RuntimeError(f'CDP handshake failed: {header!r}')
        self.request_id = 0

    def close(self):
        self.sock.close()

    def _read(self, length):
        data = bytearray()
        while len(data) < length:
            part = self.sock.recv(length - len(data))
            if not part:
                raise RuntimeError('CDP socket closed')
            data += part
        return bytes(data)

    def _send(self, opcode, payload):
        mask = os.urandom(4)
        length = len(payload)
        if length < 126:
            head = bytes([0x80 | opcode, 0x80 | length])
        elif length <= 65535:
            head = bytes([0x80 | opcode, 0xfe]) + struct.pack('!H', length)
        else:
            head = bytes([0x80 | opcode, 0xff]) + struct.pack('!Q', length)
        self.sock.sendall(head + mask + bytes(v ^ mask[i % 4] for i, v in enumerate(payload)))

    def call(self, method, params=None):
        self.request_id += 1
        self._send(1, json.dumps({'id': self.request_id, 'method': method, 'params': params or {}}).encode())
        fragments = bytearray()
        while True:
            header = self._read(2)
            length = header[1] & 127
            if length == 126:
                length = struct.unpack('!H', self._read(2))[0]
            elif length == 127:
                length = struct.unpack('!Q', self._read(8))[0]
            if header[1] & 128:
                raise RuntimeError('Unexpected masked server frame')
            payload = self._read(length)
            opcode = header[0] & 15
            if opcode == 9:
                self._send(10, payload)
                continue
            if opcode == 8:
                raise RuntimeError('CDP socket closed')
            fragments += payload
            if not header[0] & 128:
                continue
            message = json.loads(fragments)
            fragments.clear()
            if message.get('id') == self.request_id:
                if 'error' in message:
                    raise RuntimeError(message['error'])
                return message.get('result', {})

    def evaluate(self, expression):
        result = self.call('Runtime.evaluate', {'expression': expression, 'returnByValue': True})
        if 'exceptionDetails' in result:
            raise RuntimeError(result['exceptionDetails'])
        return result.get('result', {}).get('value')
