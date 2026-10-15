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
def browser(port):
    with socket.socket() as check:
        check.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        check.bind(('127.0.0.1', port))
    with tempfile.TemporaryDirectory(prefix='mcap-camera-stress-') as profile, tempfile.TemporaryFile() as log:
        command=['google-chrome','--headless','--no-sandbox','--enable-unsafe-webgpu',
                 '--enable-unsafe-swiftshader','--use-gl=angle','--use-angle=swiftshader',
                 f'--remote-debugging-port={port}',f'--user-data-dir={profile}','about:blank']
        process=subprocess.Popen(command,stdout=log,stderr=log)
        host=page=None
        try:
            for _ in range(200):
                try:
                    version=json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/version',timeout=1))
                    pages=json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list',timeout=1))
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
            if page: page.close()
            if host:
                try: host.call('Browser.close')
                except (RuntimeError,OSError): pass
                host.close()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate();process.wait(timeout=10)


def references(args):
    with browser(args.port) as (_,page,environment):
        page.call('Page.navigate',{'url':'http://127.0.0.1:8094/samples/manifest.json'})
        time.sleep(.5)
        expression='''(async()=>{
          const manifest=await(await fetch('/samples/manifest.json')).json();
          const table={'960x540':[],'112x63':[]};
          for(const sample of manifest){
            const bytes=await(await fetch('/samples/'+sample.file)).arrayBuffer();
            const sha=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(b=>b.toString(16).padStart(2,'0')).join('');
            if(sha!==sample.sha256) throw Error('JPEG sample SHA mismatch');
            for(const [w,h] of [[960,540],[112,63]]){
              const bitmap=await createImageBitmap(new Blob([bytes],{type:'image/jpeg'}),{
                resizeWidth:w,resizeHeight:h,resizeQuality:'low',imageOrientation:'none',
                colorSpaceConversion:'none',premultiplyAlpha:'none'});
              const full=document.createElement('canvas');full.width=w;full.height=h;
              full.getContext('2d').drawImage(bitmap,0,0);bitmap.close();
              const tiny=document.createElement('canvas');tiny.width=16;tiny.height=9;
              const ctx=tiny.getContext('2d');ctx.imageSmoothingEnabled=false;ctx.drawImage(full,0,0,16,9);
              let hash=2166136261;for(const b of ctx.getImageData(0,0,16,9).data)hash=Math.imul(hash^b,16777619);
              table[w+'x'+h].push((hash>>>0).toString(16));
            }
          }
          if(manifest.length!==32 || Object.values(table).some(h=>new Set(h).size!==32))throw Error('ambiguous image reference');
          return {table,manifest};
        })()'''
        response=page.call('Runtime.evaluate',{'expression':expression,'returnByValue':True,'awaitPromise':True,'timeout':60000})
        if 'exceptionDetails' in response: raise RuntimeError(response['exceptionDetails'])
        result=response['result']['value']|{'environment':environment}
        args.output.write_text(json.dumps(result,indent=2)+'\n')
        print('32 distinct image identities prepared for focus and thumbnails',flush=True)


