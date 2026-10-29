#!/usr/bin/env python3
"""Optional actual Chrome/Local/Remote smoke check; run from the repository root.
Build and generate fixtures as described in this directory's README first.
Uses existing performance-probe Chrome/CDP helpers and the existing Recording Server.
File API fault injection is confined to this disposable browser, never product code.
"""
import sys,json,time,subprocess,base64
from pathlib import Path
sys.path.insert(0,str(Path('tools/performance-probe').resolve()))
from benchmark_browser import browser,serve,free_port
from compare_native_desktop import wait_for
root=Path.cwd();out=root/'target/workspace-bindings';result={'status':'running','checks':[]}
(out/'invalid.json').write_text('{"unknown":true}')
(out/'empty.json').write_text((out/'bindings.json').read_text().replace('/robot/scan',''))
try:
 with serve(root/'apps/viewer-web/dist') as url:
  port=free_port();(out/'server.toml').write_text(f'bind="127.0.0.1:{port}"\nallowed_origins=[{json.dumps(url.rstrip("/"))}]\nmax_in_flight_requests=4\n[[recordings]]\nid="custom"\ndisplay_name="Custom bindings"\npath={json.dumps(str(out/"recording.mcap"))}\n')
  with (out/'server.log').open('w') as log:
   server=subprocess.Popen([str(out/'recording-server'),'--config',str(out/'server.toml')],stdout=log,stderr=log)
   try:
    with browser('hardware',out/'chrome.log',wake_display=True,vulkan_icd=Path('/usr/share/vulkan/icd.d/nvidia_icd.json')) as (_,page,environment):
     result['environment']=environment
     def nav():
      page.call('Page.navigate',{'url':url});wait_for(lambda:page.evaluate('document.querySelector("#workspace-bindings-status")?.textContent.includes("valid")'),20,'settings ready')
     def choose(selector,path):
      dom=page.call('DOM.getDocument');node=page.call('DOM.querySelector',{'nodeId':dom['root']['nodeId'],'selector':selector})['nodeId'];page.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(path)]})
     def status():return page.evaluate('document.querySelector("#status").textContent')
     def settings():return page.evaluate('document.querySelector("#workspace-bindings-status").textContent')
     def configure():
      choose('#workspace-bindings-file',out/'bindings.json');wait_for(lambda:'bindings.json: valid' in settings(),10,'custom settings valid');assert page.evaluate('document.querySelector("#point-cloud-topic").value')=='';assert '/robot/scan' in settings()
     def ready_scene():return page.evaluate('(()=>{const s=document.querySelector("#status").textContent;return /path [1-9][0-9]* pts/.test(s)&&/scan [1-9][0-9]* pts/.test(s)&&s.includes("0 errors")&&!s.includes("Seeking")})()')
     def seek(fraction):
      page.evaluate(f'(()=>{{let t=document.querySelector("#timeline");t.value="{fraction}";t.dispatchEvent(new Event("input",{{bubbles:true}}));}})()');time.sleep(.2);wait_for(ready_scene,15,'custom Path/cloud restored');time.sleep(.3)
      assert 'TF missing' not in page.evaluate('document.querySelector("#scene-info").textContent')
      assert 'waiting' not in page.evaluate('document.querySelector("#telemetry").textContent').lower()
     nav();assert not page.evaluate('document.querySelector("#mcap-file").disabled')
     for invalid in ['invalid.json','empty.json']:
      choose('#workspace-bindings-file',out/invalid);wait_for(lambda:page.evaluate('document.querySelector("#workspace-bindings-status").className==="error"'),10,'invalid settings visible');assert page.evaluate('document.querySelector("#mcap-file").disabled&&document.querySelector("#remote-open-playback").disabled');result['checks'].append({'invalid':invalid,'error':settings()})
     page.evaluate('document.querySelector("#workspace-bindings-reset").click()');assert not page.evaluate('document.querySelector("#mcap-file").disabled')
     page.evaluate('window.originalArrayBuffer=File.prototype.arrayBuffer;File.prototype.arrayBuffer=function(){return Promise.reject(new Error("injected read failure"))}')
     choose('#workspace-bindings-file',out/'bindings.json');wait_for(lambda:'injected read failure' in settings(),10,'file read error');assert page.evaluate('document.querySelector("#mcap-file").disabled');result['checks'].append({'read_failure_blocked':settings()})
     page.evaluate('File.prototype.arrayBuffer=function(){const file=this;return new Promise((resolve,reject)=>{window.releaseBindings=()=>window.originalArrayBuffer.call(file).then(resolve,reject)})};document.querySelector("#workspace-bindings-file").value=""')
     choose('#workspace-bindings-file',out/'bindings.json');wait_for(lambda:page.evaluate('typeof window.releaseBindings==="function"'),10,'pending settings read');assert page.evaluate('document.querySelector("#mcap-file").disabled&&document.querySelector("#remote-open-playback").disabled')
     page.evaluate('document.querySelector("#workspace-bindings-reset").click();window.releaseBindings();File.prototype.arrayBuffer=window.originalArrayBuffer');time.sleep(.3);assert settings().startswith('Bundled default: valid');result['checks'].append({'stale_read_after_explicit_reset':settings()});configure()
     page.evaluate('(()=>{let p=document.querySelector("#point-cloud-topic");p.value="/manual";p.dispatchEvent(new Event("input",{bubbles:true}));})()');assert '/manual' in settings();page.evaluate('(()=>{let p=document.querySelector("#point-cloud-topic");p.value="";p.dispatchEvent(new Event("input",{bubbles:true}));})()');assert '/robot/scan' in settings()
     choose('#mcap-file',out/'recording.mcap');wait_for(lambda:page.evaluate('!document.querySelector("#timeline").disabled'),30,'local opened');result['local_initial']=status();seek(.65)
     page.evaluate('(()=>{const s=document.querySelector("#camera-topic");s.selectedIndex=s.options.length-1;s.dispatchEvent(new Event("change",{bubbles:true}));})()');seek(.35);result['checks'].append({'local':status(),'scene':page.evaluate('document.querySelector("#scene-info").textContent'),'odometry':page.evaluate('document.querySelector("#telemetry").textContent')})
     # Settings changes are for the next source; seek/current camera retain old bindings.
     page.evaluate('document.querySelector("#workspace-bindings-reset").click()');seek(.8);result['checks'].append({'current_session_kept_bindings_after_reset':status()})
     (out/'web-local.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
     nav();configure();page.evaluate(f'document.querySelector("#remote-server-url").value="http://127.0.0.1:{port}";document.querySelector("#remote-connect").click()');wait_for(lambda:page.evaluate('document.querySelector("#remote-recording").options.length>0'),15,'remote list');page.evaluate('document.querySelector("#remote-load-catalog").click()');wait_for(lambda:page.evaluate('document.querySelector("#remote-output").textContent.includes("/robot/scan")'),15,'remote catalog')
     page.evaluate('document.querySelector("#remote-open-playback").click()');seek(.7);result['checks'].append({'remote':status(),'scene':page.evaluate('document.querySelector("#scene-info").textContent'),'odometry':page.evaluate('document.querySelector("#telemetry").textContent')})
     page.evaluate('(()=>{const s=document.querySelector("#camera-topic");s.selectedIndex=s.options.length-1;s.dispatchEvent(new Event("change",{bubbles:true}));})()');seek(.3);result['checks'].append({'remote_camera_seek':status()})
     (out/'web-remote.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
     result['status']='ok'
   finally:server.terminate();server.wait(timeout=5)
except Exception as e:result['status']='failed';result['error']=repr(e);raise
finally:(out/'web-result.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps({k:v for k,v in result.items()if k!='environment'},indent=2))
