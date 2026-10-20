#!/usr/bin/env python3
"""Bounded X11 Native smoke check, scale=1, small 5-second README fixture.

Uses existing display authorization. Captures only the spawned process's window.
Checks adapter/PID automatically; pause/resume images require human inspection.
Does not claim automated Seek verification.
Images require human inspection; successful input delivery is not a UI assertion.
"""
import argparse
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import time
import zlib

ROOT = Path(__file__).resolve().parents[2]


class XImage(c.Structure):
    _fields_ = [('width', c.c_int), ('height', c.c_int), ('xoffset', c.c_int),
                ('format', c.c_int), ('data', c.c_void_p), ('byte_order', c.c_int),
                ('bitmap_unit', c.c_int), ('bitmap_bit_order', c.c_int),
                ('bitmap_pad', c.c_int), ('depth', c.c_int), ('bytes_per_line', c.c_int),
                ('bits_per_pixel', c.c_int), ('red_mask', c.c_ulong),
                ('green_mask', c.c_ulong), ('blue_mask', c.c_ulong)]


class XKeyEvent(c.Structure):
    _fields_=[('type',c.c_int),('serial',c.c_ulong),('send_event',c.c_int),
              ('display',c.c_void_p),('window',c.c_ulong),('root',c.c_ulong),
              ('subwindow',c.c_ulong),('time',c.c_ulong),('x',c.c_int),('y',c.c_int),
              ('x_root',c.c_int),('y_root',c.c_int),('state',c.c_uint),
              ('keycode',c.c_uint),('same_screen',c.c_int)]


class XEvent(c.Union):
    _fields_=[('key',XKeyEvent),('padding',c.c_long*24)]


class X11:
    def __init__(self, display):
        self.x = c.CDLL('libX11.so.6')
        bindings = [
            (self.x, 'XOpenDisplay', [c.c_char_p], c.c_void_p),
            (self.x, 'XSendEvent', [c.c_void_p,c.c_ulong,c.c_int,c.c_long,c.POINTER(XEvent)], c.c_int),
            (self.x, 'XKeysymToKeycode', [c.c_void_p,c.c_ulong], c.c_ubyte),
            (self.x, 'XDefaultRootWindow', [c.c_void_p], c.c_ulong),
            (self.x, 'XMoveResizeWindow', [c.c_void_p,c.c_ulong,c.c_int,c.c_int,c.c_uint,c.c_uint], c.c_int),
            (self.x, 'XRaiseWindow', [c.c_void_p,c.c_ulong], c.c_int),
            (self.x, 'XSetInputFocus', [c.c_void_p,c.c_ulong,c.c_int,c.c_ulong], c.c_int),
            (self.x, 'XGetImage', [c.c_void_p,c.c_ulong,c.c_int,c.c_int,c.c_uint,c.c_uint,
                                 c.c_ulong,c.c_int], c.POINTER(XImage)),
            (self.x, 'XDestroyImage', [c.POINTER(XImage)], c.c_int),
            (self.x, 'XFlush', [c.c_void_p], c.c_int),
            (self.x, 'XCloseDisplay', [c.c_void_p], c.c_int),
        ]
        for lib, name, args, result in bindings:
            function = getattr(lib,name); function.argtypes=args; function.restype=result
        self.display = self.x.XOpenDisplay(display.encode())
        if not self.display: raise RuntimeError('XOpenDisplay failed')

    def resize(self, window, width, height):
        self.window=window
        self.x.XMoveResizeWindow(self.display,window,40,60,width,height)
        self.x.XRaiseWindow(self.display,window)
        self.x.XSetInputFocus(self.display,window,2,0)
        self.x.XFlush(self.display)

    def key(self, keysym, pressed=None):
        code=self.x.XKeysymToKeycode(self.display,keysym)
        if not code:raise RuntimeError('keysym unavailable')
        for down in ([True,False] if pressed is None else [pressed]):
            event=XEvent()
            event.key=XKeyEvent(type=2 if down else 3,send_event=1,display=self.display,
                window=self.window,root=self.x.XDefaultRootWindow(self.display),
                time=int(time.monotonic()*1000)&0xffffffff,state=0,keycode=code,same_screen=1)
            assert self.x.XSendEvent(self.display,self.window,False,1 if down else 2,c.byref(event))
            self.x.XFlush(self.display);time.sleep(.04)
        if keysym in (0xff09,0x20):time.sleep(.4)

    def capture(self, window, width, height, path):
        image=self.x.XGetImage(self.display,window,0,0,width,height,c.c_ulong(-1).value,2)
        if not image: raise RuntimeError('XGetImage failed')
        try:
            im=image.contents
            if im.bits_per_pixel != 32: raise RuntimeError('requires 32-bit XImage')
            raw=c.string_at(im.data,im.bytes_per_line*height)
            shifts=[(mask & -mask).bit_length()-1 for mask in (im.red_mask,im.green_mask,im.blue_mask)]
            rows=bytearray()
            for y in range(height):
                rows.append(0)
                row=raw[y*im.bytes_per_line:y*im.bytes_per_line+width*4]
                for pixel, in struct.iter_unpack('<I' if im.byte_order==0 else '>I',row):
                    rows.extend((pixel>>shift)&255 for shift in shifts)
            def chunk(name,data):
                return struct.pack('!I',len(data))+name+data+struct.pack('!I',zlib.crc32(name+data)&0xffffffff)
            path.write_bytes(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('!IIBBBBB',width,height,8,2,0,0,0))+
                             chunk(b'IDAT',zlib.compress(rows,6))+chunk(b'IEND',b''))
        finally:
            self.x.XDestroyImage(image)


