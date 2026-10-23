// Observe production canvas contents; no app API, payload, decoder or layout patches.
(() => {
  const probe = window.recordingProbe = {phase: 'open', beats: [], changes: [], presentations: [], errors: [], seekToken: 0};
  addEventListener('TrunkApplicationStarted', event => { probe.memory = event.detail.wasm.memory; });
  addEventListener('error', event => probe.errors.push(event.message));
  addEventListener('unhandledrejection', event => probe.errors.push(String(event.reason)));
  setInterval(() => probe.beats.push({t: performance.now(), phase: probe.phase}), 20);
  const requestAdapter = navigator.gpu.requestAdapter.bind(navigator.gpu);
  navigator.gpu.requestAdapter = async (...args) => {
    const adapter = await requestAdapter(...args);
    if (adapter) probe.adapter = {vendor: adapter.info.vendor, architecture: adapter.info.architecture,
      device: adapter.info.device, description: adapter.info.description, fallback: adapter.info.isFallbackAdapter};
    return adapter;
  };
  const bitmaps = new WeakMap(), canvasJobs = new WeakMap(), shownJobs = new Map();
  let nextJob = 0;
  const NativeWorker = window.Worker;
  window.Worker = class extends NativeWorker {
    constructor(url, options) {
      super(url, options);
      if (!String(url).includes('camera_decode_worker.js')) return;
      let active;
      const post = this.postMessage.bind(this);
      this.postMessage = (...args) => {
        if (active) probe.errors.push('two camera jobs submitted to one worker');
        active = {id: ++nextJob, seekToken: probe.seekToken};
        return post(...args);
      };
      this.addEventListener('message', event => {
        if (active && event.data.bitmap) bitmaps.set(event.data.bitmap, active);
        if (event.data.error) probe.errors.push(event.data.error);
        active = undefined;
      });
    }
  };
  const draw = CanvasRenderingContext2D.prototype.drawImage;
  const latest = new Map();
  const tiny = document.createElement('canvas'); tiny.width = 16; tiny.height = 9;
  const context = tiny.getContext('2d', {willReadFrequently: true});
  CanvasRenderingContext2D.prototype.drawImage = function(...args) {
    const result = draw.apply(this, args);
    const id = this.canvas.id;
    const job = bitmaps.get(args[0]) || canvasJobs.get(args[0]);
    if (job) canvasJobs.set(this.canvas, job);
    if (id === 'camera' || /^camera-thumb-\d+$/.test(id)) {
      if (job && shownJobs.get(id) !== job.id) {
        shownJobs.set(id, job.id);
        probe.presentations.push({id, job: job.id, seekToken: job.seekToken, t: performance.now()});
      }
      context.imageSmoothingEnabled = false;
      draw.call(context, this.canvas, 0, 0, 16, 9);
      let hash = 2166136261;
      for (const byte of context.getImageData(0, 0, 16, 9).data) hash = Math.imul(hash ^ byte, 16777619);
      if (latest.get(id) !== hash) {
        latest.set(id, hash);
        probe.changes.push({id, hash: hash >>> 0, t: performance.now(), phase: probe.phase});
      }
    }
    return result;
  };
  probe.state = () => ({now: performance.now(), phase: probe.phase,
    cursor: Number(document.querySelector('#timeline')?.value),
    status: document.querySelector('#status')?.textContent,
    metrics: document.querySelector('#performance')?.textContent,
    cards: document.querySelectorAll('.camera-card').length,
    canvases: [...document.querySelectorAll('#camera, [id^="camera-thumb-"]')]
      .map(canvas => ({id: canvas.id, width: canvas.width, height: canvas.height})),
    visibility: document.visibilityState, focused: document.hasFocus(),
    changes: probe.changes.length, presented: [...shownJobs.keys()], wasm_bytes: probe.memory?.buffer.byteLength,
    adapter: probe.adapter, errors: probe.errors});
})();
