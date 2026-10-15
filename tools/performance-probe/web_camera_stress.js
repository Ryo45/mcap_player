// Visible image content, measured at actual canvas paints. No product state is modified.
(() => {
  const draw = CanvasRenderingContext2D.prototype.drawImage;
  const tiny = document.createElement('canvas'); tiny.width=16; tiny.height=9;
  const ctx=tiny.getContext('2d',{willReadFrequently:true}); ctx.imageSmoothingEnabled=false;
  function fingerprint(canvas) {
    draw.call(ctx,canvas,0,0,16,9);
    const pixels=ctx.getImageData(0,0,16,9).data;
    let hash=2166136261;
    for(const b of pixels) hash=Math.imul(hash^b,16777619);
    return (hash>>>0).toString(16);
  }
  const stress=window.cameraStress={phase:'setup',events:[],last:{},errors:[],heartbeat:[],fingerprint};
  addEventListener('error',e=>stress.errors.push(e.message));
  addEventListener('unhandledrejection',e=>stress.errors.push(String(e.reason)));
  let previous=performance.now();
  setInterval(()=>{const now=performance.now();stress.heartbeat.push({t:now,phase:stress.phase,interval:now-previous});previous=now;},10);
  CanvasRenderingContext2D.prototype.drawImage=function(...args) {
    const result=draw.apply(this,args);
    const id=this.canvas.id;
    const cam=id==='camera'?0:/^camera-thumb-([1-7])$/.exec(id)?.[1];
    if(cam===undefined || !stress.references) return result;
    const size=this.canvas.width+'x'+this.canvas.height;
    const hash=fingerprint(this.canvas);
    const variant=stress.references[size]?.indexOf(hash) ?? -1;
    const key=Number(cam);
    const last=stress.last[key];
    if(!last || last.hash!==hash) {
      const event={t:performance.now(),phase:stress.phase,cam:key,size,hash,variant,
        sequenceModulo32:variant<0?null:(variant-key*7+224)%32,
        cursor:Number(document.querySelector('#timeline').value)*3.9,
        status:document.querySelector('#status').textContent};
      stress.events.push(event);stress.last[key]=event;
    }
    return result;
  };
  stress.state=()=>({now:performance.now(),last:stress.last,errors:stress.errors,
    cards:document.querySelectorAll('.camera-card').length,
    cursor:Number(document.querySelector('#timeline')?.value),
    status:document.querySelector('#status')?.textContent,
    performance:document.querySelector('#performance')?.textContent,
    playButtonSaysPause:document.querySelector('#play')?.textContent.includes('Pause'),
    canvases:[...document.querySelectorAll('canvas')].map(c=>({id:c.id,width:c.width,height:c.height}))});
  stress.seek=(sequence,phase)=>{
    stress.phase=phase;
    const target=(sequence/10+.04)/3.9;
    const start=performance.now();
    const timeline=document.querySelector('#timeline');timeline.value=String(target);
    timeline.dispatchEvent(new Event('input',{bubbles:true}));
    return {sequence,target,start};
  };
})();
