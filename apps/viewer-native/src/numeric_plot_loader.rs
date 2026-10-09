//! One cancellable selected-field query worker per session, with bounded UI products.
use crate::query_error::QueryLoadError;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
#[cfg(test)]
use viewer_core::NumericPlotError;
use viewer_core::{
    McapSource, NumericPlotOverview, NumericPlotOverviewBatch, NumericSeriesRequest,
};

pub(crate) struct NumericSeriesState {
    pub request: NumericSeriesRequest,
    pub result: Option<Result<NumericPlotOverview, Arc<QueryLoadError>>>,
}
struct Worker {
    requests: Vec<NumericSeriesRequest>,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Result<NumericPlotOverviewBatch, Arc<QueryLoadError>>>,
}
#[derive(Default)]
pub(crate) struct NumericPlotLoader {
    pub series: Vec<NumericSeriesState>,
    worker: Option<Worker>,
}
impl NumericPlotLoader {
    pub fn add(&mut self, request: NumericSeriesRequest) {
        if self.series.len() < 8 && !self.series.iter().any(|s| s.request == request) {
            if let Some(worker) = &self.worker
                && worker
                    .requests
                    .iter()
                    .any(|r| r.stream_id == request.stream_id && r.range == request.range)
            {
                // Restart a still-loading group so newly selected fields share its full scan.
                worker.cancel.store(true, Ordering::Relaxed);
            }
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
            && worker
                .requests
                .iter()
                .any(|r| r.stream_id == stream && r.field_path == path)
        {
            worker.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Keep one worker across source replacement without blocking the UI on a chunk decode.
    pub fn take_worker_from(&mut self, previous: &mut Self) {
        if let Some(worker) = previous.worker.take() {
            worker.cancel.store(true, Ordering::Relaxed);
            assert!(
                self.worker.is_none(),
                "new source has not started numeric queries"
            );
            self.worker = Some(worker);
        }
    }
    pub fn poll(&mut self, source: Option<&McapSource>) {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.handle.is_finished())
        {
            let worker = self.worker.take().unwrap();
            let cancelled = worker.cancel.load(Ordering::Relaxed);
            let result = worker.handle.join().unwrap_or_else(|_| {
                Err(Arc::new(QueryLoadError::WorkerPanicked {
                    worker: "numeric plot",
                }))
            });
            if !cancelled {
                let results = match result {
                    Ok(batch) => batch.series.into_iter().map(Ok).collect::<Vec<_>>(),
                    Err(error) => worker.requests.iter().map(|_| Err(error.clone())).collect(),
                };
                for (request, result) in worker.requests.into_iter().zip(results) {
                    if let Some(series) = self.series.iter_mut().find(|s| s.request == request) {
                        series.result = Some(result);
                    }
                }
            }
        }
        if self.worker.is_some() {
            return;
        }
        let Some(source) = source else {
            return;
        };
        let Some(first) = self.series.iter().find(|s| s.result.is_none()) else {
            return;
        };
        let requests = self
            .series
            .iter()
            .filter(|s| {
                s.result.is_none()
                    && s.request.stream_id == first.request.stream_id
                    && s.request.range == first.request.range
            })
            .map(|s| s.request.clone())
            .collect::<Vec<_>>();
        let job = requests.clone();
        let source = source.fork_reader();
        let cancel = Arc::new(AtomicBool::new(false));
        let job_cancel = cancel.clone();
        match std::thread::Builder::new()
            .name("numeric-plot".into())
            .spawn(move || {
                source
                    .query_numeric_overviews_cancellable(&job, || {
                        job_cancel.load(Ordering::Relaxed)
                    })
                    .map_err(|error| Arc::new(QueryLoadError::Numeric(error)))
            }) {
            Ok(handle) => {
                self.worker = Some(Worker {
                    requests,
                    cancel,
                    handle,
                })
            }
            Err(error) => {
                let error = Arc::new(QueryLoadError::StartWorker {
                    worker: "numeric plot",
                    source: error,
                });
                for series in self
                    .series
                    .iter_mut()
                    .filter(|s| requests.contains(&s.request))
                {
                    series.result = Some(Err(error.clone()));
                }
            }
        }
    }
}
impl Drop for NumericPlotLoader {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.cancel.store(true, Ordering::Relaxed);
            // Shutdown/direct session disposal waits for the one in-flight decode; no detached work.
            let _ = worker.handle.join();
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
            receive
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Err(Arc::new(QueryLoadError::Numeric(
                NumericPlotError::UnknownStream(StreamId(1)),
            )))
        });
        loader.worker = Some(Worker {
            requests: vec![request(1)],
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

#[cfg(test)]
mod source_replacement_tests {
    use super::*;
    use viewer_core::{ArrivalTime, DataWindowTimeRange, StreamId};
    #[test]
    fn source_replacement_retains_one_cancelled_worker_and_discards_old_results() {
        let request = NumericSeriesRequest {
            stream_id: StreamId(1),
            field_path: "value".into(),
            range: DataWindowTimeRange::new(ArrivalTime(0), ArrivalTime(10)).unwrap(),
            max_points: 10,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            receive
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Ok(NumericPlotOverviewBatch {
                series: vec![NumericPlotOverview {
                    samples: vec![],
                    input_sample_count: 42,
                    decode_errors: 0,
                }],
                chunks_read: 1,
            })
        });
        let mut previous = NumericPlotLoader::default();
        previous.add(request.clone());
        previous.worker = Some(Worker {
            requests: vec![request.clone()],
            cancel: cancel.clone(),
            handle,
        });
        for _ in 0..5 {
            let mut next = NumericPlotLoader::default();
            next.add(request.clone());
            next.take_worker_from(&mut previous);
            assert!(previous.worker.is_none());
            next.poll(None);
            assert!(next.worker.is_some());
            assert!(next.series[0].result.is_none());
            previous = next;
        }
        assert!(cancel.load(Ordering::Relaxed));
        send.send(()).unwrap();
        while !previous.worker.as_ref().unwrap().handle.is_finished() {
            std::thread::yield_now();
        }
        previous.poll(None);
        assert!(previous.worker.is_none());
        assert!(previous.series[0].result.is_none());
    }
}

#[cfg(test)]
mod regroup_tests {
    use super::*;
    use viewer_core::{ArrivalTime, DataWindowTimeRange, StreamId};
    #[test]
    fn new_field_cancels_inflight_group_and_retains_all_pending_demands() {
        let request = NumericSeriesRequest {
            stream_id: StreamId(1),
            field_path: "x".into(),
            range: DataWindowTimeRange::new(ArrivalTime(0), ArrivalTime(10)).unwrap(),
            max_points: 10,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            receive
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Err(Arc::new(QueryLoadError::Numeric(
                NumericPlotError::UnknownStream(StreamId(1)),
            )))
        });
        let mut loader = NumericPlotLoader::default();
        loader.add(request.clone());
        loader.worker = Some(Worker {
            requests: vec![request.clone()],
            cancel: cancel.clone(),
            handle,
        });
        let mut second = request;
        second.field_path = "y".into();
        loader.add(second);
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(loader.series.len(), 2);
        send.send(()).unwrap();
        while !loader.worker.as_ref().unwrap().handle.is_finished() {
            std::thread::yield_now();
        }
        loader.poll(None);
        assert!(loader.worker.is_none());
        assert!(loader.series.iter().all(|s| s.result.is_none()));
    }
}
