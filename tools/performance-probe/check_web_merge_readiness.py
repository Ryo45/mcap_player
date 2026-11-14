#!/usr/bin/env python3
"""Real WebGPU fault checks. Uses egui pointer events and file pickers, read-only status,
and controlled fetch/clock failures. GPU COPY_SRC instrumentation is for pixel tests only.
"""
import argparse
import base64
from contextlib import ExitStack
import io
import json
from pathlib import Path
import subprocess
import time
import urllib.request
from urllib.parse import urlsplit
from PIL import Image
from benchmark_browser import browser, serve, free_port
from check_web_egui import click, wait

SNAPSHOT = 'JSON.parse(wasmBindings.egui_status())'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('dist', 'server', 'data', 'replacement', 'output'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--base-path', default='/', help='Application path under the served dist root')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    out = args.output
    report = {'status':'failed', 'checks':[]}
    with ExitStack() as stack:
        url = stack.enter_context(serve(args.dist.resolve()))
        url = url.rstrip('/')+'/'+args.base_path.strip('/')+'/' if args.base_path.strip('/') else url
        origin = urlsplit(url)._replace(path='',query='',fragment='').geturl()
        servers = []
        for label, data in [('A', args.data), ('B', args.replacement)]:
            port = free_port()
            address = f'http://127.0.0.1:{port}'
            config = out/f'{label}.toml'
            config.write_text(f'bind="127.0.0.1:{port}"\nallowed_origins=[{json.dumps(origin)}]\n'
                f'[[recordings]]\nid="sample"\ndisplay_name="Server {label}"\npath={json.dumps(str(data.resolve()))}\n')
            log = stack.enter_context((out/f'{label}.log').open('w'))
            process = subprocess.Popen([str(args.server.resolve()),'--config',str(config.resolve())], stdout=log, stderr=log)
            def stop(child=process):
                child.terminate();child.wait(timeout=15)
            stack.callback(stop)
            for _ in range(100):
                try:
                    listing=json.load(urllib.request.urlopen(address+'/v1/recordings', timeout=1));break
                except OSError:
                    if process.poll() is not None:raise RuntimeError('Server exited')
                    time.sleep(.1)
            else:raise RuntimeError('Server did not start')
            catalog=json.load(urllib.request.urlopen(address+'/v1/recordings/sample/catalog', timeout=5))
            servers.append((address,listing,catalog))
        page=None
        try:
            _,page,environment = stack.enter_context(browser('hardware',out/'chrome.log',wake_display=True,
                vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')))
            report['environment']=environment
            page.call('Page.addScriptToEvaluateOnNewDocument', {'source':r"""
                window.failures=[];const err=console.error;
                console.error=(...a)=>{failures.push(a.map(String).join(' '));err(...a)};
                addEventListener('error',e=>failures.push(e.message));
                addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));
                const realFetch=window.fetch.bind(window);window.holdNextListing=false;
                window.payloadRequests=[];
                window.fetch=(request,...rest)=>{
                    const url=typeof request==='string'?request:request.url;
                    if(/\/(messages|restore)(\?|$)/.test(url))payloadRequests.push(url);
                    if(holdNextListing && url.endsWith('/v1/recordings')){
                        holdNextListing=false;window.heldUrl=url;
                        return new Promise(resolve=>{window.releaseHeld=()=>resolve(new Response(JSON.stringify(window.heldListing),{status:200,headers:{'content-type':'application/json'}}))});
                    }
                    if(window.holdNextCatalog && url.endsWith('/catalog')){
                        window.holdNextCatalog=false;
                        return new Promise(resolve=>{window.releaseCatalog=()=>resolve(new Response(JSON.stringify(window.heldCatalog),{status:200,headers:{'content-type':'application/json'}}))});
                    }
                    return realFetch(request,...rest);
                };
                const create=GPUDevice.prototype.createTexture;
                GPUDevice.prototype.createTexture=function(desc){
                    if(desc.label==='egui browser camera'){
                        desc={...desc,usage:desc.usage|GPUTextureUsage.COPY_SRC};window.pixelDevice=this;
                    }
                    return create.call(this,desc);
                };
                const copy=GPUQueue.prototype.copyExternalImageToTexture;
                GPUQueue.prototype.copyExternalImageToTexture=function(source,dest,size){
                    const result=copy.call(this,source,dest,size);window.pixelTexture=dest.texture;return result;
                };
                window.texturePixel=async()=>{
                    const buffer=pixelDevice.createBuffer({size:256,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ});
                    const encoder=pixelDevice.createCommandEncoder();
                    encoder.copyTextureToBuffer({texture:pixelTexture},{buffer,bytesPerRow:256},{width:1,height:1});
                    pixelDevice.queue.submit([encoder.finish()]);await buffer.mapAsync(GPUMapMode.READ);
                    const pixel=Array.from(new Uint8Array(buffer.getMappedRange()).slice(0,4));buffer.unmap();buffer.destroy();return pixel;
                };
            """})
            page.call('Page.navigate',{'url':url})
            wait(page,'!!window.wasmBindings && !document.querySelector("#startup")','startup')
            state=lambda:page.evaluate(SNAPSHOT)

            def control(name):
                wait(page,f'!!{SNAPSHOT}.controls[{json.dumps(name)}]',name)
                r=state()['controls'][name]
                click(page,(r[0]+r[2])/2,(r[1]+r[3])/2);time.sleep(.12)

            def file(selector,path):
                root=page.call('DOM.getDocument')['root']['nodeId']
                node=page.call('DOM.querySelector',{'nodeId':root,'selector':selector})['nodeId']
                page.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(path.resolve())]})
                time.sleep(.2)

            def address(value):
                control('remote-url')
                for kind in ('keyDown','keyUp'):
                    page.call('Input.dispatchKeyEvent',{'type':kind,'key':'a','code':'KeyA','modifiers':2,'windowsVirtualKeyCode':65})
                page.call('Input.insertText',{'text':value});time.sleep(.2)

            def screenshot(name):
                data=base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data'])
                (out/name).write_bytes(data)
                return Image.open(io.BytesIO(data)).convert('RGB')

            time.sleep(.5)  # Let the initial eframe layout/font pass settle before pointer input.
            control('settings')
            for i,(endpoint,listing,catalog) in enumerate(servers):
                address(endpoint)
                assert state()['remote_connected_url'] is None and not state()['remote_selected']
                assert state()['remote_catalog'] is None
                control('remote-open')  # disabled, must not reuse the previous server's list
                assert not state()['source']
                control('remote-connect')
                wait(page,f'{SNAPSHOT}.remote_connected_url=={json.dumps(endpoint)}','connection identity')
                control('remote-choice');control('recording-sample')
                control('remote-catalog')
                wait(page,f'{SNAPSHOT}.remote_catalog=="sample"','catalog only')
                assert not state()['source'] and page.evaluate('payloadRequests.length')==0
            report['checks'].append('catalog-only preview does not open playback/read messages; URL edits discard the old connection/list/selection')
            control('remote-open')
            wait(page,f'{SNAPSHOT}.source=="sample" && {SNAPSHOT}.load=="Ready"','server B playback')
            assert state()['start']==servers[1][2]['timeRange']['startNs']
            old_cursor=state()['cursor'];old_revision=state()['source_revision']
            for mode in ('cancel','timeout','address-change'):
                address(servers[0][0])
                page.evaluate('window.holdNextListing=true;window.heldListing='+json.dumps(servers[0][1]))
                control('remote-connect')
                wait(page,f'{SNAPSHOT}.remote_loading && !!window.releaseHeld','held network request')
                if mode=='cancel':control('remote-cancel')
                elif mode=='address-change':address(servers[1][0])
                else:
                    control('settings')  # No further pointer/keyboard input while awaiting the deadline.
                    wait(page,f'!{SNAPSHOT}.remote_loading && {SNAPSHOT}.remote_status.includes("timed out")','idle timeout',timeout=40)
                    control('settings')
                wait(page,f'!{SNAPSHOT}.remote_loading','cancelled')
                page.evaluate('window.releaseHeld();window.releaseHeld=null');time.sleep(.3)
                assert state()['remote_connected_url'] is None
                assert state()['source_revision']==old_revision and state()['cursor']==old_cursor
                report['checks'].append(mode+' ignores a late successful response and preserves the active recording')
            address(servers[0][0]);control('remote-connect')
            wait(page,f'{SNAPSHOT}.remote_connected_url=={json.dumps(servers[0][0])}','reconnect A')
            control('remote-choice');control('recording-sample')
            page.evaluate('window.holdNextCatalog=true;window.heldCatalog='+json.dumps(servers[0][2]))
            control('remote-open')
            wait(page,f'{SNAPSHOT}.remote_loading && !!window.releaseCatalog','held source open')
            control('remote-cancel')
            file('#mcap-file',args.data)
            wait(page,f'{SNAPSHOT}.source_revision>{old_revision} && {SNAPSHOT}.load=="Ready"','Local after cancellation')
            old_cursor=state()['cursor'];old_revision=state()['source_revision']
            page.evaluate('window.releaseCatalog()');time.sleep(.5)
            assert state()['source_revision']==old_revision and state()['cursor']==old_cursor
            assert state()['source']==args.data.name
            report['checks'].append('a late cancelled Remote open cannot replace a newer Local source')
            control('settings')
            saved_layout=state()['layout'];saved_panels=state()['panel_count']
            def panel(id,kind='topics'):
                return {'kind':'panel','id':id,'panelType':kind,'configVersion':1,'config':{}}
            invalid=[{'schemaVersion':1,'root':{'kind':'split','direction':'row','children':[]}},
                     {'schemaVersion':999,'root':panel('x')},
                     {'schemaVersion':1,'root':{'kind':'split','direction':'row','children':[{'weight':-1,'node':panel('a')},{'weight':1,'node':panel('b')}]}},
                     {'schemaVersion':1,'root':{'kind':'split','direction':'row','children':[{'weight':1,'node':panel('duplicate')},{'weight':1,'node':panel('duplicate')}]}},
                     {'schemaVersion':1,'root':{'kind':'split','direction':'row','children':[{'weight':1,'node':panel(f'i{i}')} for i in range(257)]}}]
            for i,layout in enumerate(invalid):
                path=out/f'invalid-{i}.json';path.write_text(json.dumps(layout));file('#layout-file',path)
                wait(page,f'!!{SNAPSHOT}.error','layout rejected')
                assert state()['layout']==saved_layout and state()['panel_count']==saved_panels
                assert state()['cursor']==old_cursor and state()['source_revision']==old_revision
            r=state()['plot_checkbox'];click(page,(r[0]+r[2])/2,(r[1]+r[3])/2)
            wait(page,f'{SNAPSHOT}.panel_count=={saved_panels+1}','plot after malformed layout')
            report['checks'].append('invalid empty split/version/weights/duplicate IDs/panel budget reject atomically; subsequent Plot toggle stays usable')
            single=out/'single-plot.json';p=panel('only','plot');p['config']={'signal':'vehicle-speed'}
            single.write_text(json.dumps({'schemaVersion':1,'root':p}));file('#layout-file',single)
            wait(page,f'{SNAPSHOT}.panel_kinds.join(",")=="plot"','single Plot')
            r=state()['plot_checkbox'];click(page,(r[0]+r[2])/2,(r[1]+r[3])/2)
            wait(page,f'{SNAPSHOT}.panel_kinds.join(",")=="topics"','remove last plot')
            report['checks'].append('removing the last Plot leaves a supported Topics panel')
            file('#layout-file',Path('config/layouts/native_cameras.json'))
            rev=state()['source_revision'];file('#mcap-file',args.data)
            wait(page,f'{SNAPSHOT}.source_revision>{rev} && {SNAPSHOT}.load=="Ready"','closed camera layout')
            assert len(state()['camera_choices'])==4 and '/camera/left/image/compressed' not in state()['camera_choices']
            report['checks'].append('fixed-camera dropdown choices match the four planned streams, excluding unplanned catalog cameras')
            # Each alpha type is opened with its own fixed input plan. These are source changes,
            # not test-only subscription setters. Read back exactly one test-instrumented texel.
            pixels={}
            for encoding in ('rgba8','bgra8','png'):
                node=panel('alpha','camera');node['config']={'cameraTopic':'/camera/'+encoding,'showThumbnails':False,'showOverlay':False}
                layout=out/f'alpha-{encoding}.json';layout.write_text(json.dumps({'schemaVersion':1,'root':node}))
                file('#layout-file',layout)
                rev=state()['source_revision'];file('#mcap-file',Path('tests/fixtures/camera-formats/camera_alpha.mcap'))
                wait(page,f'{SNAPSHOT}.source_revision>{rev} && {SNAPSHOT}.load=="Ready"','alpha source')
                pixels[encoding]=[]
                for sequence,ratio in enumerate((.01,.05,.09,.12)):
                    before=state()['uploads'];r=state()['timeline']
                    click(page,r[0]+(r[2]-r[0]-55)*ratio,(r[1]+r[3])/2)
                    wait(page,f'{SNAPSHOT}.load=="Ready" && {SNAPSHOT}.camera_texture && {SNAPSHOT}.uploads>{before}','alpha seek')
                    current=state();actual=(int(current['cursor'])-int(current['start']))//100000000
                    assert actual==sequence,(sequence,current)
                    pixel=page.call('Runtime.evaluate',{'expression':'texturePixel()','awaitPromise':True,'returnByValue':True})['result']['value']
                    alpha=(128,0,255)[sequence%3]
                    expected=[round(c*alpha/255) for c in (20+sequence*3,80,160)]+[alpha]
                    assert all(abs(a-b)<=2 for a,b in zip(pixel,expected)),(encoding,sequence,pixel,expected)
                    image=screenshot(f'{encoding}-{sequence}.png')
                    pixels[encoding].append({'gpu_premultiplied_rgba':pixel,'screen_rgb':list(image.getpixel((960,540)))})
            for i in range(4):
                reference=pixels['rgba8'][i]['screen_rgb']
                for encoding in ('bgra8','png'):
                    assert max(abs(a-b) for a,b in zip(reference,pixels[encoding][i]['screen_rgb']))<=2
            physical=out/'physical-views.json'
            physical.write_text(json.dumps({'schemaVersion':1,'root':{'kind':'split','direction':'row','children':[
                {'weight':1,'node':panel('scene','scene-3d')},
                {'weight':1,'node':panel('bev','bev')}]}}))
            file('#layout-file',physical)
            rev=state()['source_revision'];file('#mcap-file',args.data)
            wait(page,f'{SNAPSHOT}.source_revision>{rev} && {SNAPSHOT}.load=="Ready"','physical source')
            r=state()['timeline'];click(page,r[0]+(r[2]-r[0]-55)*.3,(r[1]+r[3])/2)
            wait(page,f'{SNAPSHOT}.load=="Ready"','physical seek')
            control('settings')
            def physical_change(action):
                before=state();action()
                wait(page,f'{SNAPSHOT}.scene_layer_uploads>{before["scene_layer_uploads"]} && {SNAPSHOT}.bev_layer_uploads>{before["bev_layer_uploads"]}','physical cache invalidation')
                assert state()['cursor']==before['cursor'] and not state()['playing']
                time.sleep(.3);settled=state();time.sleep(.3)
                assert state()['scene_layer_uploads']==settled['scene_layer_uploads']
                assert state()['bev_layer_uploads']==settled['bev_layer_uploads']
                assert state()['uploads']==before['uploads'],'physical edit imported a camera image'
            physical_change(lambda:control('physical-body'))
            physical_change(lambda:control('physical-body'))
            # Apply frames is below the current Settings scroll viewport.
            page.call('Input.dispatchMouseEvent',{'type':'mouseMoved','x':300,'y':450})
            page.call('Input.dispatchMouseEvent',{'type':'mouseWheel','x':300,'y':450,'deltaX':0,'deltaY':300})
            time.sleep(.3)
            physical_change(lambda:control('physical-frames'))
            physical_change(lambda:control('physical-frames'))  # Same policy and source revisions after reset.
            physical_change(lambda:file('#platform-file',Path('config/platform_geometry/turtlebot3_waffle_pi.json')))
            physical_change(lambda:file('#calibration-file',Path('config/camera_calibration.json')))
            control('settings')
            report['checks'].append('paused physical body/platform/frame/calibration edits invalidate Scene and BEV once, preserve cursor and do not import camera images')
            page.call('Emulation.setDeviceMetricsOverride',{'width':390,'height':844,'deviceScaleFactor':1,'mobile':True})
            time.sleep(.3)
            control('settings')
            # Settings preserves its scroll position across closing and viewport changes.
            page.call('Input.dispatchMouseEvent',{'type':'mouseMoved','x':200,'y':300})
            page.call('Input.dispatchMouseEvent',{'type':'mouseWheel','x':200,'y':300,'deltaX':0,'deltaY':-2000})
            time.sleep(.3)
            wait(page,f'!!{SNAPSHOT}.controls["remote-url"]','mobile settings')
            r=state()['controls']['remote-url']
            assert 0<=r[0]<r[2]<=390 and 0<=r[1]<r[3]<=844,r
            report['checks'].append('settings remain reachable after a desktop-to-mobile viewport change')
            report['alpha']=pixels
            report['checks'].append('RGBA/BGRA/PNG premultiplied texels and rendered pixels agree across half/zero/opaque alpha and repeated Seek; no old-frame accumulation')
            report['errors']=page.evaluate('failures');assert not report['errors']
            report['status']='ok'
        finally:
            if report['status']!='ok' and page is not None:
                try:
                    report['last_state']=page.evaluate(SNAPSHOT)
                    (out/'failure.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
                except Exception as error:
                    report['capture_error']=str(error)
            (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'status':report['status'],'checks':report['checks']}))


if __name__=='__main__':
    main()