def sha(path):
    with path.open('rb') as source: return hashlib.file_digest(source,'sha256').hexdigest()


def run(args):
    args.output.mkdir(parents=True,exist_ok=False)
    env=os.environ|{'DISPLAY':args.display, 'XAUTHORITY':str(args.xauthority),
        'VK_DRIVER_FILES':str(args.icd),
        'RUST_LOG':'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info'}
    result={'status':'running','layout':args.layout,'binary_sha256':sha(args.binary),
            'fixture_sha256':sha(args.data),'fixture':str(args.data.resolve()),
            'git_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            'probe_sha256':sha(Path(__file__)),
            'environment':{k:env[k] for k in ('DISPLAY','XAUTHORITY','VK_DRIVER_FILES','RUST_LOG')},
            'frames':[], 'input_method':'targeted XSendEvent Tab/Space for pause/resume; no automated Seek',
            'visual_validation':'not automated: inspect captured UI cursor, camera and scene',
            'seek_action_probe':args.probe_seek,'normal_ui_seek_tested':False,
            'wall_deadline_seconds':45}
    if args.probe_seek:
        env['MCAP_GPU_SEEK_PROBE_CONTROL']=str((args.output/'seek-control.txt').resolve())
    result['display_before']={
        'xset':subprocess.check_output(['xset','q'],env=env,text=True,timeout=5),
        'xrandr':subprocess.check_output(['xrandr','--current'],env=env,text=True,timeout=5),
        'nvidia':subprocess.check_output(['nvidia-smi','--query-gpu=name,driver_version,display_active,memory.used,memory.total','--format=csv'],text=True,timeout=5)}
    started=time.monotonic()
    with (args.output/'native.log').open('w') as log:
        command=[str(args.binary.resolve()),'--mcap',str(args.data.resolve()),'--layout',args.layout]
        if args.layout=='monitor':command+=['--inspect-topic','/scan']
        process=subprocess.Popen(command,cwd=ROOT,env=env,stdout=log,stderr=log)
        result.update(command=command,pid=process.pid)
        x11=None
        try:
            def wait(seconds):
                if time.monotonic()-started+seconds>45:raise RuntimeError('wall deadline exceeded')
                time.sleep(seconds)
                if process.poll() is not None:raise RuntimeError(f'Native exited {process.returncode}')
            window=None
            while time.monotonic()-started<12:
                tree=subprocess.check_output(['xwininfo','-root','-tree'],env=env,text=True,timeout=3)
                for value in re.findall(r'(0x[0-9a-f]+) "MCAP Camera',tree):
                    owner=subprocess.check_output(['xprop','-id',value,'_NET_WM_PID'],env=env,text=True,timeout=3)
                    if re.search(r'= '+str(process.pid)+r'\b',owner):window=int(value,16);break
                if window:break
                wait(.1)
            if not window:raise RuntimeError('spawned Native window not found')
            result['window']=hex(window)
            x11=X11(args.display);x11.resize(window,1920,1080);wait(1)
            def capture(phase,width=1920,height=1080):
                info=subprocess.check_output(['xwininfo','-id',hex(window)],env=env,text=True,timeout=3)
                actual=[int(re.search(key+r': (\d+)',info)[1]) for key in ('Width','Height')]
                if actual!=[width,height]:raise RuntimeError(f'unexpected client size {actual}')
                path=args.output/(phase+'.png');x11.capture(window,width,height,path)
                gpu=subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
                result['frames'].append({'phase':phase,'wall_seconds':time.monotonic()-started,
                    'image':path.name,'sha256':sha(path),'physical_size':actual,'window_info':info,
                    'nvidia_process_lines':[line for line in gpu.splitlines() if re.search(r'\b'+str(process.pid)+r'\b',line)],
                    'status_memory':Path(f'/proc/{process.pid}/status').read_text().split('VmPeak:')[1].split('SigQ:')[0]})
            capture('playing')
            # Keyboard focus changes must land in separate rendered frames.
            x11.key(0xff09);x11.key(0x20);wait(1.2);capture('paused')
            wait(1);capture('paused-stable')
            if args.probe_seek:
                def seek(request,offset,phase):
                    (args.output/'seek-control.txt').write_text(f'{request} {offset}\n')
                    wait(1);capture(phase)
                    wait(.5);capture(phase+'-stable')
                seek(1,1_000_000_000,'seek-back')
                seek(2,4_000_000_000,'seek-forward')
                (args.output/'seek-control.txt').write_text('3 1000000000\n')
                wait(.5)
            x11.key(0x20);wait(1);capture('resume')
            x11.resize(window,1600,1000);wait(.5);capture('resized',1600,1000)
            wait(3);capture('end',1600,1000)
            text=(args.output/'native.log').read_text()
            if not re.search(r'Request adapter result AdapterInfo.*NVIDIA.*device_type: DiscreteGpu.*backend: Vulkan',text):
                raise RuntimeError('NVIDIA hardware Vulkan adapter not confirmed')
            if not all(f['nvidia_process_lines'] for f in result['frames']):
                raise RuntimeError('Native PID absent from NVIDIA graphics process table')
            if args.probe_seek and not all(f'GPU_PROBE_SEEK id={i} ' in text for i in (1,2,3)):
                raise RuntimeError('seek action requests not applied')
            if re.search(r'panicked at|ERROR|DeviceLost|SurfaceError',text):raise RuntimeError('Native error in log')
            result['status']='captured-hardware-gpu'
        except Exception as error:
            result.update(status='error',error=str(error))
        finally:
            if x11:x11.x.XCloseDisplay(x11.display)
            process.terminate()
            try:process.wait(timeout=3)
            except subprocess.TimeoutExpired:process.kill();process.wait()
            result['cleanup_returncode']=process.returncode
            result['wall_seconds']=time.monotonic()-started
            (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'status':result['status'],'output':str(args.output),'error':result.get('error')}))
    if result['status']=='error':raise SystemExit(1)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=ROOT/'target/gpu-native/viewer-native')
    parser.add_argument('--data',type=Path,required=True,help='5.018 s README fixture')
    parser.add_argument('--output',type=Path,required=True,help='new artifact directory')
    parser.add_argument('--probe-seek',action='store_true',help='requires test-only seek-probe.patch binary; not a normal UI Seek test')
    parser.add_argument('--layout',choices=['monitor','standard'],default='monitor')
    parser.add_argument('--display',default=os.environ.get('DISPLAY',':1'))
    parser.add_argument('--xauthority',type=Path,default=Path('/run/user/1000/gdm/Xauthority'))
    parser.add_argument('--icd',type=Path,default=Path('/usr/share/vulkan/icd.d/nvidia_icd.json'))
    args=parser.parse_args()
    run(args)
