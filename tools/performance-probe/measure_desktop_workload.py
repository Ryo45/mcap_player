#!/usr/bin/env python3
"""Actual Desktop workloads: matched viewports, per-camera pixels, bounded fresh processes."""
import argparse
import json
import math
import os
from pathlib import Path
import re
import queue
import shutil
import signal
import socket
import statistics
import subprocess
import tempfile
import threading
import time
import urllib.request
import zlib

from PIL import Image
from browser_cdp import CDP
from check_native_gpu import X11
from compare_native_desktop import capture, digest, identify, wait_for, window_for
from measure_desktop_layout import import_layout, SUBSCRIPTIONS
from measure_viewer_pilot import process_sample

ROOT=Path(__file__).resolve().parents[2]
REPORT=ROOT/'docs/performance/2026-10-20-desktop-workloads'
BASE=1735689600000000000

# Read the live MessagePipeline store, rather than paused React memoized props,
# which can reference the previous render.
PUBLIC = """(()=>{const e=document.querySelector('[data-testid="PlaybackTime-text"] input');
let f=e?.[Object.keys(e).find(k=>k.startsWith('__reactFiber'))];
for(;f;f=f.return){const v=f.memoizedProps?.value;if(typeof v?.getState==='function'){
const p=v.getState().public;if(p?.playerState?.activeData)return p;}}return null})()"""
STATE = f"""(()=>{{const p={PUBLIC};const a=p?.playerState.activeData;return a?{{current:a.currentTime,start:a.startTime,end:a.endTime,
playing:a.isPlaying,visibility:document.visibilityState,text:document.body.innerText}}:null}})()"""


def distribution(values):
    values=sorted(values)
    return {'median':statistics.median(values),'p95':values[int((len(values)-1)*.95)],'min':values[0],'max':values[-1]}


class PssSampler:
    """One in-flight /proc read; slow smaps_rollup must not stop pixel observation."""
    def __init__(self, pid, origin):
        self.pid, self.origin = pid, origin
        self.stop_event = threading.Event()
        self.ready = queue.Queue(maxsize=8)
        self.dropped = 0
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        while not self.stop_event.is_set():
            start = time.monotonic()
            try:
                value = {'started_s': start - self.origin, 'resources': process_sample(self.pid)}
                value['completed_s'] = time.monotonic() - self.origin
            except Exception as error:
                value = {'error': repr(error)}
            try:
                self.ready.put_nowait(value)
            except queue.Full:
                self.dropped += 1
            self.stop_event.wait(max(0, .25 - (time.monotonic() - start)))

    def drain(self):
        values = []
        while True:
            try:
                values.append(self.ready.get_nowait())
            except queue.Empty:
                return values


