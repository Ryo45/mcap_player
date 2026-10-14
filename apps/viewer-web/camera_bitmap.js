// Rust owns admission/coalescing and generations; this pool only runs two jobs.
const slots = [null, null];
function slot(index) {
  if (slots[index]) return slots[index];
  const worker = new Worker(new URL('camera_decode_worker.js', document.baseURI));
  const state = {worker, job: null};
  worker.onmessage = ({data}) => {
    const job = state.job;
    state.job = null;
    if (!job) { data.bitmap?.close(); return; }
    if (data.error) job.reject(new Error(data.error));
    else job.resolve(data.bitmap);
  };
  worker.onerror = event => {
    const job = state.job;
    state.job = null;
    worker.terminate();
    slots[index] = null;
    job?.reject(new Error(event.message || 'camera worker failed'));
  };
  slots[index] = state;
  return state;
}
export function decodeCameraBitmap(bytes, width, height) {
  return new Promise((resolve, reject) => {
    let index = slots.findIndex(state => !state || !state.job);
    if (index < 0) { reject(new Error('camera worker admission exceeded')); return; }
    const state = slot(index);
    // Copy out of WASM before returning, then transfer rather than cloning to worker.
    const input = new Uint8Array(bytes);
    state.job = {resolve, reject};
    try { state.worker.postMessage({bytes: input.buffer, width, height}, [input.buffer]); }
    catch (error) { state.job = null; reject(error); }
  });
}
