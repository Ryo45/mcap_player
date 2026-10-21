//! Bounded native camera pixel work. GPU uploads remain on the UI thread.
use crate::{NativeCameraDecoder, NativeImage, NativeImageError};
use bytes::Bytes;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use viewer_core::{ArrivalTime, CameraFrame, CameraFrameMetadata, CameraId};

const MAX_CAMERAS: usize = 32;
const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

type RequestKey = (ArrivalTime, (u32, u32));

struct EncodedImage {
    data: Vec<u8>,
    bytes: Arc<AtomicUsize>,
}
impl AsRef<[u8]> for EncodedImage {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}
impl Drop for EncodedImage {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.data.len(), Ordering::Relaxed);
    }
}
struct WorkLease(Arc<AtomicUsize>);
impl Drop for WorkLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
struct Request {
    frame: CameraFrame,
    target: (u32, u32),
    generation: u64,
    lease: WorkLease,
}

pub struct CameraDecodeResult {
    pub frame: CameraFrameMetadata,
    pub image: Result<NativeImage, NativeImageError>,
    pub decode_time: Duration,
    generation: u64,
    _lease: WorkLease,
}
#[derive(Default)]
struct State {
    generation: u64,
    stopped: bool,
    pending: BTreeMap<CameraId, Request>,
    active: BTreeSet<CameraId>,
    submitted: BTreeMap<CameraId, RequestKey>,
    last_camera: Option<CameraId>,
}
struct Shared {
    state: Mutex<State>,
    available: Condvar,
}

#[derive(Debug)]
pub enum CameraDecodeStartError {
    WorkerCount,
    Codec(NativeImageError),
    Thread(std::io::Error),
}
impl std::fmt::Display for CameraDecodeStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkerCount => write!(f, "camera decode requires 1..=4 workers"),
            Self::Codec(e) => e.fmt(f),
            Self::Thread(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for CameraDecodeStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WorkerCount => None,
            Self::Codec(e) => Some(e),
            Self::Thread(e) => Some(e),
        }
    }
}

