//! One cancellable selected-field query worker per session, with bounded UI products.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
use viewer_core::{McapSource, NumericPlotOverview, NumericSeriesRequest};

pub(crate) struct NumericSeriesState {
    pub request: NumericSeriesRequest,
    pub result: Option<Result<NumericPlotOverview, String>>,
}
struct Worker {
    request: NumericSeriesRequest,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Result<NumericPlotOverview, String>>,
}
#[derive(Default)]
pub(crate) struct NumericPlotLoader {
    pub series: Vec<NumericSeriesState>,
    worker: Option<Worker>,
}
impl NumericPlotLoader {
    pub fn add(&mut self, request: NumericSeriesRequest) {
        if self.series.len() < 8 && !self.series.iter().any(|s| s.request == request) {
            self.series.push(NumericSeriesState {
                request,
                result: None,
            });
        }
    }
    pub fn remove(&mut self, stream: viewer_core::StreamId, path: &str) {
        self.series
            .retain(|s| s.request.stream_id != stream || s.request.field_path != path);
        if let Some(worker) = &self.worker
            && worker.request.stream_id == stream
            && worker.request.field_path == path
        {
            worker.cancel.store(true, Ordering::Relaxed);
        }
    }
    pub fn poll(&mut self, path: Option<&PathBuf>) {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.handle.is_finished())
        {
            let worker = self.worker.take().unwrap();
            let cancelled = worker.cancel.load(Ordering::Relaxed);
            let result = worker
                .handle
                .join()
                .unwrap_or_else(|_| Err("numeric plot worker failed".into()));
            if !cancelled
                && let Some(series) = self.series.iter_mut().find(|s| s.request == worker.request)
            {
                series.result = Some(result);
            }
        }
        if self.worker.is_some() {
            return;
        }
        let Some(path) = path.cloned() else {
            return;
        };
        let Some(series) = self.series.iter_mut().find(|s| s.result.is_none()) else {
            return;
        };
        let request = series.request.clone();
        let job = request.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let job_cancel = cancel.clone();
        match std::thread::Builder::new()
            .name("numeric-plot".into())
            .spawn(move || {
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                // SAFETY: recording mapping is read-only, with the same file immutability contract
                // as the session's playback source.
                let mapping = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| e.to_string())?;
                let source = McapSource::from_owner(mapping).map_err(|e| e.to_string())?;
                source
                    .query_numeric_overview_cancellable(&job, || job_cancel.load(Ordering::Relaxed))
            }) {
            Ok(handle) => {
                self.worker = Some(Worker {
                    request,
                    cancel,
                    handle,
                })
            }
            Err(error) => series.result = Some(Err(error.to_string())),
        }
    }
}
impl Drop for NumericPlotLoader {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, DataWindowTimeRange, StreamId};
    fn request(stream: u32) -> NumericSeriesRequest {
        NumericSeriesRequest {
            stream_id: StreamId(stream),
            field_path: "value".into(),
            range: DataWindowTimeRange::new(ArrivalTime(0), ArrivalTime(10)).unwrap(),
            max_points: 10,
        }
    }
    #[test]
    fn demand_is_deduplicated_and_series_count_is_bounded() {
        let mut loader = NumericPlotLoader::default();
        for id in 0..100 {
            loader.add(request(id));
            loader.add(request(id));
        }
        assert_eq!(loader.series.len(), 8);
        loader.remove(StreamId(0), "value");
        loader.add(request(99));
        assert_eq!(loader.series.len(), 8);
        assert!(
            loader
                .series
                .iter()
                .any(|s| s.request.stream_id == StreamId(99))
        );
    }
    #[test]
    fn removal_and_readdition_cannot_commit_cancelled_result() {
        let mut loader = NumericPlotLoader::default();
        loader.add(request(1));
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            receive.recv().unwrap();
            Err("stale result".into())
        });
        loader.worker = Some(Worker {
            request: request(1),
            cancel: cancel.clone(),
            handle,
        });
        loader.remove(StreamId(1), "value");
        loader.add(request(1));
        assert!(cancel.load(Ordering::Relaxed));
        send.send(()).unwrap();
        while !loader.worker.as_ref().unwrap().handle.is_finished() {
            std::thread::yield_now();
        }
        loader.poll(None);
        assert!(loader.worker.is_none());
        assert!(loader.series[0].result.is_none());
    }
}
