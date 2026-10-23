import sys,json,time,socket,subprocess,base64
from pathlib import Path
sys.path.insert(0,'/home/ryo/development/mcap_player/tools/performance-probe')
from recorded_hardware_browser import browser
from compare_native_desktop import wait_for,digest
from PIL import Image
root=Path('/home/ryo/development/mcap_player');out=Path('/tmp/mcap-real-recording/web-hardware-2');out.mkdir()
facts=json.loads((root/'target/real-recording-check/source.json').read_text());frames=facts['images'];start=facts['start_ns'];duration=facts['end_ns']-start
points=[(round((x+.5)*180/11),round((y+.5)*320/8))for y in range(8)for x in range(11)]
refs=[]
for i in range(len(frames)):
 with Image.open(root/f'target/real-recording-check/images/{i:03d}.png')as image:refs.append([v for point in points for v in image.getpixel(point)])
ports=[]
for _ in range(2):
 with socket.socket()as s:s.bind(('127.0.0.1',0));ports.append(s.getsockname()[1])
server=subprocess.Popen(['python3','-m','http.server',str(ports[0]),'--bind','127.0.0.1','--directory',str(root/'apps/viewer-web/dist')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
result={'status':'running','seeks':[],'browser_scope':'functional smoke with headed Chrome / X11; no Canvas acceleration disable; not performance comparison','source_sha256':facts['source_sha256']}
try:
 with browser(ports[1])as(process,page,environment):
  result['pid']=process.pid;result['environment']=environment
  page.call('Page.navigate',{'url':f'http://127.0.0.1:{ports[0]}/'})
  wait_for(lambda:page.evaluate('document.querySelector("#bev-status")?.textContent.includes("ready")'),30,'WebGPU ready')
  # Observe actual composited image contents, without changing playback/decoding.
  page.evaluate('window.recordedDraws=0; const originalDraw=CanvasRenderingContext2D.prototype.drawImage; CanvasRenderingContext2D.prototype.drawImage=function(...a){const r=originalDraw.apply(this,a);if(this.canvas.id==="camera")window.recordedDraws++;return r}')
  doc=page.call('DOM.getDocument')['root']['nodeId'];node=page.call('DOM.querySelector',{'nodeId':doc,'selector':'#mcap-file'})['nodeId'];page.call('DOM.setFileInputFiles',{'nodeId':node,'files':[str(root/'target/real-recording-check/mcap-verified/mcap-verified_0.mcap')]})
  wait_for(lambda:page.evaluate('document.querySelectorAll(".camera-card").length===1'),30,'real camera detected')
  def sample():
   return page.evaluate('(()=>{const c=document.querySelector("#camera");const ctx=c.getContext("2d");return '+json.dumps(points)+'.flatMap(([x,y])=>Array.from(ctx.getImageData(Math.round(x*c.width/180),Math.round(y*c.height/320),1,1).data).slice(0,3))})()')
  import bisect
  times=[f['time_ns']for f in frames]
  for ns in [1000000000,7000000000,4000000000,9900000000,1000000000]:
   page.evaluate(f'(()=>{{const t=document.querySelector("#timeline");t.value="{ns/duration}";t.dispatchEvent(new Event("input",{{bubbles:true}}))}})()')
   index=bisect.bisect_right(times,start+ns)-1
   def matched():
    v=sample();errors=[sum(abs(a-b)for a,b in zip(v,r))/len(r)for r in refs]
    return {'expected':index,'closest':errors.index(min(errors)),'mae':errors[index]}if errors[index]<12 and errors[index]<=min(errors)+.5 else None
   try:match=wait_for(matched,10,'source pixels after seek')
   except Exception:
    v=sample();errors=[sum(abs(a-b)for a,b in zip(v,r))/len(r)for r in refs]
    result['diagnostic']={'expected':index,'closest':errors.index(min(errors)),'mae':errors[index],'min_mae':min(errors),'pixels':v,'dom':page.evaluate('document.body.innerText'),'size':page.evaluate('(()=>{const c=document.querySelector("#camera");return [c.width,c.height,window.recordedDraws]})()')}
    out.joinpath('failed.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
    raise
   state=page.evaluate('({status:document.querySelector("#status").textContent,focus:document.querySelector("#camera-focus-label").textContent,cursor:document.querySelector("#timeline").value})')
   result['seeks'].append({'offset_ns':ns,**match,**state})
  before=page.evaluate('window.recordedDraws');page.evaluate('document.querySelector("#play").click()');time.sleep(5);page.evaluate('document.querySelector("#play").click()');time.sleep(.3)
  after=page.evaluate('window.recordedDraws');assert after-before>=40,(before,after)
  result['playback']={'seconds':5,'camera_compositions':after-before,'status':page.evaluate('document.querySelector("#status").textContent')}
  assert '0 errors' in result['playback']['status']
  out.joinpath('paused.png').write_bytes(base64.b64decode(page.call('Page.captureScreenshot',{'format':'png'})['data']))
  result['status']='ok'
except Exception as e:result['status']='failed';result['error']=repr(e);raise
finally:
 server.terminate();server.wait(timeout=5)
 out.joinpath('result.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps({k:v for k,v in result.items()if k!='environment'},indent=2))
