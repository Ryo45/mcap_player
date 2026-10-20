#!/usr/bin/env python3
"""Hardware-GPU single-camera Desktop comparison; measurement Native patch is required.

X11 samples actual client-window pixels at 20 Hz. JPEG identity is matched against
32 fixture variants. Seek invokes existing UI action handlers, bypassing OS input.
No screen unlock, power-policy change, or other application's process termination.
"""
import argparse
import base64
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import shutil
import socket
import statistics
import subprocess
import tempfile
import time
import urllib.request

from PIL import Image, ImageFilter
from browser_cdp import CDP
from check_native_gpu import X11
from measure_viewer_pilot import process_sample

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / 'docs/performance/2026-10-20-native-lichtblick-gpu'
POINTS = [(31 + x * 83, 29 + y * 61) for y in range(8) for x in range(11)]
TIME_PROPS = """(()=>{const e=document.querySelector('[data-testid="PlaybackTime-text"] input');
if(!e)return null;let f=e[Object.keys(e).find(k=>k.startsWith('__reactFiber'))];
for(;f;f=f.return)if(f.memoizedProps?.currentTime&&f.memoizedProps?.onSeek)return f.memoizedProps;
return null})()"""
STATE = f"""(()=>{{const p={TIME_PROPS};return p?{{current:p.currentTime,start:p.startTime,end:p.endTime,
playing:p.isPlaying,visibility:document.visibilityState,text:document.body.innerText}}:null}})()"""