def run(args):
    with args.data.open('rb') as file: sha=hashlib.file_digest(file,'sha256').hexdigest()
    refs=json.loads(args.references.read_text())
    script=Path(__file__).with_name('web_camera_stress.js').read_text()
    result={'dataset':str(args.data.resolve()),'dataset_sha256':sha,'dataset_bytes':args.data.stat().st_size,
            'references_sha256':hashlib.sha256(args.references.read_bytes()).hexdigest(),
            'git_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            'sources_sha256':{name:hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest() for name in
                              ['measure_web_camera_stress.py','web_camera_stress.js','browser_cdp.py','measure_viewer_pilot.py']},
            'viewport':[1920,1080],'focus':[960,540],'thumbnail':[112,63],
            'cache':'cache-allowed; dataset SHA256 read before Chrome startup',
            'pss_limit_kib':2*1024*1024,'memory':[],'seeks':[],'status':'running'}
    with browser(args.port) as (process,page,environment):
        result['environment']=environment
        start=time.monotonic();last_memory=0
        def observe():
            nonlocal last_memory
            state=page.evaluate('cameraStress.state()')
            now=time.monotonic()
            if now-last_memory>=.2:
                memory=process_sample(process.pid)
                result['memory'].append({'wall_seconds':now-start,'phase':page.evaluate('cameraStress.phase'),**memory})
                last_memory=now
                if memory['pss_kib']>result['pss_limit_kib']:
                    result['status']='resource-limit';raise RuntimeError('Chrome PSS exceeded 2 GiB')
            if state['errors']: raise RuntimeError(state['errors'])
            return state
        def wait(predicate,timeout=8):
            deadline=time.monotonic()+timeout
            while True:
                state=observe()
                if predicate(state): return state
                if time.monotonic()>deadline: raise RuntimeError(f'condition timed out: {state}')
                time.sleep(.02)
        def matched(state,sequence):
            return (state['status'].startswith('Ready') and len(state['last'])==8 and
                    all(v['sequenceModulo32']==sequence for v in state['last'].values()))
        def seek(sequence,phase):
            request=page.evaluate(f'cameraStress.seek({sequence},{json.dumps(phase)})')
            state=wait(lambda s:matched(s,sequence) and abs(s['cursor']-request['target'])<.001)
            request['all_images_ms']=state['now']-request['start'];request['completed']=state
            result['seeks'].append(request)
            # Late old-generation completions must not replace the restored images.
            until=time.monotonic()+.5
            while time.monotonic()<until:
                if not matched(observe(),sequence): raise RuntimeError('image changed after settled seek')
                time.sleep(.02)
        try:
            page.call('Page.addScriptToEvaluateOnNewDocument',{'source':script})
            page.call('Page.navigate',{'url':'http://127.0.0.1:8094/index.html'})
            for _ in range(200):
                if page.evaluate('!!document.querySelector("#mcap-file")'): break
                time.sleep(.05)
            else: raise RuntimeError('Web app did not load')
            page.evaluate('cameraStress.references='+json.dumps(refs['table']))
            page.evaluate('''{const style=document.createElement('style');style.textContent=
              '.camera-panel{position:fixed;top:140px;left:330px;width:1200px;height:850px;z-index:10}' +
              '.camera-frame{width:960px!important;height:540px!important}.scene-panel{display:none}';document.head.append(style);}''')
            result['before_open']=process_sample(process.pid)
            doc=page.call('DOM.getDocument')['root']['nodeId']
            node=page.call('DOM.querySelector',{'nodeId':doc,'selector':'#mcap-file'})['nodeId']
            page.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(args.data.resolve())]})
            initial=wait(lambda s:s['cards']==8 and matched(s,0),timeout=15)
            result['initial']=initial
            dimensions=[(c['width'],c['height']) for c in initial['canvases'] if c['id']=='camera' or c['id'].startswith('camera-thumb-')]
            if dimensions!=[(960,540)]+[(112,63)]*8: raise RuntimeError(f'wrong camera dimensions: {dimensions}')
            args.output.with_suffix('.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
            page.evaluate("cameraStress.phase='play';cameraStress.playStart=performance.now();document.querySelector('#play').click()")
            ended=wait(lambda s:s['cursor']>=1,timeout=15)
            # The clock stops at EOF; the button label is stale, so do not toggle it.
            result['play_end']=ended
            result['play_ms']=ended['now']-page.evaluate('cameraStress.playStart')
            for sequence in [28,10,26,12]: seek(sequence,'seek-'+str(sequence))
            page.evaluate("cameraStress.phase='burst';document.querySelector('#play').click()")
            result['burst_requests']=[]
            for sequence in [28,10,26,12,24,14,22,16]:
                expression=f"cameraStress.seek({sequence},'burst')"
                if sequence==16:
                    # Pause synchronously with the final seek: no intervening animation tick.
                    expression="(()=>{const request="+expression+";document.querySelector('#play').click();return request;})()"
                result['burst_requests'].append(page.evaluate(expression))
                observe();time.sleep(.05)
            request=result['burst_requests'][-1]
            state=wait(lambda s:matched(s,16) and abs(s['cursor']-request['target'])<.001)
            result['burst_recovery_ms']=state['now']-request['start']
            until=time.monotonic()+1
            while time.monotonic()<until:
                if not matched(observe(),16): raise RuntimeError('stale image after seek burst')
                time.sleep(.02)
            result['final']=observe();result['status']='ok'
        except Exception as error:
            if result['status']=='running': result['status']='error'
            result['error']=str(error)
        finally:
            try:
                result['paint_events']=page.evaluate('cameraStress.events')
                result['heartbeat']=page.evaluate('cameraStress.heartbeat')
                result['errors']=page.evaluate('cameraStress.errors')
            except (RuntimeError,OSError): pass
            result['peak_sampled_pss_kib']=max((m['pss_kib'] for m in result['memory']),default=0)
            args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:result.get(k) for k in ['status','error','play_ms','burst_recovery_ms','peak_sampled_pss_kib']}),flush=True)
    if result['status']!='ok': raise SystemExit(1)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data',type=Path)
    parser.add_argument('--references',type=Path)
    parser.add_argument('--prepare-references',action='store_true')
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--port',type=int,default=9238)
    args=parser.parse_args()
    if args.output.exists(): parser.error('use a new output path')
    if not args.prepare_references and (args.data is None or args.references is None):parser.error('--data and --references required')
    args.output.parent.mkdir(parents=True,exist_ok=True)
    (references if args.prepare_references else run)(args)


if __name__=='__main__':main()
