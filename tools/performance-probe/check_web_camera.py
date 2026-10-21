#!/usr/bin/env python3
"""Functional Browser Local Camera smoke: file input, pixels, and both seek directions."""
import argparse
import base64
from functools import partial
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import socket
import threading
import time

from measure_web_camera_stress import browser


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--camera-formats', action='store_true', help='check known colors from ros-fixture camera-formats')
    parser.add_argument('--data',type=Path,required=True)
    parser.add_argument('--dist',type=Path,default=Path('apps/viewer-web/dist'))
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if args.output.exists():parser.error('new output required')
    class QuietHandler(SimpleHTTPRequestHandler):
        def log_message(self,*_args):pass
    server=ThreadingHTTPServer(('127.0.0.1',0),partial(QuietHandler,directory=str(args.dist.resolve())))
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    result={'status':'running','purpose':'functional check; SwiftShader + software Canvas2D, not hardware performance',
            'known_camera_colors':args.camera_formats,'data_sha256':hashlib.sha256(args.data.read_bytes()).hexdigest(),'script_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'seeks':[]}
    def wait(page,expression,label):
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            value=page.evaluate(expression)
            if value:return value
            time.sleep(.05)
        raise RuntimeError(label)
    try:
        with socket.socket() as available:
            available.bind(('127.0.0.1',0));port=available.getsockname()[1]
        with browser(port,extra_flags=['--disable-accelerated-2d-canvas']) as (_,page,environment):
            result['environment']=environment
            page.call('Page.addScriptToEvaluateOnNewDocument',{'source':"window.smokeErrors=[];addEventListener('error',e=>smokeErrors.push(e.message));addEventListener('unhandledrejection',e=>smokeErrors.push(String(e.reason)))"})
            page.call('Page.navigate',{'url':f'http://127.0.0.1:{server.server_port}/'})
            wait(page,"!!window.wasmBindings && !!document.querySelector('#mcap-file')",'WASM application unavailable')
            page.call('DOM.enable')
            doc=page.call('DOM.getDocument')['root']['nodeId']
            node=page.call('DOM.querySelector',{'nodeId':doc,'selector':'#mcap-file'})['nodeId']
            page.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(args.data.resolve())]})
            ready="document.querySelector('#status').textContent.startsWith('Ready') && document.querySelectorAll('.camera-card canvas').length===7"
            wait(page,ready,'recording did not become ready')
            pixels="""(async()=>{const values=[];for(const c of document.querySelectorAll('.camera-card canvas')){
              const ctx=c.getContext('2d');const pixels=ctx.getImageData(0,0,c.width,c.height).data;
              values.push({topic:c.closest('.camera-card').textContent,pixel:Array.from(pixels.slice((Math.floor(c.height/2)*c.width+Math.floor(c.width/2))*4,(Math.floor(c.height/2)*c.width+Math.floor(c.width/2))*4+4)),size:[c.width,c.height],alpha:pixels[(Math.floor(c.height/2)*c.width+Math.floor(c.width/2))*4+3],
                sha256:[...new Uint8Array(await crypto.subtle.digest('SHA-256',pixels))].map(v=>v.toString(16).padStart(2,'0')).join('')});}
              return values})()"""
            last=None
            for target in [.8,.2,.6]:
                page.evaluate(f"(()=>{{const e=document.querySelector('#timeline');e.value='{target}';e.dispatchEvent(new Event('input',{{bubbles:true}}))}})()")
                time.sleep(.1)
                wait(page,ready+f" && Math.abs(Number(document.querySelector('#timeline').value)-{target})<.005",'seek did not complete')
                # Require an actual opaque thumbnail update; setting the slider alone is insufficient.
                deadline=time.monotonic()+20
                while True:
                    evaluation=page.call('Runtime.evaluate',{'expression':pixels,'returnByValue':True,'awaitPromise':True})
                    if 'exceptionDetails' in evaluation:raise RuntimeError(evaluation['exceptionDetails'])
                    current=evaluation['result']['value']
                    colors_ok=True
                    if args.camera_formats:
                        red=20+int(target*29)*3
                        for value in current:
                            expected=[red,red,red,255] if '/camera/mono8' in value['topic'] else [red,80,160,255]
                            tolerance=3 if '/camera/jpeg' in value['topic'] else 0
                            colors_ok &= all(abs(a-b)<=tolerance for a,b in zip(value['pixel'],expected))
                    if len(current)==7 and colors_ok and all(v['alpha']==255 for v in current) and (last is None or current[0]['sha256']!=last[0]['sha256']):break
                    if time.monotonic()>deadline:
                        result['failed_pixels']=current
                        result['failed_ui']=page.evaluate('document.body.innerText')
                        result['errors']=page.evaluate('window.smokeErrors')
                        args.output.with_suffix('.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
                        raise RuntimeError('camera pixels did not change after seek')
                    time.sleep(.05)
                result['seeks'].append({'target_fraction':target,'thumbnails':current,'status':page.evaluate("document.querySelector('#status').textContent")})
                last=current
            result['errors']=page.evaluate('window.smokeErrors')
            if result['errors']:raise RuntimeError(result['errors'])
            args.output.parent.mkdir(parents=True,exist_ok=True)
            args.output.with_suffix('.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
            result['status']='ok'
    except Exception as error:
        result.update(status='failed',error=repr(error));raise
    finally:
        server.shutdown();server.server_close();thread.join(timeout=2)
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps({'status':result['status'],'seeks':len(result['seeks']),'error':result.get('error')}))


if __name__=='__main__':main()