def digest(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def capture(x11, window, rect):
    x, y, w, h = map(int, rect)
    ptr = x11.x.XGetImage(x11.display, window, x, y, w, h, c.c_ulong(-1).value, 2)
    if not ptr:
        raise RuntimeError('XGetImage failed')
    try:
        im = ptr.contents
        if (im.bits_per_pixel, im.byte_order, im.red_mask, im.green_mask, im.blue_mask) != (32, 0, 0xff0000, 0xff00, 0xff):
            raise RuntimeError('unsupported XImage format')
        return c.string_at(im.data, im.bytes_per_line * h), im.bytes_per_line
    finally:
        x11.x.XDestroyImage(ptr)


def pixels(raw, stride):
    return tuple(sum(raw[(y+dy)*stride+(x+dx)*4+channel]
                     for dy in range(-2,3) for dx in range(-2,3))/25
                 for x,y in POINTS for channel in (2,1,0))


def templates(directory):
    files = sorted(Path(directory).glob('*.jpg'))
    if len(files) != 32:
        raise ValueError('expected exactly 32 extracted JPEG variants')
    result = []
    for file in files:
        with Image.open(file) as image:
            small = image.convert('RGB').resize((960, 540), Image.Resampling.BILINEAR).filter(ImageFilter.BoxBlur(2))
            result.append(tuple(v for point in POINTS for v in small.getpixel(point)))
    return result


def identify(sample, refs):
    errors = sorted((sum(abs(a - b) for a, b in zip(sample, ref)) / len(sample), i)
                    for i, ref in enumerate(refs))
    return {'variant': errors[0][1], 'mae': errors[0][0], 'second_mae': errors[1][0]}


def wait_for(predicate, timeout, label):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(.05)
    raise RuntimeError('timeout: ' + label)


def window_for(pid, env):
    tree = subprocess.check_output(['xwininfo', '-root', '-tree'], env=env, text=True, timeout=3)
    for line in tree.splitlines():
        if 'Lichtblick' not in line and 'MCAP Camera' not in line:
            continue
        match = re.search(r'(0x[0-9a-f]+) "', line)
        if match:
            value = match[1]
            owner = subprocess.check_output(['xprop', '-id', value, '_NET_WM_PID'], env=env, text=True, timeout=3)
            if re.search(r'= ' + str(pid) + r'\b', owner):
                info = subprocess.check_output(['xwininfo','-id',value],env=env,text=True,timeout=3)
                if 'Map State: IsViewable' in info and int(re.search(r'Width: (\d+)',info)[1]) >= 600:
                    return int(value, 16)
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', choices=['native', 'lichtblick'], required=True)
    parser.add_argument('--camera-trace', action='store_true', help='enable temporary per-stage Native measurement hooks')
    parser.add_argument('--native-patch', type=Path, default=HERE/'native-measurement.patch')
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--jpeg-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--native', type=Path, default=ROOT/'target/gpu-native/viewer-native-comparison')
    parser.add_argument('--desktop', type=Path, default=Path('/tmp/mcap-lichtblick-desktop/lichtblick-1.29.1-linux-x64/lichtblick'))
    parser.add_argument('--port', type=int, default=9247)
    parser.add_argument('--wake-display', action='store_true', help='force DPMS On for this authorized run; does not unlock or change power policy')
    parser.add_argument('--seconds', type=float, default=12)
    parser.add_argument('--seek-offsets', default='310000000,1710000000,4110000000', help='three fixture-relative ns positions')
    args = parser.parse_args()
    if args.output.exists() or not 3 <= args.seconds <= 15:
        parser.error('new output directory and seconds 3..15 required')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    refs = templates(args.jpeg_dir)
    env = os.environ | {'DISPLAY': ':1', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority',
                        'VK_DRIVER_FILES': '/usr/share/vulkan/icd.d/nvidia_icd.json',
                        'RUST_LOG': 'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info'}
    result = {'app': args.app, 'status': 'running', 'dataset_sha256': digest(args.data),
              'dataset_bytes': args.data.stat().st_size, 'samples': [], 'seeks': [],
              'source_commit': subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
              'harness_sha256': digest(__file__), 'native_patch_sha256': digest(args.native_patch), 'camera_trace_enabled': args.camera_trace,
              'cache': 'cache-allowed; full SHA256 read before launch',
              'observer': 'XGetImage 960x540 BGRA; 88 RGB 5x5 patch means; 32 JPEG templates; 20 Hz target',
              'seek_boundary': 'existing UI action handler; no OS mouse/key latency; 10 ms target polling',
              'pss_limit_mib': 2048, 'wall_limit_seconds': 100}
    if args.camera_trace:
        env['MCAP_BENCH_TRACE']='1'
    process = x11 = cdp = browser = profile = None
    started = time.monotonic()
    try:
        result['display_initial'] = subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
        if args.wake_display:
            subprocess.run(['xset','dpms','force','on'],env=env,check=True,timeout=5)
        result['display_before'] = subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
        result['gpu_before'] = subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
        if 'Monitor is On' not in result['display_before']:
            raise RuntimeError('DPMS monitor must already be On')
        profile = tempfile.mkdtemp(prefix='mcap-desktop-compare-')
        control = args.output/'control.txt'
        control.write_text('1 seek 0\n')
        env |= {'MCAP_BENCH_LAYOUT': str(HERE/'native-layout.json'), 'MCAP_BENCH_CONTROL': str(control)}
        if args.app == 'native':
            command = [str(args.native.resolve()), '--mcap', str(args.data.resolve()), '--camera-topic','/camera/0','--no-platform-geometry']
        else:
            with socket.socket() as check:
                check.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                check.bind(('127.0.0.1', args.port))
            command = [str(args.desktop.resolve()), '--no-sandbox', '--force-multiple-windows',
                       '--user-data-dir='+profile, f'--remote-debugging-port={args.port}']
        result |= {'command': command, 'binary_sha256': digest(command[0])}
        with (args.output/'app.log').open('w') as log:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=log, start_new_session=True)
            result['pid'] = process.pid
            window = wait_for(lambda: window_for(process.pid, env), 15, 'owned app window')
            wait_for(lambda: 'Map State: IsViewable' in subprocess.check_output(
                ['xwininfo','-id',hex(window)],env=env,text=True,timeout=3),10,'mapped app window')
            x11 = X11(':1')
            x11.x.XMoveResizeWindow(x11.display,window,40,60,1920,1080)
            x11.x.XRaiseWindow(x11.display,window)
            x11.x.XFlush(x11.display)
            result['window'] = hex(window)
            if args.app == 'lichtblick':
                def pages():
                    try:
                        return [p for p in json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json',timeout=2)) if p['type']=='page']
                    except (OSError, ValueError):
                        return None
                page = wait_for(pages,10,'CDP page')[0]
                cdp = CDP(page['webSocketDebuggerUrl'])
                version = json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json/version',timeout=2))
                browser = CDP(version['webSocketDebuggerUrl'])
                result['gpu'] = browser.call('SystemInfo.getInfo')['gpu']
                if 'NVIDIA' not in str(result['gpu']['devices']) or result['gpu']['featureStatus'].get('webgl') != 'enabled':
                    raise RuntimeError('Lichtblick is not using NVIDIA WebGL')
                wait_for(lambda: cdp.evaluate("!!document.querySelector('input[type=file]')"),10,'file input')
                layout = {'configById': {'Image!focus': {'imageMode': {'imageTopic':'/camera/0'}}},
                          'globalVariables':{},'userNodes':{},'playbackConfig':{'speed':1},'layout':'Image!focus'}
                cdp.evaluate("(()=>{const d=new DataTransfer();d.items.add(new File(["+json.dumps(json.dumps(layout))+"],'camera.json',{type:'application/json'}));const i=document.querySelector('input[type=file]');i.files=d.files;i.dispatchEvent(new Event('change',{bubbles:true}));})()")
                time.sleep(.8)
                doc = cdp.call('DOM.getDocument')['root']['nodeId']
                node = cdp.call('DOM.querySelector',{'nodeId':doc,'selector':'input[type=file]'})['nodeId']
                result['file_input_wall_s'] = time.monotonic()-started
                cdp.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(args.data.resolve())]})
                wait_for(lambda: cdp.evaluate(STATE),15,'playback UI')
                cdp.evaluate("(()=>{const c=document.querySelector('canvas');c.parentElement.style.width='960px';c.parentElement.style.height='540px';})()")
                time.sleep(.5)
                canvases=cdp.evaluate("[...document.querySelectorAll('canvas')].map(c=>({width:c.width,height:c.height,rect:c.getBoundingClientRect().toJSON()}))")
                result['canvases']=canvases
                result['web_viewport']=cdp.evaluate('({width:innerWidth,height:innerHeight,dpr:devicePixelRatio})')
                extents=subprocess.check_output(['xprop','-id',hex(window),'_GTK_FRAME_EXTENTS'],env=env,text=True,timeout=3)
                result['gtk_frame_extents']=extents
                match=re.search(r'= (\d+), (\d+), (\d+), (\d+)',extents)
                inset=[int(match[1]),int(match[3])] if match else [0,0]
                if len(canvases)!=1 or [canvases[0]['width'],canvases[0]['height']] != [960,540]:
                    raise RuntimeError('expected one 960x540 camera canvas')
                r=canvases[0]['rect']; rect=[round(r['x'])+inset[0],round(r['y'])+inset[1],960,540]
            else:
                def native_rect():
                    content=(args.output/'app.log').read_text()
                    found=re.findall(r'GPU_BENCH_FRAME arrival=\d+ rect=\[\[([\d.]+) ([\d.]+)\] - \[([\d.]+) ([\d.]+)\]\]',content)
                    return found[-1] if found else None
                r=wait_for(native_rect,15,'Native first camera texture')
                rect=[round(float(r[0])),round(float(r[1])),round(float(r[2])-float(r[0])),round(float(r[3])-float(r[1]))]
                if rect[2:] != [960,540]:raise RuntimeError(f'Native image size {rect}')
                if not re.search(r'Request adapter result.*NVIDIA.*DiscreteGpu.*Vulkan',(args.output/'app.log').read_text()):
                    raise RuntimeError('Native did not select NVIDIA Vulkan')
            result['image_rect']=rect
            info=subprocess.check_output(['xwininfo','-id',hex(window)],env=env,text=True,timeout=3)
            result['window_info']=info
            if not all(re.search(k+r': '+str(v)+r'\b',info) for k,v in [('Width',1920),('Height',1080)]):
                raise RuntimeError('client window is not FHD')
            request=1
            def command(verb, offset=0):
                nonlocal request
                request+=1
                if args.app=='native':
                    temp=control.with_suffix('.tmp');temp.write_text(f'{request} {verb}'+(f' {offset}' if verb=='seek' else '')+'\n');temp.replace(control)
                elif verb=='seek':
                    cdp.evaluate(f"(()=>{{const p={TIME_PROPS};p.onPause();p.onSeek({{sec:1735689600+{offset//1000000000},nsec:{offset%1000000000}}});}})()")
                elif verb=='play':
                    cdp.evaluate("document.querySelector('[data-testid=play-button]').click()")
                else:
                    cdp.evaluate(f'({TIME_PROPS}).onPause()')
            def image():
                raw,stride=capture(x11,window,rect)
                return identify(pixels(raw,stride),refs)
            def confirmed_seek(offset):
                expected=(offset//100000000)%32
                before=image(); begin=time.monotonic();command('seek',offset)
                observed=[]
                while time.monotonic()-begin<4:
                    match=image(); now=time.monotonic();observed.append({'s':now-begin,**match})
                    if match['variant']==expected and match['mae']<18 and match['second_mae']-match['mae']>1:
                        return {'offset_ns':offset,'expected_variant':expected,'before':before,'latency_ms':(now-begin)*1000,'observations':observed}
                    time.sleep(.01)
                raise RuntimeError(f'Seek pixels not correct: expected {expected}, observed {observed[-3:]}')
            # Fixture-specific correctness check against three distinct known JPEGs.
            for offset in map(int,args.seek_offsets.split(',')):
                result['seeks'].append(confirmed_seek(offset))
            result['seeks'].append(confirmed_seek(0))
            result['ready_wall_s']=time.monotonic()-started
            raw,stride=capture(x11,window,[0,0,1920,1080])
            Image.frombytes('RGB',(1920,1080),raw,'raw','BGRX',stride,1).save(args.output/'ready.png')
            baseline=process_sample(process.pid);result['before_play']=baseline
            command('play'); begin=time.monotonic();last_memory=begin-1;last_variant=0;frame=0
            while time.monotonic()-begin<args.seconds:
                tick=time.monotonic();match=image();now=time.monotonic()
                valid=match['mae']<18 and match['second_mae']-match['mae']>1
                if valid:
                    delta=(match['variant']-last_variant)%32
                    if delta>16:raise RuntimeError(f'backward/ambiguous pixel identity {last_variant}->{match}')
                    frame+=delta;last_variant=match['variant']
                row={'s':now-begin,'valid_identity':valid,'capture_ms':(now-tick)*1000,'forward_frame':frame,'wall_lag_ms':((now-begin)-frame*.1)*1000,**match}
                if now-last_memory>=.25:
                    memory=process_sample(process.pid);row['resources']=memory;last_memory=now
                    if memory['pss_kib']>2048*1024:raise RuntimeError('PSS guard exceeded')
                    if cdp:row['ui']=cdp.evaluate(STATE)
                result['samples'].append(row)
                if time.monotonic()-started>100:raise RuntimeError('wall guard exceeded')
                time.sleep(max(0,.05-(time.monotonic()-tick)))
            command('pause');result['after_play']=process_sample(process.pid)
            raw,stride=capture(x11,window,[0,0,1920,1080]);Image.frombytes('RGB',(1920,1080),raw,'raw','BGRX',stride,1).save(args.output/'played.png')
            if cdp:result['final_ui']=cdp.evaluate(STATE)
            result['gpu_after']=subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
            result['display_after']=subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
            if 'Monitor is On' not in result['display_after']:raise RuntimeError('DPMS became Off')
            all_samples=result['samples'];samples=[s for s in all_samples if s['valid_identity']]
            if len(samples)<2:raise RuntimeError('insufficient confident image samples')
            elapsed=samples[-1]['s']-samples[0]['s']
            updates=sum(b['variant']!=a['variant'] for a,b in zip(samples,samples[1:]))
            resources=[s['resources'] for s in samples if 'resources'in s]
            lag=[s['wall_lag_ms'] for s in samples if s['s']>=1]
            result['summary']={'observed_updates_hz':updates/elapsed,'source_progress_hz':(samples[-1]['forward_frame']-samples[0]['forward_frame'])/elapsed,
                'wall_lag_median_ms':statistics.median(lag),'wall_lag_p95_ms':sorted(lag)[int((len(lag)-1)*.95)],
                'peak_sampled_pss_mib':max(r['pss_kib'] for r in resources)/1024,
                'cpu_seconds':result['after_play']['cpu_seconds_live_processes']-baseline['cpu_seconds_live_processes'],
                'observation_seconds':samples[-1]['s'],'samples':len(all_samples),'confident_samples':len(samples),'rejected_samples':len(all_samples)-len(samples),'max_template_mae':max(s['mae'] for s in samples),
                'minimum_identity_margin':min(s['second_mae']-s['mae'] for s in samples),
                'max_sample_gap_ms':max(b['s']-a['s'] for a,b in zip(samples,samples[1:]))*1000}
            if len(samples)<len(all_samples)*.95 or result['summary']['max_sample_gap_ms']>=1500:
                raise RuntimeError('pixel identity confidence/sampling invalid')
            result['status']='ok'
    except Exception as error:
        result['status']='failed';result['error']=repr(error)
        if x11:
            try:
                raw,stride=capture(x11,window,[0,0,1920,1080])
                Image.frombytes('RGB',(1920,1080),raw,'raw','BGRX',stride,1).save(args.output/'failed-x11.png')
            except Exception as capture_error:result['capture_error']=repr(capture_error)
        if cdp:
            try:
                result['failed_ui']=cdp.evaluate(STATE)
                (args.output/'failed-cdp.png').write_bytes(base64.b64decode(cdp.call('Page.captureScreenshot',{'format':'png'})['data']))
            except Exception as capture_error:result['cdp_capture_error']=repr(capture_error)
    finally:
        if browser and process and process.poll() is None:
            try:
                browser.sock.settimeout(2)
                browser.call('Browser.close')
            except (OSError, RuntimeError):pass
            try:process.wait(timeout=3)
            except subprocess.TimeoutExpired:pass
        for connection in (cdp,browser):
            if connection:
                try:connection.close()
                except OSError:pass
        if process and process.poll() is None:
            os.killpg(process.pid,signal.SIGTERM)
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
        if profile:shutil.rmtree(profile,ignore_errors=True)
        if process:result['exit_code']=process.poll()
        if x11:x11.x.XCloseDisplay(x11.display)
        result['total_wall_s']=time.monotonic()-started
        (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:result[k] for k in ['app','status','summary','error'] if k in result}),flush=True)
    if result['status']!='ok':raise SystemExit(1)

if __name__=='__main__':main()
