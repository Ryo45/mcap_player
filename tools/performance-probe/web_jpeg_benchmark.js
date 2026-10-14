(async () => {
  const bindings = window.wasmBindings;
  const modules = await Promise.all([...performance.getEntriesByType('resource')]
    .filter(e => /camera_bitmap\.js$/.test(e.name)).map(e => import(e.name)));
  if (modules.length !== 1) throw new Error('camera bitmap module not found');
  const {decodeCameraBitmap} = modules[0];
  const manifest = await (await fetch('samples/manifest.json')).json();
  const samples = [];
  for (const entry of manifest) {
    const bytes = new Uint8Array(await (await fetch(`samples/${entry.file}`)).arrayBuffer());
    const hash = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), b => b.toString(16).padStart(2,'0')).join('');
    if (hash !== entry.sha256) throw new Error('JPEG fixture SHA mismatch');
    samples.push(bytes);
  }
  await bindings.check_camera_bitmap_queue(samples[0]);
  let rejected = false;
  try { await decodeCameraBitmap(new Uint8Array([0,1,2]), 32, 24); } catch (_) { rejected=true; }
  if (!rejected) throw new Error('corrupt JPEG unexpectedly decoded');
  const valid = await decodeCameraBitmap(samples[0], 160, 90);
  if (valid.width !== 160 || valid.height !== 90) throw new Error('resize failed');
  valid.close();
  // EXIF orientation must not rotate ROS optical pixels behind TF/calibration.
  const exif = new Uint8Array([0xff,0xe1,0,34,69,120,105,102,0,0,73,73,42,0,8,0,0,0,1,0,18,1,3,0,1,0,0,0,6,0,0,0,0,0,0,0]);
  const oriented = new Uint8Array(samples[0].length + exif.length);
  oriented.set(samples[0].subarray(0,2)); oriented.set(exif,2); oriented.set(samples[0].subarray(2),2+exif.length);
  const pixels = async bytes => {
    const image = await decodeCameraBitmap(bytes,160,90);
    try { const c=document.createElement('canvas');c.width=160;c.height=90;
      const context=c.getContext('2d');context.drawImage(image,0,0);return context.getImageData(0,0,160,90).data; }
    finally { image.close(); }
  };
  const originalPixels = await pixels(samples[0]), exifPixels = await pixels(oriented);
  if (originalPixels.some((v,i)=>v!==exifPixels[i])) throw new Error('EXIF changed camera optical pixels');
  const work = [];
  for (let sequence=0;sequence<40;sequence++) {
    work.push({camera:0,variant:sequence%32});
    for (let camera=1;camera<8;camera++) {
      if (sequence%2===0 && (camera!==7 || sequence<38)) work.push({camera,variant:(sequence+camera*7)%32});
    }
  }
  if (work.length !== 179) throw new Error('invalid comparison worklist');
  const modes = ['wasm-rgba','bitmap-full','bitmap-fhd','bitmap-fhd-workers2'];
  const runs=[];
  for (let repeat=0;repeat<3;repeat++) {
    // Rotate order; pixels/camera/worklist stay identical in every mode.
    for (const mode of modes.slice(repeat).concat(modes.slice(0,repeat))) {
      window.jpegBenchmarkProgress={repeat,mode,runs:runs.length};
      const canvases=Array.from({length:8},()=>document.createElement('canvas'));
      const durations=[];
      let heartbeat=0, previous=performance.now(), maxHeartbeatGap=0;
      const timer=setInterval(()=>{const now=performance.now();maxHeartbeatGap=Math.max(maxHeartbeatGap,now-previous);previous=now;heartbeat++;},10);
      const started=performance.now();
      const draw=async job=>{
        const before=performance.now(),canvas=canvases[job.camera];
        if (mode==='wasm-rgba') bindings.benchmark_jpeg_canvas(samples[job.variant],canvas);
        else {
          const size=mode==='bitmap-full' ? [3840,2160] : job.camera===0 ? [1920,1080] : [160,90];
          const bitmap=await decodeCameraBitmap(samples[job.variant],...size);
          try { if(canvas.width!==bitmap.width)canvas.width=bitmap.width;if(canvas.height!==bitmap.height)canvas.height=bitmap.height;canvas.getContext('2d').drawImage(bitmap,0,0); }
          finally { bitmap.close(); }
        }
        durations.push(performance.now()-before);
      };
      const workers=mode.endsWith('workers2') ? 2 : 1;
      let workMs=0;
      for (let i=0;i<work.length;) {
        const batch=[work[i++]];
        if (workers===2 && i<work.length && work[i].camera!==batch[0].camera) batch.push(work[i++]);
        const batchStarted=performance.now();
        await Promise.all(batch.map(draw));
        workMs+=performance.now()-batchStarted;
        // Every mode yields between batches, like a real UI tick; no microtask-only starvation.
        await new Promise(r=>setTimeout(r,0));
      }
      const elapsedMs=performance.now()-started;
      // Let the timer observe a block before clearing; excludes this wait from workMs.
      await new Promise(r=>setTimeout(r,20));clearInterval(timer);
      const retainedPixels=canvases.reduce((sum,c)=>sum+c.width*c.height*4,0);
      const sorted=durations.slice().sort((a,b)=>a-b);
      runs.push({repeat,mode,workMs,elapsedMs,decoded:durations.length,jobP50Ms:sorted[Math.floor(sorted.length*.5)],jobP95Ms:sorted[Math.floor(sorted.length*.95)],maxHeartbeatGapMs:maxHeartbeatGap,heartbeat,retainedCanvasBytes:retainedPixels,wasmLinearBytes:bindings.benchmark_wasm_bytes()});
      for (const canvas of canvases) {canvas.width=0;canvas.height=0;}
      await new Promise(r=>setTimeout(r,100));
    }
  }
  const median=xs=>xs.sort((a,b)=>a-b)[Math.floor(xs.length/2)];
  const summary=modes.map(mode=>{const group=runs.filter(r=>r.mode===mode);return {mode,workMs:median(group.map(r=>r.workMs)),maxHeartbeatGapMs:median(group.map(r=>r.maxHeartbeatGapMs)),retainedCanvasBytes:group[0].retainedCanvasBytes};});
  return {browser:navigator.userAgent,display:{width:innerWidth,height:innerHeight,dpr:devicePixelRatio},manifest,work,runs,summary,
    tests:'EXIF orientation ignored; production queue coalescing + seek generations + two-job limit + byte lease release; corrupt JPEG then valid; exact resized dimensions',
    scope:'JPEG + base canvas only; fixed 179 jobs, at most one active job per camera, macro-task yield between batches excluded from workMs, no MCAP I/O, no frame drops, no visible DOM/scene GPU rendering; cache allowed; one page, rotated order; canvas bytes are dimensions*4, not measured RSS'};
})()
