// Measurement-only hooks. No image payloads or bitmaps are retained by this probe.
(() => {
  const probe=window.pipelineProbe={jobs:[],paints:[],errors:[],phase:'setup'};
  if(window.probeMode==='focus') {
    probe.focusedCamera=0;
    document.addEventListener('click',event=>{
      const card=event.target.closest?.('button[data-camera]');
      if(card)probe.focusedCamera=Number(card.dataset.camera);
    },true);
  }
  addEventListener('TrunkApplicationStarted',e=>{probe.memory=e.detail.wasm.memory;});
  addEventListener('error',e=>probe.errors.push(e.message));
  addEventListener('unhandledrejection',e=>probe.errors.push(String(e.reason)));
  probe.state=()=>({now:performance.now(),phase:probe.phase,wasm_bytes:probe.memory?.buffer.byteLength,
    cursor:Number(document.querySelector('#timeline')?.value),status:document.querySelector('#status')?.textContent,
    metrics:document.querySelector('#performance')?.textContent,
    labels:[...document.querySelectorAll('[id^="camera-label-"]')].map(e=>e.textContent),
    jobs:probe.jobs.length,paints:probe.paints.length,errors:probe.errors});
  if(window.probeMode==='bare')return;
  const NativeWorker=window.Worker, bitmaps=new WeakMap(), canvases=new WeakMap();
  window.Worker=class extends NativeWorker {
    constructor(url,options){
      const original=new URL(url,document.baseURI).href;
      if(!original.includes('camera_decode_worker.js')){super(url,options);return;}
      const shim=`const clock=()=>performance.timeOrigin+performance.now();
        const decode=self.createImageBitmap.bind(self),send=self.postMessage.bind(self);
        let timing;
        self.createImageBitmap=async(...args)=>{timing={start:clock()};try{return await decode(...args);}finally{timing.end=clock();}};
        self.postMessage=(data,...rest)=>{data.__probeTiming=timing;return send(data,...rest);};
        importScripts(${JSON.stringify(original)});`;
      const shimUrl=URL.createObjectURL(new Blob([shim],{type:'application/javascript'}));
      super(shimUrl,options);URL.revokeObjectURL(shimUrl);
      let job;
      const post=this.postMessage.bind(this);
      this.postMessage=(data,...rest)=>{
        job={id:probe.jobs.length,phase:probe.phase,post:performance.timeOrigin+performance.now(),
          bytes:data.bytes.byteLength,width:data.width,height:data.height};
        if(window.probeMode==='focus') {
          const bytes=new Uint8Array(data.bytes);let hash=2166136261;
          for(let i=0;i<128;i++)hash=Math.imul(hash^bytes[Math.floor(i*(bytes.length-1)/127)],16777619);
          job.signature=bytes.length+':'+(hash>>>0).toString(16);
          job.cursor=Number(document.querySelector('#timeline').value);
        }
        probe.jobs.push(job);return post(data,...rest);
      };
      this.addEventListener('message',e=>{
        if(!job)return;
        job.received=performance.timeOrigin+performance.now();job.decode=e.data.__probeTiming;
        if(e.data.bitmap)bitmaps.set(e.data.bitmap,job);
        else job.error=e.data.error;
        job=undefined;
      });
    }
  };
  const draw=CanvasRenderingContext2D.prototype.drawImage;
  const latest=new Map();
  CanvasRenderingContext2D.prototype.drawImage=function(...args){
    const job=bitmaps.get(args[0])||canvases.get(args[0]);
    const before=performance.timeOrigin+performance.now();
    const result=draw.apply(this,args);
    if(job){
      canvases.set(this.canvas,job);
      if(bitmaps.has(args[0])){job.upload=before;job.draw_ms=performance.timeOrigin+performance.now()-before;}
      const id=this.canvas.id;
      const focus=probe.focusedCamera ?? 0;
      const camera=window.probeMode==='focus'
        ? (id==='camera'?focus:/^camera-thumb-(\d+)$/.exec(id)?.[1])
        : (id==='camera'?0:/^camera-thumb-([1-7])$/.exec(id)?.[1]);
      if(camera!==undefined && latest.get(Number(camera))!==job.id){
        latest.set(Number(camera),job.id);
        probe.paints.push({phase:probe.phase,camera:Number(camera),job:job.id,t:before,
          ...(window.probeMode==='focus'?{focus,cursor:Number(document.querySelector('#timeline').value)}:{})});
      }
    }
    return result;
  };
})();
