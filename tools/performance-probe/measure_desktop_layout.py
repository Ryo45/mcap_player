#!/usr/bin/env python3
"""Matched default-layout Desktop probe: grid, camera, optional scan and cursor inspection.

Uses dedicated fresh processes/profiles. Captured per-app reference patches identify
actual displayed JPEGs at the smaller default image size. Calibration is separate
from timed playback. No production renderer/decoder algorithm is substituted.
"""
import argparse
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import statistics
import subprocess
import tempfile
import time
import urllib.request
import zlib

from PIL import Image
from browser_cdp import CDP
from check_native_gpu import X11
from compare_native_desktop import capture, digest, identify, wait_for, window_for, TIME_PROPS, STATE
from measure_viewer_pilot import process_sample

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT/'docs/performance/2026-10-20-desktop-default-layout'
BASE = 1735689600000000000
SUBSCRIPTIONS = """(()=>{const e=document.querySelector('[data-testid="PlaybackTime-text"] input');
let f=e?.[Object.keys(e).find(k=>k.startsWith('__reactFiber'))];
for(;f;f=f.return){const v=f.memoizedProps?.value;if(typeof v?.getState==='function'){
const s=v.getState();if(Array.isArray(s.public?.subscriptions))return s.public.subscriptions;}}return null})()"""


