#!/usr/bin/env python3
"""Declare display workloads before measuring. Geometry is resolved from Lichtblick canvases."""
import copy
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parents[2]
REPORT=ROOT/'docs/performance/2026-10-20-desktop-workloads'
DATA=Path('/tmp/mcap-point-cloud2-data')
CAMERA=Path('/tmp/mcap-web-60s-data/3840x2160-8cam-10hz-none-8MiB-60s.mcap')
BASELINE=json.loads((ROOT/'docs/performance/2026-10-20-desktop-default-layout/lichtblick-scan.json').read_text())
SCENE=BASELINE['configById']['3D!18i6zy7']


def panel(kind,index=0,**config):
    return {'kind':'panel','id':f'{kind}-{index}','panelType':kind,'configVersion':1,'config':config}


def native_split(direction,children,weights=None):
    weights=weights or [1/len(children)]*len(children)
    return {'kind':'split','direction':direction,'children':[{'weight':weight,'node':node}for weight,node in zip(weights,children)]}


def light_split(direction,children,weights=None):
    if len(children)==1:return children[0]
    weights=weights or [1/len(children)]*len(children)
    return {'direction':direction,'first':children[0],'second':light_split(direction,children[1:],weights[1:]),'splitPercentage':100*weights[0]/sum(weights)}


def camera_panel(index):
    return panel('camera',index,cameraTopic=f'/camera/{index}',showThumbnails=False,showOverlay=False,schedulerPriority=index==0)


def save(name,native,light,config,**facts):
    value={'name':name,'native_layout':{'schemaVersion':1,'root':native},
           'lichtblick_layout':{'configById':config,'layout':light,'globalVariables':{},'userNodes':{},'playbackConfig':{'speed':1}},
           'rects':{'layout':[320,44,1600,958]},**facts}
    (REPORT/f'{name}.json').write_text(json.dumps(value,indent=2)+'\n')


def main():
    REPORT.mkdir(exist_ok=True)
    for count in [1,4,8]:
        panels=[camera_panel(i)for i in range(count)]
        ids=[f'Image!bench{i}'for i in range(count)]
        cols=1 if count==1 else count//2
        native=native_split('column',[native_split('row',panels[i:i+cols]) for i in range(0,count,cols)]) if count>1 else panels[0]
        light=light_split('column',[light_split('row',ids[i:i+cols])for i in range(0,count,cols)]) if count>1 else ids[0]
        save(f'camera-{count}',native,light,{key:{'imageMode':{'imageTopic':f'/camera/{i}'}}for i,key in enumerate(ids)},
             kind='camera',data=str(CAMERA),cameras=count,topics=[f'/camera/{i}'for i in range(count)])
    ids=[f'Image!bench{i}'for i in range(8)]
    save('camera-focus-7',native_split('row',[camera_panel(0),native_split('column',[camera_panel(i)for i in range(1,8)])],[.8,.2]),
         light_split('row',[ids[0],light_split('column',ids[1:])],[.8,.2]),
         {key:{'imageMode':{'imageTopic':f'/camera/{i}'}}for i,key in enumerate(ids)},
         kind='camera',data=str(CAMERA),cameras=8,topics=[f'/camera/{i}'for i in range(8)])
    for expanded in [False,True]:
        save('raw-open'if expanded else'raw-closed',panel('inspector',topic='/scan'),'RawMessagesVirtual!bench',
             {'RawMessagesVirtual!bench':{'topicPath':'/scan','expansion':{'ranges':'e'}if expanded else'none'}},
             kind='raw',data=str(DATA/'scan-64-20s.mcap'),cameras=0,topics=['/scan'],points=64,expanded=expanded)
    for kind,counts in [('scan',[360,3600,36000]),('cloud',[10000,60000])]:
        for count in counts:
            config=copy.deepcopy(SCENE)
            topic='/scan'if kind=='scan'else'/points'
            config['topics']={topic:{'visible':True,'frameLocked':False,'pointSize':5,'pointShape':'circle','decayTime':0,
                                    'colorMode':'flat'if kind=='scan'else'rgb','colorField':'range'if kind=='scan'else'rgb',
                                    'flatColor':'rgba(51,209,255,0.88)','explicitAlpha':1}}
            save(f'{kind}-{count}',panel('scene-3d',optionalData=True,coloring={'mode':'solid'if kind=='scan'else'rgb','intensityRange':[0,255]}),
                 '3D!bench',{'3D!bench':config},kind='scene',data=str(DATA/f'{kind}-{count}-20s.mcap'),cameras=0,topics=[topic],points=count)
    print('Prepared 11 conditions; each will use 3 fresh runs per app (66 timed runs).')


if __name__=='__main__':main()
