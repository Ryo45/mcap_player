#!/usr/bin/env python3
"""Sequential cache-allowed Desktop pilot; fresh application process/profile per job."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[2]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--small',type=Path,required=True)
    parser.add_argument('--large',type=Path,required=True)
    parser.add_argument('--jpeg-dir',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=False)
    jobs=[]
    for repeat in range(3):
        for case,data,seeks in [('1cam-20s',args.small,'310000000,1710000000,4110000000'),
                                ('8cam-60s-one-selected',args.large,'25310000000,48710000000,16110000000')]:
            for app in (['native','lichtblick'] if repeat%2==0 else ['lichtblick','native']):
                jobs.append(dict(case=case,app=app,repeat=repeat,data=str(data.resolve()),seeks=seeks))
    manifest={'jobs':jobs,'started_unix':time.time(),'status':'running'}
    path=args.output/'manifest.json';path.write_text(json.dumps(manifest,indent=2)+'\n')
    for index,job in enumerate(jobs):
        name=f"{job['case']}-{job['app']}-{job['repeat']}";print('START',name,flush=True)
        command=[sys.executable,str(ROOT/'tools/performance-probe/compare_native_desktop.py'),
                 '--app',job['app'],'--data',job['data'],'--jpeg-dir',str(args.jpeg_dir.resolve()),
                 '--output',str((args.output/name).resolve()),'--port',str(9260+index),
                 '--seconds','12','--seek-offsets',job['seeks'],'--wake-display']
        run=subprocess.run(command,cwd=ROOT,timeout=160,check=False)
        job.update(exit_code=run.returncode,output=name)
        path.write_text(json.dumps(manifest,indent=2)+'\n')
        if run.returncode:
            manifest['status']='failed';path.write_text(json.dumps(manifest,indent=2)+'\n')
            raise SystemExit(run.returncode)
    manifest.update(status='ok',finished_unix=time.time())
    path.write_text(json.dumps(manifest,indent=2)+'\n')

if __name__=='__main__':main()
