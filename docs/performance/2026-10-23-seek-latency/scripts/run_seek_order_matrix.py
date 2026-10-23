import json,os,socket,subprocess
from pathlib import Path
root=Path('/home/ryo/development/mcap_player');report=root/'docs/performance/2026-10-23-seek-latency';out=Path('/tmp/mcap-seek-order');out.mkdir(exist_ok=True)
base=json.loads((report/'small.json').read_text())
before=base|{'name':'seek-order-before'}
after=base|{'name':'seek-order-after','native_binary':str(root/'target/gpu-native/viewer-native-seek-order'),'native_patch':str(report/'order/native-seek-timing.patch')}
for name,case in [('before',before),('after',after)]: (report/'order'/f'{name}.json').write_text(json.dumps(case,indent=2)+'\n')
def run(name,app,phase='after',calibrate=False,trace=False):
 if (out/name).exists():return
 with socket.socket()as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 cmd=['python3','tools/performance-probe/measure_desktop_workload.py','--case',str(report/'order'/f'{phase}.json'),'--app',app,'--output',str(out/name),'--port',str(port)]
 if calibrate:cmd+=['--calibrate']
 else:
  refs=out/'calibrate-native/references.json'if phase=='after'else Path('/tmp/mcap-fifo-latency/calibrate-native/references.json')
  if app=='lichtblick':refs=Path('/tmp/mcap-raw-storage-results/calibrate-1-lichtblick/references.json')
  cmd+=['--seek-only','--references',str(refs)]
 print('START',name,flush=True);p=subprocess.run(cmd,cwd=root,env=os.environ|{'MCAP_BENCH_FRAME_LATENCY':'2','MCAP_SEEK_TRACE':'1'if trace else'0'});print('END',name,p.returncode,flush=True)
 if p.returncode:raise RuntimeError(name)
run('calibrate-native','native',calibrate=True)
for i in range(3):
 jobs=[('before','native'),('after','native'),('lichtblick','lichtblick')]
 if i%2:jobs.reverse()
 for tag,app in jobs:run(f'{i}-{tag}',app,'before'if tag=='before'else'after')
run('diagnostic-trace','native',trace=True)