/// One pending image per camera, at most one active decode per camera, and a
/// completion channel bounded by worker count. Owned image copies do not retain
/// MCAP chunk backing. The 64 MiB budget covers pending/active input; completed
/// results retain metadata and pixels only.
pub struct CameraDecodeQueue {
    shared: Arc<Shared>,
    ready: Option<Receiver<CameraDecodeResult>>,
    workers: Vec<JoinHandle<()>>,
    encoded_bytes: Arc<AtomicUsize>,
    outstanding: Arc<AtomicUsize>,
}
impl CameraDecodeQueue {
    pub fn new(worker_count: usize) -> Result<Self, CameraDecodeStartError> {
        if !(1..=4).contains(&worker_count) {
            return Err(CameraDecodeStartError::WorkerCount);
        }
        let (sender, receiver) = mpsc::sync_channel(worker_count);
        let mut queue = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                available: Condvar::new(),
            }),
            ready: Some(receiver),
            workers: Vec::new(),
            encoded_bytes: Arc::new(AtomicUsize::new(0)),
            outstanding: Arc::new(AtomicUsize::new(0)),
        };
        for index in 0..worker_count {
            let mut decoder = NativeCameraDecoder::new().map_err(CameraDecodeStartError::Codec)?;
            let shared = Arc::clone(&queue.shared);
            let sender = sender.clone();
            queue.workers.push(
                thread::Builder::new()
                    .name(format!("camera-decode-{index}"))
                    .spawn(move || {
                        loop {
                            let request = {
                                let mut state = shared.state.lock().unwrap();
                                loop {
                                    if state.stopped {
                                        return;
                                    }
                                    // Round robin prevents a busy low-numbered camera starving others.
                                    let id = state
                                        .pending
                                        .keys()
                                        .copied()
                                        .filter(|id| !state.active.contains(id))
                                        .find(|id| state.last_camera.is_none_or(|last| *id > last))
                                        .or_else(|| {
                                            state
                                                .pending
                                                .keys()
                                                .copied()
                                                .find(|id| !state.active.contains(id))
                                        });
                                    if let Some(id) = id {
                                        state.last_camera = Some(id);
                                        state.active.insert(id);
                                        break state.pending.remove(&id).unwrap();
                                    }
                                    state = shared.available.wait(state).unwrap();
                                }
                            };
                            let start = Instant::now();
                            let image = decoder.decode_frame(&request.frame, request.target);
                            let result = CameraDecodeResult {
                                frame: request.frame.into(),
                                image,
                                decode_time: start.elapsed(),
                                generation: request.generation,
                                _lease: request.lease,
                            };
                            let current = {
                                let state = shared.state.lock().unwrap();
                                !state.stopped && state.generation == result.generation
                            };
                            let id = result.frame.camera_id;
                            // Keep this camera active until its completion is queued, preserving order.
                            if current && sender.send(result).is_err() {
                                return;
                            }
                            let mut state = shared.state.lock().unwrap();
                            state.active.remove(&id);
                            shared.available.notify_all();
                        }
                    })
                    .map_err(CameraDecodeStartError::Thread)?,
            );
        }
        Ok(queue)
    }

    /// False means duplicate work or byte-budget backpressure; retry next UI tick.
    pub fn submit(
        &mut self,
        frame: &CameraFrame,
        target: (u32, u32),
    ) -> Result<bool, NativeImageError> {
        let mut state = self.shared.state.lock().unwrap();
        let key = (frame.arrival_time, target);
        if state.submitted.get(&frame.camera_id) == Some(&key) {
            return Ok(false);
        }
        if !state.submitted.contains_key(&frame.camera_id) && state.submitted.len() >= MAX_CAMERAS {
            return Err(NativeImageError::Limit("32 camera identities"));
        }
        if frame.data.len() > 32 * 1024 * 1024 || frame.frame_id.len() > 4096 {
            return Err(NativeImageError::Limit(
                "32 MiB image / 4096 byte frame identifier",
            ));
        }
        // Free the superseded pending copy before reserving its replacement.
        state.pending.remove(&frame.camera_id);
        let len = frame.data.len();
        if self
            .encoded_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bytes| {
                bytes.checked_add(len).filter(|n| *n <= MAX_ENCODED_BYTES)
            })
            .is_err()
        {
            return Ok(false);
        }
        let mut data = Vec::new();
        if let Err(error) = data.try_reserve_exact(len) {
            self.encoded_bytes.fetch_sub(len, Ordering::Relaxed);
            return Err(NativeImageError::Allocation(error));
        }
        data.extend_from_slice(&frame.data);
        let mut frame = frame.clone();
        frame.data = Bytes::from_owner(EncodedImage {
            data,
            bytes: Arc::clone(&self.encoded_bytes),
        });
        self.outstanding.fetch_add(1, Ordering::Relaxed);
        let generation = state.generation;
        state.submitted.insert(frame.camera_id, key);
        state.pending.insert(
            frame.camera_id,
            Request {
                frame,
                target,
                generation,
                lease: WorkLease(Arc::clone(&self.outstanding)),
            },
        );
        self.shared.available.notify_all();
        Ok(true)
    }

    pub fn take_ready(&mut self) -> Option<CameraDecodeResult> {
        while let Ok(result) = self.ready.as_ref()?.try_recv() {
            if result.generation == self.shared.state.lock().unwrap().generation {
                return Some(result);
            }
        }
        None
    }
    pub fn has_work(&self) -> bool {
        self.outstanding.load(Ordering::Relaxed) != 0
    }
    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes.load(Ordering::Relaxed)
    }

    /// Called only after a successful seek/source transition, never on failed restore.
    pub fn clear(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.pending.clear();
        state.submitted.clear();
        drop(state);
        while self.ready.as_ref().unwrap().try_recv().is_ok() {}
    }
}
impl Drop for CameraDecodeQueue {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.stopped = true;
            state.pending.clear();
            self.shared.available.notify_all();
        }
        // Disconnect before join so a full completion channel cannot deadlock shutdown.
        self.ready.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
    use viewer_core::MeasurementTime;

    fn frame(id: u16, time: i64) -> CameraFrame {
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg)
            .encode(&vec![128; 64 * 48 * 3], 64, 48, ExtendedColorType::Rgb8)
            .unwrap();
        CameraFrame {
            camera_id: CameraId(id),
            arrival_time: ArrivalTime(time),
            measurement_time: MeasurementTime(time - 1),
            frame_id: "camera_optical".into(),
            format: viewer_core::CameraImageFormat::Jpeg,
            data: jpeg.into(),
        }
    }
    fn complete(queue: &mut CameraDecodeQueue) -> CameraDecodeResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = queue.take_ready() {
                return result;
            }
            assert!(Instant::now() < deadline, "worker failed to complete");
            thread::yield_now();
        }
    }
    #[test]
    fn latest_pending_only_and_seek_discards_old_generation() {
        let mut queue = CameraDecodeQueue::new(2).unwrap();
        // Keep this camera active to control pending admission independently of scheduling.
        queue
            .shared
            .state
            .lock()
            .unwrap()
            .active
            .insert(CameraId(0));
        for time in 0..50 {
            assert!(queue.submit(&frame(0, time), (32, 24)).unwrap());
        }
        assert_eq!(queue.outstanding.load(Ordering::Relaxed), 1);
        assert_eq!(queue.encoded_bytes(), frame(0, 0).data.len());
        queue.clear();
        assert_eq!(queue.encoded_bytes(), 0);
        // Backward seek permits an older log time in the new generation.
        queue.submit(&frame(0, -10), (32, 24)).unwrap();
        queue
            .shared
            .state
            .lock()
            .unwrap()
            .active
            .remove(&CameraId(0));
        queue.shared.available.notify_all();
        let result = complete(&mut queue);
        assert_eq!(result.frame.arrival_time, ArrivalTime(-10));
        assert_eq!(result.frame.measurement_time, MeasurementTime(-11));
        assert_eq!(result.image.unwrap().geometry.source_size, (64, 48));
    }
    #[test]
    fn bad_camera_does_not_block_others_and_completions_release_input() {
        let mut queue = CameraDecodeQueue::new(2).unwrap();
        let mut good = frame(1, 1);
        let mut backing = vec![0; 1024 * 1024];
        backing[..good.data.len()].copy_from_slice(&good.data);
        good.data = Bytes::from(backing).slice(..good.data.len());
        let mut bad = frame(0, 1);
        bad.data = Bytes::from_static(b"not jpeg");
        {
            queue
                .shared
                .state
                .lock()
                .unwrap()
                .active
                .insert(good.camera_id);
            queue.submit(&good, (32, 24)).unwrap();
            let mut state = queue.shared.state.lock().unwrap();
            let pending = &state.pending[&good.camera_id].frame;
            assert_ne!(pending.data.as_ptr(), good.data.as_ptr());
            assert_eq!(pending.data, good.data);
            state.active.remove(&good.camera_id);
        }
        queue.shared.available.notify_all();
        queue.submit(&bad, (32, 24)).unwrap();
        let first = complete(&mut queue);
        let second = complete(&mut queue);
        let results = [first, second];
        let decoded = results
            .iter()
            .find(|r| r.frame.camera_id == CameraId(1))
            .unwrap();
        assert!(decoded.image.is_ok());
        assert_eq!(decoded.frame, CameraFrameMetadata::from(&good));
        assert!(
            results
                .iter()
                .find(|r| r.frame.camera_id == CameraId(0))
                .unwrap()
                .image
                .is_err()
        );
        assert_eq!(queue.encoded_bytes(), 0);
        drop(results);
        assert_eq!(queue.encoded_bytes(), 0);
        assert!(!queue.has_work());
    }
    #[test]
    fn byte_budget_backpressure_and_clear_release_pending_input() {
        let mut queue = CameraDecodeQueue::new(1).unwrap();
        queue
            .shared
            .state
            .lock()
            .unwrap()
            .active
            .extend([CameraId(0), CameraId(1), CameraId(2)]);
        let mut huge = frame(0, 0);
        huge.data = Bytes::from(vec![0; 32 * 1024 * 1024]);
        assert!(queue.submit(&huge, (32, 24)).unwrap());
        huge.camera_id = CameraId(1);
        assert!(queue.submit(&huge, (32, 24)).unwrap());
        assert_eq!(queue.encoded_bytes(), MAX_ENCODED_BYTES);
        assert!(!queue.submit(&frame(2, 0), (32, 24)).unwrap());
        queue.clear();
        assert_eq!(queue.encoded_bytes(), 0);
        assert!(!queue.has_work());
    }
    #[test]
    fn completed_images_release_saturated_budget_while_metadata_is_retained() {
        let mut queue = CameraDecodeQueue::new(2).unwrap();
        queue
            .shared
            .state
            .lock()
            .unwrap()
            .active
            .extend([CameraId(0), CameraId(1)]);
        let mut large = frame(0, 123456789);
        let mut jpeg = large.data.to_vec();
        // Valid JPEG with trailing padding: exercise the encoded limit without giant pixels.
        jpeg.resize(32 * 1024 * 1024, 0);
        large.data = jpeg.into();
        assert!(queue.submit(&large, (32, 24)).unwrap());
        large.camera_id = CameraId(1);
        assert!(queue.submit(&large, (32, 24)).unwrap());
        assert_eq!(queue.encoded_bytes(), MAX_ENCODED_BYTES);
        assert!(!queue.submit(&frame(2, 1), (32, 24)).unwrap());
        queue.shared.state.lock().unwrap().active.clear();
        queue.shared.available.notify_all();
        let results = [complete(&mut queue), complete(&mut queue)];
        let displayed: Vec<_> = results
            .iter()
            .map(|result| {
                assert_eq!(
                    result.image.as_ref().unwrap().geometry.source_size,
                    (64, 48)
                );
                assert_eq!(result.frame.arrival_time, large.arrival_time);
                assert_eq!(result.frame.measurement_time, large.measurement_time);
                assert_eq!(result.frame.frame_id, large.frame_id);
                result.frame.clone()
            })
            .collect();
        // Even retained pixel results and displayed metadata must not retain encoded leases.
        assert_eq!(queue.encoded_bytes(), 0);
        assert!(queue.submit(&frame(2, 2), (32, 24)).unwrap());
        let next = complete(&mut queue);
        assert!(next.image.is_ok());
        assert_eq!(next.frame.camera_id, CameraId(2));
        assert_eq!(displayed.len(), 2);
        assert_eq!(queue.encoded_bytes(), 0);
    }
    #[test]
    fn stale_ready_is_filtered_and_full_completion_queue_can_shutdown() {
        let mut queue = CameraDecodeQueue::new(2).unwrap();
        let (sender, receiver) = mpsc::sync_channel(2);
        queue.ready = Some(receiver);
        queue.outstanding.fetch_add(1, Ordering::Relaxed);
        sender
            .send(CameraDecodeResult {
                frame: frame(0, 0).into(),
                image: Err(NativeImageError::Limit("test")),
                decode_time: Duration::ZERO,
                generation: 0,
                _lease: WorkLease(Arc::clone(&queue.outstanding)),
            })
            .unwrap();
        queue.shared.state.lock().unwrap().generation = 1;
        assert!(queue.take_ready().is_none());
        assert!(!queue.has_work());
        // Real workers now see a disconnected receiver. Drop must still join cleanly.
        for id in 0..8 {
            queue.submit(&frame(id, 1), (32, 24)).unwrap();
        }
        drop(queue);
    }
    #[test]
    fn shutdown_disconnects_a_full_completion_channel() {
        let mut queue = CameraDecodeQueue::new(2).unwrap();
        for id in 0..8 {
            queue.submit(&frame(id, 1), (32, 24)).unwrap();
        }
        // Wait for at least one worker to be blocked on the bounded completion queue.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = queue.shared.state.lock().unwrap();
            if state.pending.len() <= 4 {
                break;
            }
            assert!(Instant::now() < deadline);
            drop(state);
            thread::yield_now();
        }
        drop(queue);
    }
}
