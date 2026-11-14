#!/usr/bin/env python3
"""Real egui Local/Remote Inspector pause/seek checks, with delayed stale HTTP responses."""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import time
import urllib.request
from benchmark_browser import browser, serve, free_port
from check_web_egui import click, wait
S='JSON.parse(wasmBindings.egui_status())'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('dist','server','data','output'):parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args();out=args.output;out.mkdir(parents=True,exist_ok=False)
    report={'status':'failed','checks':[]}
    with serve(args.dist.resolve()) as url,(out/'server.log').open('w') as log:
        port=free_port();address=f'http://127.0.0.1:{port}'
        config=out/'server.toml';config.write_text(f'bind="127.0.0.1:{port}"\nallowed_origins=[{json.dumps(url.rstrip("/"))}]\n[[recordings]]\nid="sample"\ndisplay_name="Sample"\npath={json.dumps(str(args.data.resolve()))}\n')
        server=subprocess.Popen([str(args.server.resolve()),'--config',str(config.resolve())],stdout=log,stderr=log)
        try:
            for _ in range(100):
                try:
                    catalog=json.load(urllib.request.urlopen(address+'/v1/recordings/sample/catalog',timeout=1));break
                except OSError:time.sleep(.1)
            else:raise RuntimeError('Recording server startup failed')
            scalar=next(v['id'] for v in catalog['streams'] if v['topic']=='/vehicle/speed_mps')
            with browser('hardware',out/'chrome.log',wake_display=True,vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (_,p,env):
                report['environment']=env
                p.call('Page.addScriptToEvaluateOnNewDocument',{'source':r'''
                    window.failures=[];window.held=[];window.holdInspector=false;
                    addEventListener('error',e=>failures.push(e.message));
                    addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));
                    const fetchOriginal=window.fetch.bind(window);
                    window.fetch=(request,...rest)=>{
                        const url=new URL(typeof request==='string'?request:(request.url||request.href),document.baseURI);
                        if(holdInspector && url.pathname.endsWith('/restore') && url.searchParams.get('latest_streams')===String(window.scalarStream)){
                            // Consume the body before AbortSignal cancellation, then release a detached response.
                            const ready=fetchOriginal(request,...rest).then(async r=>({body:await r.arrayBuffer(),status:r.status,headers:[...r.headers]}));
                            return new Promise(resolve=>held.push(async fail=>{
                                const r=await ready;resolve(fail?new Response('injected failure',{status:500}):new Response(r.body,{status:r.status,headers:r.headers}));
                            }));
                        }
                        return fetchOriginal(request,...rest);
                    };
                '''})
                p.call('Page.navigate',{'url':url});wait(p,'!!window.wasmBindings && !document.querySelector("#startup")','startup');time.sleep(.3)
                def state():return p.evaluate(S)
                def control(name):
                    wait(p,f'!!{S}.controls[{json.dumps(name)}]',name)
                    r=state()['controls'][name];click(p,(r[0]+r[2])/2,(r[1]+r[3])/2);time.sleep(.1)
                def type_text(text):
                    for kind in ('keyDown','keyUp'):p.call('Input.dispatchKeyEvent',{'type':kind,'key':'a','code':'KeyA','modifiers':2,'windowsVirtualKeyCode':65})
                    p.call('Input.insertText',{'text':text})
                def auto_off():
                    control('settings');p.call('Input.dispatchMouseEvent',{'type':'mouseMoved','x':200,'y':400})
                    p.call('Input.dispatchMouseEvent',{'type':'mouseWheel','x':200,'y':400,'deltaX':0,'deltaY':1500});time.sleep(.3)
                    control('inspector-auto');assert not state()['inspector_automatic'];control('settings')
                def play():
                    r=state()['play'];click(p,(r[0]+r[2])/2,(r[1]+r[3])/2)
                def seek(ratio):
                    before=state()['cursor'];r=state()['timeline'];click(p,r[0]+(r[2]-r[0]-55)*ratio,(r[1]+r[3])/2)
                    wait(p,f'{S}.cursor!={json.dumps(before)} && {S}.load=="Ready"','committed seek')
                def topic():
                    click(p,100,85);type_text('/vehicle/speed_mps');time.sleep(.2);click(p,100,106)
                    wait(p,f'{S}.has_message','initial Inspector')
                try:
                    control('settings');control('remote-url');type_text(address);control('remote-connect')
                    wait(p,f'{S}.remote_connected_url=={json.dumps(address)}','connection')
                    control('remote-choice');control('recording-sample');control('remote-open')
                    wait(p,f'{S}.source=="sample" && {S}.load=="Ready"','remote source');control('settings')
                    seek(.2);topic();p.evaluate('scalarStream='+str(scalar)+';holdInspector=true')
                    play();wait(p,'held.length>=1','held follow request')
                    cursor=int(state()['cursor']);wait(p,f'BigInt({S}.cursor)>{cursor}n+100000000n','playback independent of Inspector')
                    auto_off();play();wait(p,f'!{S}.playing && {S}.inspector_loading && held.length>=2','paused exact demand')
                    stopped=state()['cursor'];assert state()['inspector_target']==stopped and not state()['has_message']
                    p.evaluate('held[0]()');time.sleep(.3)
                    assert state()['inspector_loading'] and not state()['has_message']
                    p.evaluate('holdInspector=false;held[1]()')
                    wait(p,f'{S}.has_message && !{S}.inspector_loading','exact pause result')
                    assert state()['inspector_target']==stopped
                    report['checks'].append('Remote pause with automatic off rejects delayed follow; playback progresses during held Inspector I/O')
                    p.evaluate('held=[];holdInspector=true');seek(.7)
                    wait(p,'held.length>=1','forced seek Inspector')
                    assert state()['inspector_loading'] and not state()['has_message']
                    assert state()['inspector_target']==state()['cursor']
                    p.evaluate('holdInspector=false;held[0](true)')
                    wait(p,f'!!{S}.inspector_error && !{S}.inspector_loading','query failure')
                    assert not state()['has_message'] and not state()['playing']
                    report['checks'].append('Remote successful seek forces exact query with automatic off and reports failure without old values')
                    seek(.3);wait(p,f'{S}.has_message && !{S}.inspector_loading','seek recovery')
                    assert not state()['inspector_error'] and state()['inspector_target']==state()['cursor']
                    root=p.call('DOM.getDocument')['root']['nodeId'];node=p.call('DOM.querySelector',{'nodeId':root,'selector':'#mcap-file'})['nodeId']
                    p.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(args.data.resolve())]})
                    wait(p,f'{S}.source=={json.dumps(args.data.name)} && {S}.load=="Ready"','Local source')
                    seek(.2);topic();auto_off();seek(.6);wait(p,f'{S}.has_message && !{S}.inspector_loading','Local automatic-off seek')
                    assert state()['inspector_target']==state()['cursor'] and not state()['inspector_error']
                    report['checks'].append('Local successful seek refreshes exact Inspector with automatic off after Source replacement')
                    report['status']='ok'
                except Exception:
                    report['state']=state();(out/'failure.png').write_bytes(base64.b64decode(p.call('Page.captureScreenshot',{'format':'png'})['data']));raise
        finally:
            server.terminate();server.wait(timeout=15);(out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'status':report['status'],'checks':report['checks']}))


if __name__=='__main__':main()
