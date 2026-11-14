#!/usr/bin/env python3
"""Paused camera-switch latency after an exact Seek; controlled desktop layout/DPR 1.

Measures focused image GPU import, not compositor presentation or Android.
"""
import argparse
import base64
import json
from pathlib import Path
import statistics
import time
from benchmark_browser import browser, serve
from check_web_egui import click, wait
from measure_web_seek_io import HOOK, SNAPSHOT


def run(dist,data,out,delay):
    out.mkdir(exist_ok=False)
    panel=lambda name,kind,config:{'kind':'panel','id':name,'panelType':kind,'configVersion':1,'config':config}
    layout=out/'layout.json'
    layout.write_text(json.dumps({'schemaVersion':1,'root':{'kind':'split','direction':'row','children':[
        {'weight':1,'node':panel('camera','camera',{'showOverlay':False})},
        {'weight':1,'node':panel('scene','scene-3d',{'optionalData':True})}]}}))
    results=[]
    with serve(dist.resolve()) as url,browser('hardware',out/'chrome.log',wake_display=True,
            vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (_,p,env):
        p.call('Page.addScriptToEvaluateOnNewDocument',{'source':HOOK})
        p.call('Page.navigate',{'url':url})
        wait(p,'!!window.wasmBindings && !document.querySelector("#startup")','startup')
        def state():return p.evaluate(SNAPSHOT)
        def file(selector,path):
            root=p.call('DOM.getDocument')['root']['nodeId']
            node=p.call('DOM.querySelector',{'nodeId':root,'selector':selector})['nodeId']
            p.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(path.resolve())]})
        file('#layout-file',layout);wait(p,f'{SNAPSHOT}.layout=="layout.json"','layout')
        file('#mcap-file',data);wait(p,f'{SNAPSHOT}.source && {SNAPSHOT}.load=="Ready"','source')
        rect=state()['timeline'];click(p,rect[0]+(rect[2]-rect[0]-55)*.3,(rect[1]+rect[3])/2)
        wait(p,f'{SNAPSHOT}.load=="Ready" && {SNAPSHOT}.camera_count==7','exact images')
        time.sleep(.4)
        try:
            for index in [1,2,0,2,1,0]:
                before=state();cursor=before['cursor']
                click(p,300,32);time.sleep(.15)
                p.evaluate('events=[];measuring=true;previousLoad=null;delayMs='+str(delay))
                click(p,200,56+index*21)
                wait(p,f'{SNAPSHOT}.camera=={index} && events.some(e=>e.stage=="image-import")','camera image',timeout=40)
                wait(p,f'{SNAPSHOT}.load=="Ready" && {SNAPSHOT}.camera_count==7','settled source',timeout=40)
                settled=p.evaluate('performance.now()')
                p.evaluate('measuring=false')
                events=p.evaluate('events');tap=next(e['t'] for e in events if e['stage']=='tap')
                images=[e for e in events if e['stage']=='image-import']
                image=images[0]
                assert state()['cursor']==cursor
                reads=[e for e in events if e['stage']=='read-start' and e['t']>=tap]
                results.append({'camera':index,'latency_ms':image['t']-tap,'last_focus_import_ms':images[-1]['t']-tap,
                    'settled_ms':settled-tap,'reads':len(reads),'events':events})
                wait(p,f'{SNAPSHOT}.load=="Ready" && {SNAPSHOT}.camera_count==7','settle')
                time.sleep(.3)
        except Exception:
            (out/'failure.png').write_bytes(base64.b64decode(p.call('Page.captureScreenshot',{'format':'png'})['data']))
            (out/'failure.json').write_text(json.dumps(state(),indent=2));raise
    return {'environment':env,'delay_ms':delay,'switches':results,'median_ms':statistics.median(v['latency_ms'] for v in results),
        'median_settled_ms':statistics.median(v['settled_ms'] for v in results),'median_reads':statistics.median(v['reads'] for v in results)}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('before','after','data','output'):parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--read-delay-ms',type=int,default=0)
    args=parser.parse_args()
    if not 0<=args.read_delay_ms<=200:parser.error('read-delay-ms must be 0..200')
    args.output.mkdir(parents=True,exist_ok=False)
    report={'status':'failed','runs':[]}
    try:
        for variant in ('before','after'):
            result=run(getattr(args,variant),args.data,args.output/variant,args.read_delay_ms)
            report['runs'].append({'variant':variant,**result})
            print(variant,{k:result[k] for k in ['median_ms','median_settled_ms','median_reads']},flush=True)
        report['status']='ok'
    finally:(args.output/'result.json').write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
