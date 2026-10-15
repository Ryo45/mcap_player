#!/usr/bin/env python3
"""Bounded Web camera stage/memory probe; keeps production sources unchanged."""
import argparse
import hashlib
import json
from pathlib import Path
import time

from measure_web_camera_stress import browser
from measure_viewer_pilot import process_sample


def run(args):
    here=Path(__file__).parent
    with args.data.open('rb') as source:
        dataset_sha=hashlib.file_digest(source,'sha256').hexdigest()
    result={'mode':args.mode,'dataset':str(args.data),'samples':[],'status':'running',
            'dataset_sha256':dataset_sha,'dataset_bytes':args.data.stat().st_size,
            'play_timeout_seconds':args.play_timeout,
            'sources_sha256':{p:hashlib.sha256((here/p).read_bytes()).hexdigest() for p in
                              ['diagnose_web_camera.py','web_camera_pipeline.js','measure_viewer_pilot.py',
                               'measure_web_camera_stress.py','browser_cdp.py']}}
    with browser(args.port) as (process,page,environment):
        result['environment']=environment
        script='window.probeMode='+json.dumps(args.mode)+';'+(here/'web_camera_pipeline.js').read_text()
        if args.mode=='fingerprint':script+=(here/'web_camera_stress.js').read_text()
        page.call('Page.addScriptToEvaluateOnNewDocument',{'source':script})
        page.call('Page.navigate',{'url':'http://127.0.0.1:8094/index.html'})
        try:
            deadline=time.monotonic()+15
            while not page.evaluate('!!(pipelineProbe.memory && document.querySelector("#mcap-file"))'):
                if time.monotonic()>deadline:raise RuntimeError('app startup timeout')
                time.sleep(.05)
            if args.mode=='fingerprint':
                refs=json.loads((here.parent.parent/'docs/performance/2026-10-15-web-camera-stress/references.json').read_text())
                page.evaluate('cameraStress.references='+json.dumps(refs['table']))
            page.evaluate("{const s=document.createElement('style');s.textContent='.camera-panel{position:fixed;top:140px;left:330px;width:1200px;height:850px;z-index:10}.camera-frame{width:960px!important;height:540px!important}.scene-panel{display:none}';document.head.append(s);}")
            def sample():
                state=page.evaluate('pipelineProbe.state()')
                memory=process_sample(process.pid)
                state=state|{'process':memory,'heap':page.call('Runtime.getHeapUsage')}
                result['samples'].append(state)
                if memory['pss_kib']>2*1024*1024:
                    result['status']='resource-limit';raise RuntimeError('Chrome PSS exceeded 2 GiB')
                if state['errors']:raise RuntimeError(state['errors'])
                return state
            sample()
            doc=page.call('DOM.getDocument')['root']['nodeId']
            node=page.call('DOM.querySelector',{'nodeId':doc,'selector':'#mcap-file'})['nodeId']
            page.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(args.data.resolve())]})
            deadline=time.monotonic()+20
            while True:
                s=sample()
                if (s['status'] or '').startswith('Ready') and len(s['labels'])==8:break
                if time.monotonic()>deadline:raise RuntimeError('open timeout')
                time.sleep(.2)
            time.sleep(1)
            sample()
            result['play_start_ms']=page.evaluate("(()=>{pipelineProbe.phase='play';pipelineProbe.playStart=performance.now();document.querySelector('#play').click();return pipelineProbe.playStart;})()")
            deadline=time.monotonic()+args.play_timeout
            while True:
                s=sample()
                if s['cursor']>=1:break
                if time.monotonic()>deadline:raise RuntimeError('play timeout')
                time.sleep(.2)
            result['play_ms']=s['now']-result['play_start_ms']
            page.evaluate("pipelineProbe.phase='drain'")
            for _ in range(5):time.sleep(.2);sample()
            result['pre_gc']=sample()
            page.call('HeapProfiler.collectGarbage')
            time.sleep(.4)
            result['post_gc']=sample()
            result['status']='ok'
        except Exception as error:
            if result['status']=='running':result['status']='error'
            result['error']=str(error)
        finally:
            result['jobs']=page.evaluate('pipelineProbe.jobs')
            result['paints']=page.evaluate('pipelineProbe.paints')
            if args.mode=='fingerprint':result['fingerprints']=page.evaluate('cameraStress.events')
            args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'output':str(args.output),'status':result['status'],'error':result.get('error'),
                      'jobs':len(result['jobs']),'paints':len(result['paints']),
                      'peak_pss_mib':max(s['process']['pss_kib'] for s in result['samples'])/1024}),flush=True)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data',type=Path,required=True)
    parser.add_argument('--mode',choices=['bare','light','fingerprint'],required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--port',type=int,default=9241)
    parser.add_argument('--play-timeout',type=float,default=20,
                        help='maximum wall seconds for playback (use 180 for 60-second fixtures)')
    args=parser.parse_args()
    if args.output.exists():parser.error('use a new output path')
    if not 0 < args.play_timeout <= 3600:parser.error('--play-timeout must be in (0, 3600]')
    args.output.parent.mkdir(parents=True,exist_ok=True)
    run(args)
