import json,subprocess,socket
from pathlib import Path
root=Path('/home/ryo/development/mcap_player')
out=root/'target/fastwrite-desktop'
manifest=[]
def run(stem,app,name,calibrate=False):
    folder=out/name
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    command=['python3','tools/performance-probe/measure_desktop_workload.py','--case',str(out/f'{stem}.json'),'--app',app,'--output',str(folder),'--seconds','3','--port',str(port)]
    command+=['--calibrate'] if calibrate else ['--references',str(out/f'calibrate-{stem}-{app}/references.json')]
    print('START '+name,flush=True)
    result=subprocess.run(command,cwd=root)
    manifest.append({'name':name,'command':command,'returncode':result.returncode,'calibration':calibrate})
    (out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    if calibrate and result.returncode:raise RuntimeError('calibration failed')
run('indexed','lichtblick','calibrate-indexed-lichtblick',True)
run('indexed','native','calibrate-indexed-native',True)
cases=[('fastwrite','lichtblick'),('indexed','native'),('indexed','lichtblick')]
for repeat in range(3):
    for stem,app in cases if repeat%2==0 else reversed(cases):
        run(stem,app,f'{repeat}-{stem}-{app}')
print('FINISHED',flush=True)
