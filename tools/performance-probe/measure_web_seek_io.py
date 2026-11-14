#!/usr/bin/env python3
"""Compare egui tap-to-image preparation and serialized Browser File I/O.

Uses the default desktop Monitor layout, a real GPU, and optional synthetic latency
per Blob read. GPU import is not compositor presentation or actual Android latency.
"""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import time

from benchmark_browser import browser, serve
from check_web_egui import click, wait

SNAPSHOT = 'JSON.parse(wasmBindings.egui_status())'
HOOK = r'''
window.failures=[];window.events=[];window.measuring=false;window.delayMs=0;
function note(stage,extra={}){
    if(measuring && events.length<4096)events.push({stage,t:performance.now(),...extra});
}
addEventListener('error',e=>failures.push(e.message));
addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));
addEventListener('pointerdown',()=>note('tap'),true);
const read=Blob.prototype.arrayBuffer;
Blob.prototype.arrayBuffer=function(){
    const delay=measuring?delayMs:0;
    note('read-start',{bytes:this.size});
    return read.call(this).then(bytes=>{
        if(!delay){note('read-end',{bytes:bytes.byteLength});return bytes}
        return new Promise(resolve=>setTimeout(()=>{
            note('read-end',{bytes:bytes.byteLength});resolve(bytes);
        },delay));
    });
};
const copy=GPUQueue.prototype.copyExternalImageToTexture;
GPUQueue.prototype.copyExternalImageToTexture=function(source,destination,size){
    const result=copy.call(this,source,destination,size);
    // Monitor thumbnails are at most 96px wide at the fixed DPR=1 used here.
    if(destination.texture.label==='egui browser camera' && size.width>256)
        note('image-import',{width:size.width,height:size.height});
    return result;
};
const raf=requestAnimationFrame;
window.requestAnimationFrame=callback=>raf(t=>{
    callback(t);
    if(!measuring || !window.wasmBindings?.egui_status)return;
    const s=JSON.parse(wasmBindings.egui_status());
    if(s.load!==window.previousLoad){note('load',{load:s.load,cursor:s.cursor});window.previousLoad=s.load}
});
'''


def run(dist, base_path, data, output, delay_ms):
    output.mkdir(exist_ok=False)
    with serve(dist.resolve()) as url, browser('hardware', output/'chrome.log',
            wake_display=True, vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (_, page, environment):
        if base_path.strip('/'):
            url += base_path.strip('/')+'/'
        page.call('Page.addScriptToEvaluateOnNewDocument', {'source': HOOK})
        page.call('Page.navigate', {'url': url})
        wait(page, '!!window.wasmBindings && !document.querySelector("#startup")', 'startup')
        root=page.call('DOM.getDocument')['root']['nodeId']
        node=page.call('DOM.querySelector', {'nodeId':root,'selector':'#mcap-file'})['nodeId']
        page.call('DOM.setFileInputFiles', {'nodeId':node,'files':[str(data.resolve())]})
        wait(page, f'{SNAPSHOT}.source && {SNAPSHOT}.load=="Ready"', 'source open')
        results=[]
        for ratio in (.2,.7,.3,.8,.4,.65):
            time.sleep(.3)
            before=page.evaluate(SNAPSHOT)
            rect=before['timeline']
            page.evaluate('events=[];measuring=true;previousLoad=null;delayMs='+str(delay_ms))
            click(page,rect[0]+(rect[2]-rect[0]-55)*ratio,(rect[1]+rect[3])/2)
            wait(page, 'events.some(e=>e.stage==="image-import")', 'focused image', timeout=40)
            page.evaluate('measuring=false')
            events=page.evaluate('events')
            tap=next(e['t'] for e in events if e['stage']=='tap')
            image=next(e for e in events if e['stage']=='image-import')
            seeking=next(i for i,e in enumerate(events) if e['stage']=='load' and str(e['load']).startswith('Seeking'))
            restored=next(e for e in events[seeking+1:] if e['stage']=='load' and e['load']=='Ready')
            reads=[e for e in events if e['stage']=='read-start' and tap<=e['t']<=restored['t']]
            current=page.evaluate(SNAPSHOT)
            assert current['cursor']!=before['cursor'] and not current['error'], current
            results.append({'ratio':ratio,'tap_to_restore_ms':restored['t']-tap,
                'tap_to_image_import_ms':image['t']-tap,'restore_to_image_import_ms':image['t']-restored['t'],
                'restore_reads':len(reads),'restore_read_bytes':sum(e['bytes'] for e in reads),
                'image_size':[image['width'],image['height']],'events':events})
            wait(page,f'{SNAPSHOT}.load=="Ready"','settled',timeout=40)
        return {'environment':environment,'read_delay_ms':delay_ms,'seeks':results,
            'median_ms':statistics.median(s['tap_to_image_import_ms'] for s in results),
            'median_restore_reads':statistics.median(s['restore_reads'] for s in results)}


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('before','after','data','output'):
        p.add_argument('--'+name,type=Path,required=True)
    for label in ('before','after'):
        p.add_argument('--'+label+'-base-path',default='/')
    p.add_argument('--read-delay-ms',type=int,default=100)
    args=p.parse_args()
    if not 0<=args.read_delay_ms<=200:
        p.error('read-delay-ms must be 0..200')
    args.output.mkdir(parents=True,exist_ok=False)
    result={'status':'failed','scope':__doc__,'data_sha256':hashlib.file_digest(args.data.open('rb'),'sha256').hexdigest(),'runs':[]}
    try:
        for label in ('before','after'):
            measured=run(getattr(args,label),getattr(args,label+'_base_path'),args.data,args.output/label,args.read_delay_ms)
            result['runs'].append({'variant':label,**measured})
            print(label,{'median_ms':measured['median_ms'],'median_restore_reads':measured['median_restore_reads']},flush=True)
        result['status']='ok'
    finally:
        (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')


if __name__=='__main__':
    main()
