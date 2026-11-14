#!/usr/bin/env python3
"""Real egui input / file-picker checks for Remote, bindings and layout-owned panels."""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import time
import urllib.request
from benchmark_browser import browser, serve, free_port
from check_web_egui import wait, click


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dist', type=Path, required=True)
    p.add_argument('--server', type=Path, required=True)
    p.add_argument('--data', type=Path, required=True)
    p.add_argument('--sample', type=Path, required=True, help='Real ROS recording with complete nested message schemas')
    p.add_argument('--bindings', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    out = args.output
    expr = 'JSON.parse(wasmBindings.egui_status())'
    report = {'status':'failed'}
    with serve(args.dist.resolve()) as url:
        port = free_port()
        config = out/'server.toml'
        config.write_text(f'bind="127.0.0.1:{port}"\nallowed_origins=[{json.dumps(url.rstrip("/"))}]\n'
            f'[[recordings]]\nid="renamed"\ndisplay_name="Renamed sample"\npath={json.dumps(str(args.data.resolve()))}\n'
            f'[[recordings]]\nid="sample"\ndisplay_name="Real sample"\npath={json.dumps(str(args.sample.resolve()))}\n')
        with (out/'server.log').open('w') as log:
            server = subprocess.Popen([str(args.server.resolve()), '--config', str(config.resolve())], stdout=log, stderr=log)
            try:
                for _ in range(100):
                    try:
                        urllib.request.urlopen(f'http://127.0.0.1:{port}/readyz', timeout=1).close()
                        break
                    except OSError:
                        if server.poll() is not None:
                            raise RuntimeError('Server exited')
                        time.sleep(.1)
                with browser('hardware', out/'chrome.log', wake_display=True,
                        vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (_, page, environment):
                    report['environment'] = environment
                    page.call('Page.addScriptToEvaluateOnNewDocument', {'source':"window.failures=[];addEventListener('error',e=>failures.push(e.message));addEventListener('unhandledrejection',e=>failures.push(String(e.reason)));"})
                    page.call('Page.navigate', {'url':url})
                    wait(page, '!!window.wasmBindings && !document.querySelector("#startup")', 'startup')
                    state = lambda:page.evaluate(expr)

                    def control(name):
                        wait(page, f'!!{expr}.controls[{json.dumps(name)}]', name)
                        r = state()['controls'][name]
                        click(page, (r[0]+r[2])/2, (r[1]+r[3])/2)
                        time.sleep(.12)

                    def file(selector, path):
                        root = page.call('DOM.getDocument')['root']['nodeId']
                        node = page.call('DOM.querySelector', {'nodeId':root, 'selector':selector})['nodeId']
                        page.call('DOM.setFileInputFiles', {'nodeId':node,'files':[str(path.resolve())]})
                        time.sleep(.25)

                    bad = out/'bad-bindings.json'
                    bad.write_text('{"pathTopic":"/foo","unknown":1}')
                    file('#bindings-file', bad)
                    wait(page, f'!!{expr}.pending_bindings_error', 'invalid bindings explicit')
                    file('#mcap-file', args.data)
                    wait(page, f'!!{expr}.error', 'invalid bindings block open')
                    assert not state()['source']
                    file('#bindings-file', args.bindings)
                    wait(page, f'!{expr}.pending_bindings_error', 'valid bindings')
                    file('#mcap-file', args.data)
                    wait(page, f'{expr}.load=="Ready" && !!{expr}.source', 'renamed local source')
                    expected = json.loads(args.bindings.read_text())
                    assert state()['bindings'] == expected, state()
                    report['local_bindings'] = state()
                    control('settings')
                    control('remote-url')
                    page.call('Input.dispatchKeyEvent', {'type':'keyDown','key':'a','code':'KeyA','modifiers':2,'windowsVirtualKeyCode':65})
                    page.call('Input.dispatchKeyEvent', {'type':'keyUp','key':'a','code':'KeyA','modifiers':2,'windowsVirtualKeyCode':65})
                    page.call('Input.insertText', {'text':f'http://127.0.0.1:{port}'})
                    control('remote-connect')
                    wait(page, f'{expr}.remote_status=="2 recording(s)"', 'remote list')
                    control('remote-choice')
                    control('recording-renamed')
                    old_source, old_cursor = state()['source'], state()['cursor']
                    control('remote-catalog')
                    wait(page, f'{expr}.remote_catalog=="renamed"', 'catalog-only preview')
                    assert state()['source']==old_source and state()['cursor']==old_cursor
                    assert 'message batch served' not in (out/'server.log').read_text(), 'catalog preview read payload messages'
                    report['catalog_only_no_message_reads'] = True
                    control('remote-open')
                    wait(page, f'{expr}.source=="renamed" && {expr}.load=="Ready"', 'remote open')
                    assert state()['bindings'] == expected
                    control('settings')
                    r = state()['play'];click(page,(r[0]+r[2])/2,(r[1]+r[3])/2)
                    wait(page, f'{expr}.camera_count>0', 'remote camera')
                    time.sleep(.4)
                    r=state()['play'];click(page,(r[0]+r[2])/2,(r[1]+r[3])/2)
                    r=state()['timeline'];click(page,r[0]+(r[2]-r[0]-55)*.6,(r[1]+r[3])/2)
                    wait(page, f'{expr}.load=="Ready" && !{expr}.playing', 'remote seek')
                    report['remote_bindings'] = state()
                    # The tiny renamed fixture deliberately omits nested ROS definitions;
                    # use a full-schema recording for successful Inspector field decoding.
                    file('#bindings-file', Path('config/workspace_bindings.json'))
                    control('settings')
                    control('remote-choice')
                    control('recording-sample')
                    control('remote-open')
                    wait(page, f'{expr}.source=="sample" && {expr}.load=="Ready"', 'full-schema remote')
                    control('settings')
                    expected = json.loads(Path('config/workspace_bindings.json').read_text())
                    r=state()['timeline'];click(page,r[0]+(r[2]-r[0]-55)*.6,(r[1]+r[3])/2)
                    wait(page, f'{expr}.load=="Ready"', 'full-schema remote seek')
                    # Multiple independent query consumers plus BEV, speed and yaw.
                    def panel(id, kind, config=None):
                        return {'kind':'panel','id':id,'panelType':kind,'configVersion':1,'config':config or {}}
                    nodes=[panel('bev','bev'),panel('speed','plot',{'signal':'vehicle-speed'}),
                           panel('yaw','plot',{'signal':'yaw-rate'}),
                           panel('odom','inspector',{'topic':expected['odometryTopic']}),
                           panel('path','inspector',{'topic':expected['pathTopic']}),
                           panel('missing','inspector',{'topic':'/does-not-exist'})]
                    layout=out/'layout.json'
                    layout.write_text(json.dumps({'schemaVersion':1,'root':{'kind':'split','direction':'row',
                        'children':[{'weight':1,'node':n} for n in nodes]}}))
                    file('#layout-file',layout)
                    wait(page, f'{expr}.panel_count==6 && {expr}.yaw_points>0 && {expr}.plot_points>0', 'remote bounded plots')
                    topics=[expected['odometryTopic'],expected['pathTopic']]
                    for topic in topics:
                        wait(page, f'{expr}.pinned_inspectors[{json.dumps(topic)}].message', topic+' Inspector')
                    wait(page, f'!!{expr}.pinned_inspectors["/does-not-exist"].error', 'missing Inspector input')
                    assert state()['plot_requests'] >= 1 and not state()['plot_error']
                    report['remote_layout'] = state()
                    (out/'remote-layout.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot', {'format':'png'})['data']))
                    assert not state()['error'], state()['error']
                    report['errors'] = page.evaluate('failures')
                    assert not report['errors'], report['errors']
                    report['status']='ok'
            finally:
                server.terminate();server.wait(timeout=15)
                (out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'status':report['status']}))


if __name__ == '__main__':
    main()
