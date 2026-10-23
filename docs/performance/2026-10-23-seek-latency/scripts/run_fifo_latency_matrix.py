import json,os,socket,subprocess
from pathlib import Path
root=Path('/home/ryo/development/mcap_player');report=root/'docs/performance/2026-10-23-seek-latency';output=Path('/tmp/mcap-fifo-latency');output.mkdir(exist_ok=True)
case=json.loads((root/'docs/performance/2026-10-23-recorded-seek/small.json').read_text());case.update(name='fifo-latency-small',native_binary=str(root/'target/gpu-native/viewer-native-seek-latency'),native_patch=str(report/'native-seek-timing.patch'))
path=report/'small.json';path.write_text(json.dumps(case,indent=2)+'\n')
def run(name,app,calibrate=False,latency=1,trace=False):
 if (output/name).exists():return
 with socket.socket()as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 cmd=['python3','tools/performance-probe/measure_desktop_workload.py','--case',str(path),'--app',app,'--output',str(output/name),'--port',str(port)]
 if calibrate:cmd+=['--calibrate']
 else:cmd+=['--seek-only','--references',str(output/'calibrate-native/references.json')if app=='native'else'/tmp/mcap-raw-storage-results/calibrate-1-lichtblick/references.json']
 print('START',name,flush=True)
 p=subprocess.run(cmd,cwd=root,env=os.environ|{'MCAP_BENCH_FRAME_LATENCY':str(latency),'MCAP_SEEK_TRACE':'1'if trace else'0'})
 print('END',name,p.returncode,flush=True)
 if p.returncode:raise RuntimeError(name)
run('calibrate-native','native',True)
for i in range(3):
 jobs=[('fifo2','native',2,False),('fifo1','native',1,False),('lichtblick','lichtblick',1,False)]
 if i%2:jobs.reverse()
 for tag,app,latency,trace in jobs:run(f'{i}-{tag}',app,latency=latency,trace=trace)
run('diagnostic-trace','native',latency=1,trace=True)