def camera_geometry(rect,index):
    x,y,w,h=[rect[k]for k in ['x','y','width','height']]
    scale=min(w/16,h/9);pw,ph=16*scale,9*scale;px=x+(w-pw)/2;py=y+(h-ph)/2
    left,top=math.ceil(px+2),math.ceil(py+2)
    width,height=math.floor(px+pw-2)-left,math.floor(py+ph-2)-top
    points=[(round(8+xx*(width-17)/10),round(8+yy*(height-17)/7))for yy in range(8)for xx in range(11)]
    return {'camera_id':index,'canvas':[x,y,w,h],'picture':[px,py,pw,ph],'roi':[left,top,width,height],'points':points,'patch_radius':0 if height<150 else 2}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case',type=Path,required=True)
    parser.add_argument('--app',choices=['native','lichtblick'],required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--calibrate',action='store_true')
    parser.add_argument('--jpeg-dir',type=Path)
    parser.add_argument('--references',type=Path)
    parser.add_argument('--seconds',type=float,default=12)
    parser.add_argument('--port',type=int,default=9370)
    args=parser.parse_args()
    case=json.loads(args.case.read_text());data=Path(case['data']);kind=case['kind']
    if args.output.exists() or not 3<=args.seconds<=15 or (args.calibrate and kind=='camera'and not args.jpeg_dir):
        parser.error('new output, 3..15 seconds, and JPEG directory for camera calibration required')
    if not args.calibrate and kind=='camera'and not args.references:parser.error('camera references required')
    if args.app=='native'and not case.get('images')and kind=='camera':parser.error('Lichtblick calibration must resolve geometry first')
    args.output=args.output.resolve();args.output.mkdir(parents=True)
    result={'status':'running','app':args.app,'case':case['name'],'kind':kind,'calibration':args.calibrate,
            'data_sha256':digest(data),'data_bytes':data.stat().st_size,'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            'harness_sha256':digest(__file__),'native_patch_sha256':digest(REPORT/'native-measurement.patch'),
            'cache':'cache-allowed; SHA256 read before launch','samples':[],'seeks':[],'pss_samples':[],
            'resource_sampler_sha256':digest(Path(__file__).with_name('measure_viewer_pilot.py')),
            'limits':{'timed_pss_mib':4096,'timed_summed_rss_mib':16384,'timed_seconds':args.seconds,'wall_seconds':180},
            'camera_policy':'Native focus 10Hz/background <=5Hz; Lichtblick receives all displayed 10Hz streams',
            'color_profile':'srgb'if args.app=='lichtblick'else'Native source sRGB'}
    env=os.environ|{'DISPLAY':':1','XAUTHORITY':'/run/user/1000/gdm/Xauthority','VK_DRIVER_FILES':'/usr/share/vulkan/icd.d/nvidia_icd.json',
                    'RUST_LOG':'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info','MCAP_BENCH_GEOMETRY':str(args.case.resolve()),
                    'MCAP_BENCH_RAW_EXPAND':'1'if case.get('expanded')else'0'}
    processes=[];pss_sampler=None;cdp=browser=x11=window=None;profile=None;started=time.monotonic();request=1;inset=[0,0]
    control=args.output/'control.txt';control.write_text('1 seek 0\n')
    layout=args.output/'native-layout.json';layout.write_text(json.dumps(case['native_layout']))
    env.update(MCAP_BENCH_CONTROL=str(control),MCAP_BENCH_LAYOUT=str(layout))
    try:
        if args.app=='lichtblick':
            with socket.socket()as probe:probe.bind(('127.0.0.1',args.port))
        subprocess.run(['xset','dpms','force','on'],env=env,check=True,timeout=5)
        result['display_before']=subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
        result['gpu_before']=subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
        if 'Monitor is On'not in result['display_before']:raise RuntimeError('DPMS not On')
        profile=tempfile.mkdtemp(prefix='mcap-desktop-workload-')
        if args.app=='native':
            command=[str(ROOT/'target/gpu-native/viewer-native-workloads'),'--mcap',str(data.resolve()),
                     '--camera-topic','/camera/0','--fixed-frame','base_scan','--no-platform-geometry',
                     '--point-cloud-topic',case['topics'][0]if kind=='scene'else'/scan']
        else:
            command=['/tmp/mcap-lichtblick-desktop/lichtblick-1.29.1-linux-x64/lichtblick','--no-sandbox','--force-multiple-windows','--force-color-profile=srgb',
                     '--user-data-dir='+profile,f'--remote-debugging-port={args.port}']
        result.update(command=command,binary_sha256=digest(command[0]))
        with (args.output/'app.log').open('w')as log:
            process=subprocess.Popen(command,cwd=ROOT,env=env,stdout=log,stderr=log,start_new_session=True);processes.append(process)
            result['pid']=process.pid
            window=wait_for(lambda:window_for(process.pid,env),15,'owned mapped window');x11=X11(':1')
            size=[1920,1080]if args.app=='native'else[1952,1122]
            x11.x.XMoveResizeWindow(x11.display,window,40,60,*size);x11.x.XFlush(x11.display);result['window']=hex(window)
            if args.app=='lichtblick':
                def page():
                    try:return next((p for p in json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json',timeout=2))if p['type']=='page'),None)
                    except OSError:return None
                cdp=CDP(wait_for(page,15,'CDP page')['webSocketDebuggerUrl'])
                version=json.load(urllib.request.urlopen(f'http://127.0.0.1:{args.port}/json/version'))
                browser=CDP(version['webSocketDebuggerUrl']);result['gpu']=browser.call('SystemInfo.getInfo')['gpu']
                if 'NVIDIA'not in str(result['gpu']['devices'])or result['gpu']['featureStatus'].get('webgl')!='enabled':raise RuntimeError('hardware WebGL required')
                wait_for(lambda:cdp.evaluate("!!document.querySelector('input[type=file]')"),10,'file input')
                if args.calibrate:
                    cdp.evaluate("window.workloadHashes=[];window.workloadDraws=[];const bitmap=createImageBitmap;window.createImageBitmap=async function(b,...a){const image=await bitmap.call(this,b,...a);if(b instanceof Blob&&b.type.includes('jpeg')){const h=await crypto.subtle.digest('SHA-256',await b.arrayBuffer());window.workloadHashes.push([...new Uint8Array(h)].map(x=>x.toString(16).padStart(2,'0')).join(''));}return image};for(const C of [WebGLRenderingContext,WebGL2RenderingContext]){const f=C.prototype.drawArrays;C.prototype.drawArrays=function(m,s,n){if(m===0)window.workloadDraws.push(n);return f.call(this,m,s,n)}}")
                import_layout(cdp,case['lichtblick_layout']);time.sleep(.3)
                doc=cdp.call('DOM.getDocument')['root']['nodeId'];node=cdp.call('DOM.querySelector',{'nodeId':doc,'selector':'input[type=file]'})['nodeId']
                cdp.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(data.resolve())]})
                wait_for(lambda:cdp.evaluate(STATE),20,'recording ready');time.sleep(.5)
                result['subscriptions']=cdp.evaluate(SUBSCRIPTIONS)
                if result['subscriptions']is None:raise RuntimeError('cannot audit subscriptions')
                selected=sorted({v['topic']for v in result['subscriptions']if re.fullmatch(r'/camera/[0-7]|/scan|/points',v['topic'])})
                if selected!=sorted(case['topics']):raise RuntimeError('selection mismatch: '+str(selected))
                result['selected_recording_topics']=selected
                result['viewport']=cdp.evaluate('[innerWidth,innerHeight,devicePixelRatio]')
                canvases=cdp.evaluate("[...document.querySelectorAll('canvas')].map(c=>({width:c.width,height:c.height,rect:c.getBoundingClientRect().toJSON()}))")
                result['canvases']=canvases
                if result['viewport']!=[1920,1080,1]or len(canvases)!=case['cameras']+(kind=='scene'):raise RuntimeError('viewport/canvas mismatch')
                extents=subprocess.check_output(['xprop','-id',hex(window),'_GTK_FRAME_EXTENTS'],env=env,text=True,timeout=3)
                match=re.search(r'= (\d+), (\d+), (\d+), (\d+)',extents)
                if not match:raise RuntimeError('missing GTK extents')
                inset=[int(match[1]),int(match[3])];result['gtk_frame_extents']=extents
                if args.calibrate:
                    if kind=='camera':
                        case['images']=[camera_geometry(p['rect'],i)for i,p in enumerate(canvases)]
                        for image in case['images']:case['rects']['camera_'+str(image['camera_id'])]=image['canvas']
                    elif kind=='scene':
                        r=canvases[0]['rect'];case['rects']['scene']=[r[k]for k in ['x','y','width','height']]
                    else:case['rects']['raw']=[320,76,1600,926]
                    args.case.write_text(json.dumps(case,indent=2)+'\n')
                else:
                    rects=[p['canvas']for p in case.get('images',[])]if kind=='camera'else([case['rects']['scene']]if kind=='scene'else[])
                    actual=[[p['rect'][k]for k in ['x','y','width','height']]for p in canvases]
                    if any(max(abs(a-b)for a,b in zip(x,y))>.1 for x,y in zip(actual,rects)):raise RuntimeError('viewport geometry changed')
            else:
                wait_for(lambda:'GPU_BENCH_COMMAND id=1'in(args.output/'app.log').read_text(),20,'Native recording ready')
                text=(args.output/'app.log').read_text()
                if not re.search(r'Request adapter result.*NVIDIA.*DiscreteGpu.*Vulkan',text):raise RuntimeError('NVIDIA Vulkan required')
                result['selection']=next(line for line in text.splitlines()if'GPU_BENCH_SELECTION'in line)
                selected=re.findall(r'CameraId\(\d+\), "([^"]+)"',result['selection'])
                cloud=re.search(r'cloud=Some\("([^"]+)"\)',result['selection'])
                if cloud:selected.append(cloud[1])
                result['selected_stream_topics']=sorted(selected)
                if kind!='raw'and sorted(selected)!=sorted(case['topics']):raise RuntimeError('Native selection mismatch: '+str(selected))
            result['case_sha256']=digest(args.case);result['geometry']=case['rects']

            def send(verb,ns=0):
                nonlocal request
                request+=1
                if args.app=='native':
                    temp=control.with_suffix('.tmp');temp.write_text(f'{request} {verb}'+(f' {ns}'if verb=='seek'else'')+'\n');temp.replace(control)
                elif verb=='seek':
                    if args.calibrate and kind=='camera':cdp.evaluate('window.workloadHashes=[]')
                    cdp.evaluate(f"(()=>{{const p={PUBLIC};p.pausePlayback();p.seekPlayback({{sec:1735689600+{ns//1000000000},nsec:{ns%1000000000}}})}})()")
                elif verb=='play':cdp.evaluate("document.querySelector('[data-testid=play-button]').click()")
                else:cdp.evaluate(f'({PUBLIC}).pausePlayback()')

            def samples():
                values=[]
                for image in case.get('images',[]):
                    roi=image['roi'];raw,stride=capture(x11,window,[roi[0]+inset[0],roi[1]+inset[1],roi[2],roi[3]])
                    radius=image.get('patch_radius',2);offsets=range(-radius,radius+1)
                    values.append(tuple(sum(raw[(y+dy)*stride+(x+dx)*4+k]for dy in offsets for dx in offsets)/(2*radius+1)**2 for x,y in image['points']for k in (2,1,0)))
                return values

            def screenshot(name):
                raw,stride=capture(x11,window,[inset[0],inset[1],1920,1080])
                Image.frombytes('RGB',(1920,1080),raw,'raw','BGRX',stride,1).save(args.output/name)

            if args.calibrate:
                if kind=='camera':
                    refs=[[None]*32 for _ in range(case['cameras'])]
                    hashes=[digest(args.jpeg_dir/f'{i:02}.jpg')for i in range(32)]
                    for index in range(32):
                        send('seek',index*100000000+10000000);time.sleep(.15)
                        if cdp:
                            expected={hashes[(index+i*7)%32]for i in range(case['cameras'])}
                            wait_for(lambda:expected.issubset(set(cdp.evaluate('window.workloadHashes'))),5,'JPEG input hashes')
                            target={'sec':1735689600+index//10,'nsec':index%10*100000000+10000000}
                            wait_for(lambda:cdp.evaluate(STATE)['current']==target,5,'completed calibration seek')
                            time.sleep(.2)
                        else:
                            wait_for(lambda:all(f'GPU_BENCH_FRAME camera={i} arrival={BASE+index*100000000} 'in(args.output/'app.log').read_text()for i in range(case['cameras'])),5,'all texture timestamps')
                        before=samples()
                        for attempt in range(40):
                            time.sleep(.025);current=samples()
                            if max(abs(a-b)for x,y in zip(before,current)for a,b in zip(x,y))<.15:break
                            before=current
                        else:raise RuntimeError('unstable camera calibration')
                        for i,vector in enumerate(current):refs[i][(index+i*7)%32]=vector
                    minimum=min(identify(ref,[v for j,v in enumerate(per_camera)if j!=i])['mae']for per_camera in refs for i,ref in enumerate(per_camera))
                    result['minimum_pair_mae']=minimum
                    (args.output/'diagnostic-references.json').write_text(json.dumps(refs)+'\\n')
                    if minimum<=1:raise RuntimeError('nonunique calibration')
                    reference={'app':args.app,'binary_sha256':result['binary_sha256'],'case_sha256':result['case_sha256'],'dataset_sha256':result['data_sha256'],
                               'references':refs,'minimum_pair_mae':minimum,'color_profile':result['color_profile'],'source_jpeg_sha256':hashes}
                    (args.output/'references.json').write_text(json.dumps(reference)+'\n')
                    result['minimum_pair_mae']=minimum
                else:
                    send('seek',310000000);time.sleep(.5)
                    if cdp and kind=='scene':
                        result['point_draw_counts']=cdp.evaluate('window.workloadDraws')
                        if case['points']not in result['point_draw_counts']:raise RuntimeError('expected point count never drawn')
                screenshot('calibrated.png');result['status']='ok';return

            refs=None
            if kind=='camera':
                reference=json.loads(args.references.read_text())
                for key,expected in [('app',args.app),('binary_sha256',result['binary_sha256']),('case_sha256',result['case_sha256']),('dataset_sha256',result['data_sha256'])]:
                    if reference[key]!=expected:raise RuntimeError('reference provenance mismatch: '+key)
                if reference.get('color_profile')!=result['color_profile']:raise RuntimeError('reference color profile mismatch')
                refs=reference['references'];result['references_sha256']=digest(args.references)
            offsets=[25310000000,48710000000,16110000000,0]if kind=='camera'else[13310000000,18710000000,6110000000,0]
            for ns in offsets:
                begin=time.monotonic();send('seek',ns)
                if refs:
                    while time.monotonic()-begin<6:
                        matches=[identify(v,r)for v,r in zip(samples(),refs)]
                        if all(m['variant']==(ns//100000000+i*7)%32 and m['mae']<18 and m['second_mae']-m['mae']>1 for i,m in enumerate(matches)):break
                        time.sleep(.01)
                    else:raise RuntimeError('seek pixels mismatch')
                    seek_result={'offset_ns':ns,'latency_ms':(time.monotonic()-begin)*1000,'images':matches}
                    if args.app=='native':
                        expected=BASE+ns//100000000*100000000
                        def exact_textures():
                            text=(args.output/'app.log').read_text()
                            stamps=[re.findall(rf'GPU_BENCH_FRAME camera={i} arrival=(\d+) ',text)for i in range(case['cameras'])]
                            return all(v and int(v[-1])==expected for v in stamps)
                        wait_for(exact_textures,2,'exact Native seek texture log times')
                        seek_result['native_texture_log_ns']=[expected]*case['cameras']
                    result['seeks'].append(seek_result)
                else:
                    time.sleep(.25)
                    result['seeks'].append({'offset_ns':ns,'note':'command issued; message metadata audited, not pixel-identity seek latency'})
            if cdp:
                result['before_ui']=cdp.evaluate(STATE)
                if result['before_ui']['current']!={'sec':1735689600,'nsec':0}:raise RuntimeError('playback did not return to start')
                if kind=='raw'and('LaserScan @'not in result['before_ui']['text']or'64 keys'not in result['before_ui']['text']and not case.get('expanded')):raise RuntimeError('Raw message absent')
            else:
                text=(args.output/'app.log').read_text()
                cursors=re.findall(r'GPU_BENCH_CURSOR cursor=(\d+)',text)
                if not cursors or int(cursors[-1])!=BASE:raise RuntimeError('Native playback did not return to start')
                if kind=='scene'and not re.search(rf'GPU_BENCH_CLOUD arrival=\d+ points={case["points"]}\b',text):raise RuntimeError('Native point count mismatch')
            screenshot('ready.png');baseline=process_sample(process.pid);baseline_clock=time.monotonic();result['before_play']=baseline
            if args.app=='lichtblick'and not any(p['role']=='renderer'for p in baseline['processes']):raise RuntimeError('renderer process not accounted')
            send('play');begin=time.monotonic();result['play_command_ms']=(begin-baseline_clock)*1000;last_resources=begin-1;last_variants=[i*7%32 for i in range(case['cameras'])];frames=[0]*case['cameras']
            pss_sampler=PssSampler(process.pid,begin)
            while time.monotonic()-begin<args.seconds:
                tick=time.monotonic();row={'images':[]}
                if refs:
                    for i,(vector,reference)in enumerate(zip(samples(),refs)):
                        match=identify(vector,reference);now=time.monotonic()
                        valid=match['mae']<18 and match['second_mae']-match['mae']>1
                        if valid:
                            delta=(match['variant']-last_variants[i])%32
                            if delta>16:raise RuntimeError('backward/ambiguous camera progression')
                            frames[i]+=delta;last_variants[i]=match['variant']
                        row['images'].append({'camera_id':i,'valid':valid,'forward_frame':frames[i],'wall_lag_ms':(now-begin-frames[i]*.1)*1000,**match})
                now=time.monotonic();row['s']=now-begin
                if kind=='scene':
                    r=case['rects']['scene'];raw,_=capture(x11,window,[round(r[0])+inset[0],round(r[1])+inset[1],round(r[2]),round(r[3])]);row['scene_crc']=zlib.crc32(raw)
                if now-last_resources>=.25:
                    row['resources']=process_sample(process.pid,include_pss=False);last_resources=now
                    if row['resources']['summed_rss_kib']>16384*1024:raise RuntimeError('16 GiB summed RSS guard; not a product limit')
                    if cdp and kind=='raw':
                        row['ui']=cdp.evaluate(STATE)
                        stamp=re.search(r'LaserScan @ (\d+)\.(\d+) sec',row['ui']['text'])
                        if not stamp:raise RuntimeError('Raw timestamp absent')
                        row['raw_log_ns']=int(stamp[1])*1000000000+int(stamp[2].ljust(9,'0'))
                    elif not cdp:
                        text=(args.output/'app.log').read_text()
                        if kind=='raw':
                            stamps=re.findall(r'GPU_BENCH_INSPECTOR log=(\d+)',text)
                            if not stamps:raise RuntimeError('Native Raw timestamp absent')
                            row['raw_log_ns']=int(stamps[-1])
                for value in pss_sampler.drain():
                    if 'error'in value:raise RuntimeError('PSS sampler: '+value['error'])
                    result['pss_samples'].append(value)
                    if value['resources']['pss_kib']>4096*1024:raise RuntimeError('4 GiB PSS guard; not a product limit')
                row['observer_work_ms']=(time.monotonic()-tick)*1000;result['samples'].append(row)
                if time.monotonic()-started>180:raise RuntimeError('wall deadline')
                time.sleep(max(0,.05-(time.monotonic()-tick)))
            result['after_play']=process_sample(process.pid,include_pss=False)
            result['cpu_interval_seconds']=time.monotonic()-baseline_clock
            result['observed_play_seconds']=time.monotonic()-begin
            if result['observed_play_seconds']>args.seconds+1:raise RuntimeError('measurement observer exceeded timed interval')
            pss_sampler.stop_event.set()
            send('pause')
            pss_sampler.thread.join(timeout=.5)
            result['pss_samples'].extend(pss_sampler.drain())
            result['pss_samples_dropped']=pss_sampler.dropped
            result['after_pause']=process_sample(process.pid)
            if result['after_pause']['pss_kib']>4096*1024:raise RuntimeError('4 GiB paused PSS guard; not a product limit')
            screenshot('played.png')
            result['gpu_after']=subprocess.check_output(['nvidia-smi'],text=True,timeout=5)
            result['display_after']=subprocess.check_output(['xset','q'],env=env,text=True,timeout=5)
            if 'Monitor is On'not in result['display_after']:raise RuntimeError('DPMS became Off')
            rows=result['samples']
            timed_pss=[v for v in result['pss_samples']if 'resources'in v and v['completed_s']<=result['observed_play_seconds']]
            result['summary']={'cpu_one_core_percent':100*(result['after_play']['cpu_seconds_live_processes']-baseline['cpu_seconds_live_processes'])/result['cpu_interval_seconds'],
                'peak_sampled_pss_mib':max((v['resources']['pss_kib']/1024 for v in timed_pss),default=None),
                'pss_sample_count':len(timed_pss),
                'pss_read_ms':distribution([(v['completed_s']-v['started_s'])*1000 for v in timed_pss])if timed_pss else None,
                'paused_pss_mib':max(baseline['pss_kib'],result['after_pause']['pss_kib'])/1024,
                'peak_sampled_summed_rss_mib':max(r['resources']['summed_rss_kib']/1024 for r in rows if'resources'in r),'observer_work_ms':distribution([r['observer_work_ms']for r in rows]),'cameras':[]}
            for i in range(case['cameras']):
                good=[(r['s'],r['images'][i])for r in rows if r['images'][i]['valid']]
                if len(good)<len(rows)*.95:raise RuntimeError('unreliable camera identity')
                elapsed=good[-1][0]-good[0][0];gap=max(b[0]-a[0]for a,b in zip(good,good[1:]))
                if gap>=1.5:raise RuntimeError('ambiguous cyclic identity')
                result['summary']['cameras'].append({'camera_id':i,'updates_hz':sum(a[1]['variant']!=b[1]['variant']for a,b in zip(good,good[1:]))/elapsed,
                    'progress_hz':(good[-1][1]['forward_frame']-good[0][1]['forward_frame'])/elapsed,'lag_ms':distribution([r['wall_lag_ms']for t,r in good if t>=1]),
                    'rejected_samples':len(rows)-len(good),'max_sample_gap_ms':gap*1000})
            if kind=='scene':
                changes=sum(a['scene_crc']!=b['scene_crc']for a,b in zip(rows,rows[1:]))
                result['summary']['scene_changes_hz']=changes/(rows[-1]['s']-rows[0]['s'])
            if kind=='raw':
                values=[r['raw_log_ns']for r in rows if'raw_log_ns'in r]
                if len(set(values))<5 or any(b<a for a,b in zip(values,values[1:])):raise RuntimeError('Raw timestamps did not advance monotonically')
                result['summary']['raw_distinct_observed_timestamps']=len(set(values))
                result['summary']['raw_progress_hz']=(values[-1]-values[0])/1e8/(rows[-1]['s']-rows[0]['s'])
            result['status']='ok'
    except Exception as error:
        result.update(status='failed',error=repr(error))
        if x11 and window:
            try:screenshot('failed.png')
            except Exception:pass
        if cdp:
            try:result['failed_ui']=cdp.evaluate(STATE)
            except Exception:pass
    finally:
        if pss_sampler:pss_sampler.stop_event.set()
        if browser:
            try:browser.sock.settimeout(2);browser.call('Browser.close')
            except (OSError,RuntimeError):pass
        for connection in [browser,cdp]:
            if connection:
                try:connection.close()
                except OSError:pass
        for process in processes:
            if process.poll()is None:
                try:process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid,signal.SIGTERM)
                    try:process.wait(timeout=3)
                    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=3)
        if x11:x11.x.XCloseDisplay(x11.display)
        if profile:shutil.rmtree(profile,ignore_errors=True)
        result['exit_codes']=[p.poll()for p in processes];result['wall_seconds']=time.monotonic()-started
        (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps({k:result[k]for k in ['app','case','status','summary','error']if k in result}),flush=True)
    if result['status']!='ok':raise SystemExit(1)


if __name__=='__main__':main()
