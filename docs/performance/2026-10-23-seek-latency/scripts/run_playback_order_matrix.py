import json,os,socket,subprocess
from pathlib import Path
root=Path('/home/ryo/development/mcap_player');report=root/'docs/performance/2026-10-23-seek-latency';out=Path('/tmp/mcap-seek-order-playback');out.mkdir(exist_ok=True)
cases={}
for views in [1,4]:
 base=json.loads((root/f'docs/performance/2026-10-22-camera-30hz/none-{views}-source.json').read_text())
 for phase in ['before','after']:
  case=base|{'name':f'playback-{views}-{phase}','native_binary':str(root/'target/gpu-native'/('viewer-native-seek-latency'if phase=='before'else'viewer-native-seek-order-final')),'native_patch':str(report/('native-seek-timing.patch'if phase=='before'else'final/native-seek-timing.patch'))}
  p=report/'playback'/f'camera-{views}-{phase}.json';p.parent.mkdir(exist_ok=True);p.write_text(json.dumps(case,indent=2)+'\n');cases[views,phase]=p
scene=json.loads((root/'docs/performance/2026-10-20-desktop-workloads/cloud-60000.json').read_text())
for phase in ['before','after']:
 case=scene|{'name':f'cloud-60000-{phase}','native_binary':str(root/'target/gpu-native'/('viewer-native-seek-latency'if phase=='before'else'viewer-native-seek-order-final')),'native_patch':str(report/('native-seek-timing.patch'if phase=='before'else'final/native-seek-timing.patch'))}
 p=report/'playback'/f'cloud-{phase}.json';p.write_text(json.dumps(case,indent=2)+'\n');cases['scene',phase]=p

def run(name,key,phase,calibrate=False):
 if(out/name).exists():return
 with socket.socket()as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 cmd=['python3','tools/performance-probe/measure_desktop_workload.py','--case',str(cases[key,phase]),'--app','native','--output',str(out/name),'--port',str(port),'--seconds','6'if key=='scene'else'4']
 if calibrate:cmd+=['--calibrate']
 elif key!='scene':cmd+=['--references',str(out/f'calibrate-{key}-{phase}'/'references.json')]
 print('START',name,flush=True);p=subprocess.run(cmd,cwd=root,env=os.environ|{'MCAP_BENCH_FRAME_LATENCY':'2','MCAP_SEEK_TRACE':'0'});print('END',name,p.returncode,flush=True)
 if p.returncode:raise RuntimeError(name)
for key in [1,4]:
 for phase in ['before','after']:run(f'calibrate-{key}-{phase}',key,phase,True)
for i in range(3):
 for key in [1,4,'scene']:
  for phase in (['before','after']if i%2==0 else['after','before']):run(f'{i}-{key}-{phase}',key,phase)
