import json,os,socket,subprocess
from pathlib import Path
root=Path('/home/ryo/development/mcap_player')
report=root/'docs/performance/2026-10-23-recorded-seek'
output=Path('/tmp/mcap-seek-timing');output.mkdir(exist_ok=True)
case=json.loads((root/'docs/performance/2026-10-22-seek-revisit/scrub-after.json').read_text())
case.update(name='seek-timing-small',native_binary=str(root/'target/gpu-native/viewer-native-seek-timing'),native_patch=str(report/'native-seek-timing.patch'),camera_policy='Focused camera follows input; pause/seek bypasses cadence; single displayed camera')
path=report/'small.json';path.write_text(json.dumps(case,indent=2)+'\n')
def run(name,app,calibrate=False,trace=False):
 if (output/name).exists():return
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 cmd=['python3','tools/performance-probe/measure_desktop_workload.py','--case',str(path),'--app',app,'--output',str(output/name),'--port',str(port)]
 if calibrate:cmd+=['--calibrate']
 else:
  refs=output/'calibrate-native/references.json' if app=='native' else Path('/tmp/mcap-raw-storage-results/calibrate-1-lichtblick/references.json')
  cmd+=['--seek-only','--references',str(refs)]
 print('START',name,flush=True)
 p=subprocess.run(cmd,cwd=root,env=os.environ|{'MCAP_SEEK_TRACE':'1'if trace else'0'})
 print('END',name,p.returncode,flush=True)
 if p.returncode:raise RuntimeError(name)
run('calibrate-native','native',True)
for i in range(3):
 jobs=[('control','native',False),('trace','native',True),('lichtblick','lichtblick',False)]
 if i%2:jobs.reverse()
 for tag,app,trace in jobs:run(f'{i}-{tag}',app,trace=trace)