def import_layout(cdp, layout):
    cdp.evaluate("(()=>{const d=new DataTransfer();d.items.add(new File(["+json.dumps(json.dumps(layout))+"],'layout.json',{type:'application/json'}));const i=document.querySelector('input[type=file]');i.files=d.files;i.dispatchEvent(new Event('change',{bubbles:true}));})()")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', choices=['native', 'lichtblick'], required=True)
    parser.add_argument('--case', choices=['default', 'scan', 'raw'], required=True)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--calibrate', action='store_true')
    parser.add_argument('--jpeg-dir', type=Path)
    parser.add_argument('--references', type=Path)
    parser.add_argument('--port', type=int, default=9281)
    parser.add_argument('--seconds', type=float, default=12)
    args = parser.parse_args()
    if args.output.exists() or not 3 <= args.seconds <= 15 or (not args.calibrate and not args.references):
        parser.error('new output, duration 3..15, and references (or --calibrate) required')
    if args.calibrate and not args.jpeg_dir:
        parser.error('--jpeg-dir required for source hash verification during calibration')
    args.output = args.output.resolve(); args.output.mkdir(parents=True)
    geometry = json.loads((REPORT/'geometry.json').read_text()); roi = geometry['camera_roi']
    points = [(round(8+x*(roi[2]-17)/10), round(8+y*(roi[3]-17)/7)) for y in range(8) for x in range(11)]
    result = dict(status='running', app=args.app, case=args.case, calibration=args.calibrate,
                  data_sha256=digest(args.data), data_bytes=args.data.stat().st_size,
                  geometry=geometry, harness_sha256=digest(__file__), native_patch_sha256=digest(REPORT/'native-measurement.patch'),
                  source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                  samples=[], seeks=[], cache='cache-allowed; SHA256-read before launch',
                  inspector_policy='latest-before cursor, arrays/header collapsed; Native exact queries versus Lichtblick stream subscription',
                  observation='20 Hz image identity from 88 patch means; Scene CRC change is a lower bound, not a timestamp or GPU FPS')
    env=os.environ|{'DISPLAY':':1','XAUTHORITY':'/run/user/1000/gdm/Xauthority',
                    'VK_DRIVER_FILES':'/usr/share/vulkan/icd.d/nvidia_icd.json',
                    'RUST_LOG':'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info',
                    'MCAP_BENCH_LAYOUT':str(REPORT/f'native-{args.case}.json'),
                    'MCAP_BENCH_GEOMETRY':str(REPORT/'geometry.json'),'MCAP_BENCH_MODE':args.case}
    process=cdp=browser=x11=profile=None; started=time.monotonic(); window=None
    try:
        subprocess.run(['xset','dpms','force','on'],env=env,check=True,timeout=5)
        result['display_before']=subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
        result['gpu_before']=subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
        if 'Monitor is On' not in result['display_before']:raise RuntimeError('DPMS not On')
        profile=tempfile.mkdtemp(prefix='mcap-layout-probe-');control=args.output/'control.txt';control.write_text('1 seek 0\n')
        env['MCAP_BENCH_CONTROL']=str(control)
        if args.app=='native':
            command=[str(ROOT/'target/gpu-native/viewer-native-default-layout'),'--mcap',str(args.data.resolve()),
                     '--camera-topic','/camera/0','--no-platform-geometry','--fixed-frame','base_scan']
        else:
            command=['/tmp/mcap-lichtblick-desktop/lichtblick-1.29.1-linux-x64/lichtblick','--no-sandbox',
                     '--force-multiple-windows','--user-data-dir='+profile,f'--remote-debugging-port={args.port}']
        result.update(command=command,binary_sha256=digest(command[0]))
        with (args.output/'app.log').open('w') as log:
            process=subprocess.Popen(command,cwd=ROOT,env=env,stdout=log,stderr=log,start_new_session=True)
            result['pid']=process.pid
            window=wait_for(lambda:window_for(process.pid,env),15,'owned mapped window');x11=X11(':1')
            size=[1920,1080] if args.app=='native' else [1952,1122]
            x11.x.XMoveResizeWindow(x11.display,window,40,60,*size);x11.x.XFlush(x11.display)
            result['window']=hex(window);inset=[0,0]
            if args.app=='lichtblick':
                def page():
                    try:return next((p for p in json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json',timeout=2)) if p['type']=='page'),None)
                    except OSError:return None
                cdp=CDP(wait_for(page,10,'CDP page')['webSocketDebuggerUrl'])
                version=json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json/version',timeout=2));browser=CDP(version['webSocketDebuggerUrl'])
                result['gpu']=browser.call('SystemInfo.getInfo')['gpu']
                if 'NVIDIA' not in str(result['gpu']['devices']) or result['gpu']['featureStatus'].get('webgl')!='enabled':raise RuntimeError('hardware WebGL required')
                wait_for(lambda:cdp.evaluate("!!document.querySelector('input[type=file]')"),10,'file input')
                if args.calibrate:
                    # Hash only calibration JPEG inputs. Timed runs never install this wrapper.
                    cdp.evaluate("window.layoutDecodedHash=null;window.layoutScanDraws=0;const originalBitmap=createImageBitmap;window.createImageBitmap=async function(b,...a){const image=await originalBitmap.call(this,b,...a);if(b instanceof Blob&&b.type.includes('jpeg')){const h=await crypto.subtle.digest('SHA-256',await b.arrayBuffer());window.layoutDecodedHash=[...new Uint8Array(h)].map(x=>x.toString(16).padStart(2,'0')).join('');}return image};for(const C of [WebGLRenderingContext,WebGL2RenderingContext]){const f=C.prototype.drawArrays;C.prototype.drawArrays=function(m,s,n){if(m===0&&n===360)window.layoutScanDraws++;return f.call(this,m,s,n)}}")
                if args.case!='default':
                    import_layout(cdp,json.loads((REPORT/f'lichtblick-{args.case}.json').read_text()));time.sleep(.5)
                doc=cdp.call('DOM.getDocument')['root']['nodeId'];node=cdp.call('DOM.querySelector',{'nodeId':doc,'selector':'input[type=file]'})['nodeId']
                cdp.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(args.data.resolve())]})
                wait_for(lambda:cdp.evaluate(STATE),15,'recording ready')
                time.sleep(.5)
                result['layout']=cdp.evaluate("JSON.parse(localStorage.getItem('studio.layout'))")
                result['subscriptions']=cdp.evaluate(SUBSCRIPTIONS)
                if result['subscriptions'] is None:raise RuntimeError('cannot audit Lichtblick subscriptions')
                selected=sorted(set(v['topic']for v in result['subscriptions']if re.fullmatch(r'/camera/[0-7]|/scan',v['topic'])))
                result['selected_recording_topics']=selected
                if selected!=(['/camera/0']if args.case=='default'else['/camera/0','/scan']):raise RuntimeError('Lichtblick selection mismatch')
                result['viewport']=cdp.evaluate('[innerWidth,innerHeight,devicePixelRatio]')
                result['canvases']=cdp.evaluate("[...document.querySelectorAll('canvas')].map(c=>({width:c.width,height:c.height,rect:c.getBoundingClientRect().toJSON()}))")
                if result['viewport']!=[1920,1080,1] or [[p['width'],p['height']]for p in result['canvases']]!=[[1120,926],[480,255]]:raise RuntimeError('default viewport changed')
                extents=subprocess.check_output(['xprop','-id',hex(window),'_GTK_FRAME_EXTENTS'],env=env,text=True,timeout=3)
                match=re.search(r'= (\d+), (\d+), (\d+), (\d+)',extents)
                if not match:raise RuntimeError('missing GTK extents')
                inset=[int(match[1]),int(match[3])];result['gtk_frame_extents']=extents
            else:
                wait_for(lambda:'GPU_BENCH_FRAME'in (args.output/'app.log').read_text(),15,'Native camera')
                text=(args.output/'app.log').read_text();result['selection']=next(line for line in text.splitlines()if'GPU_BENCH_SELECTION'in line)
                if not re.search(r'Request adapter result.*NVIDIA.*DiscreteGpu.*Vulkan',text):raise RuntimeError('NVIDIA Vulkan required')
                if ('cloud=None'in result['selection'])!=(args.case=='default'):raise RuntimeError('Native selection mismatch')
            request=1
            def send(verb,ns=0):
                nonlocal request
                request+=1
                if args.app=='native':
                    temp=control.with_suffix('.tmp');temp.write_text(f'{request} {verb}'+(f' {ns}'if verb=='seek'else'')+'\n');temp.replace(control)
                elif verb=='seek':cdp.evaluate(f"(()=>{{const p={TIME_PROPS};p.onPause();p.onSeek({{sec:1735689600+{ns//1000000000},nsec:{ns%1000000000}}})}})()")
                elif verb=='play':cdp.evaluate("document.querySelector('[data-testid=play-button]').click()")
                else:cdp.evaluate(f'({TIME_PROPS}).onPause()')
            def image_sample():
                rect=[roi[0]+inset[0],roi[1]+inset[1],roi[2],roi[3]];raw,stride=capture(x11,window,rect)
                return tuple(sum(raw[(y+dy)*stride+(x+dx)*4+k]for dy in range(-2,3)for dx in range(-2,3))/25 for x,y in points for k in (2,1,0))
            def screenshot(name):
                raw,stride=capture(x11,window,[inset[0],inset[1],1920,1080]);Image.frombytes('RGB',(1920,1080),raw,'raw','BGRX',stride,1).save(args.output/name)
            if args.calibrate:
                refs=[];hashes=[digest(args.jpeg_dir/f'{i:02}.jpg')for i in range(32)]
                for index in range(32):
                    ns=index*100000000+10000000;send('seek',ns);time.sleep(.14)
                    if cdp:wait_for(lambda:cdp.evaluate('window.layoutDecodedHash')==hashes[index],3,'decoded JPEG source identity')
                    else:wait_for(lambda:f'GPU_BENCH_FRAME arrival={BASE+index*100000000} 'in(args.output/'app.log').read_text(),3,'texture source identity')
                    previous=image_sample()
                    for attempt in range(30):
                        time.sleep(.025);current=image_sample()
                        if max(abs(a-b)for a,b in zip(previous,current))<.15:break
                        previous=current
                    else:raise RuntimeError('unstable calibration pixels')
                    refs.append(current)
                # Every reference must be distinguishable from every other reference.
                minimum=min(identify(ref,[other for j,other in enumerate(refs)if j!=i])['mae']for i,ref in enumerate(refs))
                if minimum<=1:raise RuntimeError('nonunique reference images')
                references=dict(app=args.app,binary_sha256=result['binary_sha256'],dataset_sha256=result['data_sha256'],geometry=geometry,points=points,jpeg_sha256=hashes,references=refs,minimum_pair_mae=minimum)
                (args.output/'references.json').write_text(json.dumps(references)+'\n');result['reference_minimum_pair_mae']=minimum
                if cdp:
                    result['calibration_scan_draws']=cdp.evaluate('window.layoutScanDraws')
                    if args.case!='default' and not result['calibration_scan_draws']:raise RuntimeError('scan never drew 360 points')
                screenshot('calibrated.png');result['status']='ok';return
            references=json.loads(args.references.read_text());refs=references['references']
            if references['app']!=args.app or references['binary_sha256']!=result['binary_sha256'] or references['dataset_sha256']!=result['data_sha256'] or references['geometry']!=geometry:raise RuntimeError('reference provenance mismatch')
            result['references_sha256']=digest(args.references)
            for ns in [25310000000,48710000000,16110000000,0]:
                expected=ns//100000000%32;begin=time.monotonic();send('seek',ns)
                while time.monotonic()-begin<4:
                    match=identify(image_sample(),refs)
                    if match['variant']==expected and match['mae']<18 and match['second_mae']-match['mae']>1:break
                    time.sleep(.01)
                else:raise RuntimeError('seek pixels mismatch')
                result['seeks'].append(dict(offset_ns=ns,latency_ms=(time.monotonic()-begin)*1000,**match))
            if cdp:
                result['before_ui']=cdp.evaluate(STATE)
                text=result['before_ui']['text']
                if args.case=='raw' and ('LaserScan @'not in text or '360 keys'not in text):raise RuntimeError('raw scan data not visible')
            screenshot('ready.png');baseline=process_sample(process.pid);result['before_play']=baseline
            send('play');begin=time.monotonic();last_resources=begin-1;last_variant=frame=0
            while time.monotonic()-begin<args.seconds:
                tick=time.monotonic();match=identify(image_sample(),refs);now=time.monotonic()
                valid=match['mae']<18 and match['second_mae']-match['mae']>1
                if valid:
                    delta=(match['variant']-last_variant)%32
                    if delta>16:raise RuntimeError('backward/ambiguous image progression')
                    frame+=delta;last_variant=match['variant']
                sr=geometry['scene'];raw,_=capture(x11,window,[sr[0]+inset[0],sr[1]+inset[1],sr[2],sr[3]])
                row=dict(s=now-begin,valid_identity=valid,forward_frame=frame,wall_lag_ms=(now-begin-frame*.1)*1000,scene_crc=zlib.crc32(raw),**match)
                if now-last_resources>=.25:
                    row['resources']=process_sample(process.pid);last_resources=now
                    if row['resources']['pss_kib']>2048*1024:raise RuntimeError('2 GiB PSS guard')
                    if cdp:
                        row['ui']=cdp.evaluate(STATE)
                        if args.case=='raw':
                            stamp=re.search(r'LaserScan @ (\d+)\.(\d+) sec',row['ui']['text'])
                            if not stamp:raise RuntimeError('raw message timestamp absent')
                            raw_time=int(stamp[1])*1000000000+int(stamp[2].ljust(9,'0'))
                            cursor=row['ui']['current']['sec']*1000000000+row['ui']['current']['nsec']
                            row['raw_log_ns']=raw_time;row['raw_lag_ms']=(cursor-raw_time)/1e6
                    elif args.case=='raw':
                        content=(args.output/'app.log').read_text()
                        times=re.findall(r'GPU_BENCH_CURSOR cursor=(\d+)',content)
                        latest=re.findall(r'GPU_BENCH_INSPECTOR cursor=(\d+) log=(\d+)',content)
                        if not times or not latest:raise RuntimeError('Native inspector timestamp absent')
                        row['raw_log_ns']=int(latest[-1][1]);row['raw_lag_ms']=(int(times[-1])-row['raw_log_ns'])/1e6
                row['observer_work_ms']=(time.monotonic()-tick)*1000;result['samples'].append(row)
                if time.monotonic()-started>100:raise RuntimeError('wall deadline')
                time.sleep(max(0,.05-(time.monotonic()-tick)))
            send('pause');result['after_play']=process_sample(process.pid);screenshot('played.png')
            result['gpu_after']=subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
            result['display_after']=subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
            if 'Monitor is On'not in result['display_after']:raise RuntimeError('DPMS became Off')
            good=[s for s in result['samples']if s['valid_identity']]
            if len(good)<len(result['samples'])*.95:raise RuntimeError('image identity unreliable')
            elapsed=good[-1]['s']-good[0]['s'];lag=sorted(s['wall_lag_ms']for s in good if s['s']>=1)
            changes=sum(a['variant']!=b['variant']for a,b in zip(good,good[1:]));rows=result['samples']
            scene_changes=sum(a['scene_crc']!=b['scene_crc']for a,b in zip(rows,rows[1:]))
            result['summary']=dict(image_updates_hz=changes/elapsed,progress_hz=(good[-1]['forward_frame']-good[0]['forward_frame'])/elapsed,
                                   lag_median_ms=statistics.median(lag),lag_p95_ms=lag[int((len(lag)-1)*.95)],
                                   scene_changes_hz=scene_changes/(rows[-1]['s']-rows[0]['s']),
                                   peak_sampled_pss_mib=max(r['resources']['pss_kib']/1024 for r in rows if 'resources'in r),
                                   cpu_one_core_percent=100*(result['after_play']['cpu_seconds_live_processes']-baseline['cpu_seconds_live_processes'])/rows[-1]['s'],
                                   rejected_samples=len(rows)-len(good),samples=len(rows),max_sample_gap_ms=max(b['s']-a['s']for a,b in zip(good,good[1:]))*1000)
            if args.case=='raw':
                raw_lag=sorted(s['raw_lag_ms']for s in rows if 'raw_lag_ms'in s and s['s']>=1)
                if not raw_lag or min(raw_lag)<-20:raise RuntimeError('invalid raw-message timing')
                result['summary']['raw_lag_median_ms']=statistics.median(raw_lag)
                result['summary']['raw_lag_p95_ms']=raw_lag[int((len(raw_lag)-1)*.95)]
            if result['summary']['max_sample_gap_ms']>=1500:raise RuntimeError('ambiguous cyclic image identity')
            if args.case=='default' and scene_changes>1:raise RuntimeError('unconfigured grid unexpectedly changed')
            if args.case!='default' and result['summary']['scene_changes_hz']<4:raise RuntimeError('scan did not visibly update')
            result['status']='ok'
    except Exception as error:
        result['status']='failed';result['error']=repr(error)
        if x11 and window:
            try:screenshot('failed.png')
            except Exception:pass
        if cdp:
            try:result['failed_ui']=cdp.evaluate(STATE)
            except Exception:pass
    finally:
        if browser and process and process.poll()is None:
            try:browser.sock.settimeout(2);browser.call('Browser.close')
            except (OSError,RuntimeError):pass
            try:process.wait(timeout=3)
            except subprocess.TimeoutExpired:pass
        for connection in [cdp,browser]:
            if connection:
                try:connection.close()
                except OSError:pass
        if process and process.poll()is None:
            os.killpg(process.pid,signal.SIGTERM)
            try:process.wait(timeout=3)
            except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=3)
        if x11:x11.x.XCloseDisplay(x11.display)
        if profile:shutil.rmtree(profile,ignore_errors=True)
        if process:result['exit_code']=process.poll()
        result['total_wall_s']=time.monotonic()-started
        (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps({k:result[k]for k in ['app','case','status','summary','error']if k in result}),flush=True)
    if result['status']!='ok':raise SystemExit(1)


if __name__=='__main__':main()
