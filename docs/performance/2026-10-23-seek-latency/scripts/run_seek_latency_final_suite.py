from pathlib import Path
import subprocess,os
root=Path('/home/ryo/development/mcap_player')
commands=[(['python3','/tmp/run_seek_order_final_matrix.py'],'/tmp/seek-order-final-matrix.log'),(['python3','/tmp/run_playback_order_matrix.py'],'/tmp/seek-order-playback-matrix.log'),(['python3','tools/performance-probe/check_recorded_camera.py','--source','target/real-recording-check/source.json','--data','target/real-recording-check/mcap-verified/mcap-verified_0.mcap','--binary','target/gpu-native/viewer-native-seek-order-final','--output','/tmp/mcap-seek-order-recorded'],'/tmp/seek-order-recorded.log')]
for command,logpath in commands:
 print('START',command,flush=True)
 with Path(logpath).open('w')as log: p=subprocess.run(command,cwd=root,env=os.environ|{'MCAP_SEEK_TRACE':'0'},stdout=log,stderr=log)
 print('END',p.returncode,logpath,flush=True)
 if p.returncode:raise RuntimeError(logpath)
