use std::{error::Error, fmt, time::Duration};
use viewer_core::{
    ArrivalTime, DataWindowTimeRange, FetchDemand, FetchPlanner, MemoryWindowStore, RawMessage,
    SerializedWindow,
};

const DEFAULT_WINDOW_SIZE: Duration = Duration::from_secs(1);
const DEFAULT_TARGET_AHEAD: Duration = Duration::from_secs(2);
const DEFAULT_MAX_RESIDENT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WindowLoaderMetrics {
    pub load_requests: u64,
    pub source_reads: u64,
    pub source_bytes: u64,
    pub messages_loaded: u64,
    pub request_latency_ms: f64,
    pub last_window_latency_ms: f64,
    pub stale_results_discarded: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WindowLoadDiagnostics {
    pub source_reads: u64,
    pub source_bytes: usize,
    pub latency_ms: f64,
}

#[derive(Debug)]
pub(crate) struct LoadedWindow {
    pub window: SerializedWindow,
    pub diagnostics: WindowLoadDiagnostics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataLoadError(String);

impl DataLoadError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DataLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for DataLoadError {}

/// Poll-based boundary implemented by browser and remote recording loaders.
pub(crate) trait WindowLoader {
    fn request(&mut self, range: DataWindowTimeRange) -> Result<(), DataLoadError>;
    fn poll(&mut self) -> Option<Result<LoadedWindow, DataLoadError>>;
    fn cancel(&mut self);
    fn is_idle(&self) -> bool;
    fn metrics(&self) -> &WindowLoaderMetrics;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RecordingDataPlaneDiagnostics {
    pub load_requests: u64,
    pub source_reads: u64,
    pub source_bytes: u64,
    pub messages_loaded: u64,
    pub last_window_latency_ms: f64,
    pub stale_results_discarded: u64,
    pub buffering_count: u64,
    pub window_count: usize,
    pub resident_bytes: usize,
    pub buffer_ahead: Duration,
    pub eviction_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataPlaneError(String);

impl DataPlaneError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DataPlaneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for DataPlaneError {}

pub(crate) struct RecordingDataPlane<L> {
    planner: FetchPlanner,
    store: MemoryWindowStore,
    loader: L,
    end_exclusive: ArrivalTime,
    buffering_count: u64,
    delivery_started: bool,
    failed: Option<DataPlaneError>,
}

impl<L: WindowLoader> RecordingDataPlane<L> {
    pub(crate) fn new(
        loader: L,
        start: ArrivalTime,
        end_exclusive: ArrivalTime,
    ) -> Result<Self, DataPlaneError> {
        Self::with_limits(
            loader,
            start,
            end_exclusive,
            DEFAULT_WINDOW_SIZE,
            DEFAULT_TARGET_AHEAD,
            DEFAULT_MAX_RESIDENT_BYTES,
        )
    }

    fn with_limits(
        loader: L,
        start: ArrivalTime,
        end_exclusive: ArrivalTime,
        window_size: Duration,
        target_ahead: Duration,
        max_resident_bytes: usize,
    ) -> Result<Self, DataPlaneError> {
        if start >= end_exclusive {
            return Err(DataPlaneError::new("recording data range is empty"));
        }
        Ok(Self {
            planner: FetchPlanner::new(window_size, target_ahead)
                .map_err(|error| DataPlaneError::new(error.to_string()))?,
            store: MemoryWindowStore::new(start, max_resident_bytes)
                .map_err(|error| DataPlaneError::new(error.to_string()))?,
            loader,
            end_exclusive,
            buffering_count: 0,
            delivery_started: false,
            failed: None,
        })
    }

    pub(crate) fn poll(&mut self, cursor: ArrivalTime) -> Result<bool, DataPlaneError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let Some(result) = self.loader.poll() else {
            return Ok(false);
        };
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(error) => {
                let error = DataPlaneError::new(error.to_string());
                self.failed = Some(error.clone());
                return Err(error);
            }
        };
        if let Err(error) = self.store.insert(loaded.window) {
            let error = DataPlaneError::new(error.to_string());
            self.failed = Some(error.clone());
            return Err(error);
        }
        self.store.enforce_budget(cursor);
        Ok(true)
    }

    pub(crate) fn ensure_available_through(
        &mut self,
        target: ArrivalTime,
    ) -> Result<(), DataPlaneError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if !self.loader.is_idle() {
            return Ok(());
        }
        let demand = FetchDemand {
            cursor: target,
            complete_until: self.store.complete_until(),
            end_exclusive: self.end_exclusive,
        };
        let Some(range) = self.planner.plan(demand) else {
            return Ok(());
        };
        if let Err(error) = self.loader.request(range) {
            let error = DataPlaneError::new(error.to_string());
            self.failed = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn is_complete_through(&self, target: ArrivalTime) -> bool {
        self.store.is_complete_through(target, self.end_exclusive)
    }

    pub(crate) fn messages_through(
        &mut self,
        after: ArrivalTime,
        through: ArrivalTime,
    ) -> Vec<RawMessage> {
        let messages = self
            .store
            .take_messages_through(after, through, !self.delivery_started);
        self.delivery_started = true;
        messages
    }

    pub(crate) fn buffer_ahead(&self, cursor: ArrivalTime) -> Duration {
        Duration::from_nanos(
            self.store
                .complete_until()
                .0
                .saturating_sub(cursor.0)
                .max(0) as u64,
        )
    }

    pub(crate) fn note_buffering(&mut self) {
        self.buffering_count = self.buffering_count.saturating_add(1);
    }

    pub(crate) fn diagnostics(&self, cursor: ArrivalTime) -> RecordingDataPlaneDiagnostics {
        let loader = self.loader.metrics();
        RecordingDataPlaneDiagnostics {
            load_requests: loader.load_requests,
            source_reads: loader.source_reads,
            source_bytes: loader.source_bytes,
            messages_loaded: loader.messages_loaded,
            last_window_latency_ms: loader.last_window_latency_ms,
            stale_results_discarded: loader.stale_results_discarded,
            buffering_count: self.buffering_count,
            window_count: self.store.window_count(),
            resident_bytes: self.store.resident_bytes(),
            buffer_ahead: self.buffer_ahead(cursor),
            eviction_count: self.store.eviction_count(),
        }
    }

    #[cfg(test)]
    pub(crate) fn loader_mut(&mut self) -> &mut L {
        &mut self.loader
    }

    #[cfg(test)]
    pub(crate) fn resident_bytes(&self) -> usize {
        self.store.resident_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};
    use viewer_core::StreamId;

    const SECOND: i64 = 1_000_000_000;

    #[derive(Default)]
    struct TestLoaderState {
        requested: Option<DataWindowTimeRange>,
        results: VecDeque<Result<LoadedWindow, DataLoadError>>,
    }

    struct TestLoader {
        shared: Rc<RefCell<TestLoaderState>>,
        metrics: WindowLoaderMetrics,
    }

    impl WindowLoader for TestLoader {
        fn request(&mut self, range: DataWindowTimeRange) -> Result<(), DataLoadError> {
            let mut shared = self.shared.borrow_mut();
            if shared.requested.is_some() {
                return Err(DataLoadError::new("test loader is not idle"));
            }
            shared.requested = Some(range);
            self.metrics.load_requests += 1;
            Ok(())
        }

        fn poll(&mut self) -> Option<Result<LoadedWindow, DataLoadError>> {
            let result = self.shared.borrow_mut().results.pop_front()?;
            self.shared.borrow_mut().requested = None;
            Some(result)
        }

        fn cancel(&mut self) {
            self.shared.borrow_mut().requested = None;
        }

        fn is_idle(&self) -> bool {
            self.shared.borrow().requested.is_none()
        }

        fn metrics(&self) -> &WindowLoaderMetrics {
            &self.metrics
        }
    }

    fn data_plane(
        max_bytes: usize,
    ) -> (RecordingDataPlane<TestLoader>, Rc<RefCell<TestLoaderState>>) {
        let shared = Rc::new(RefCell::new(TestLoaderState::default()));
        let loader = TestLoader {
            shared: Rc::clone(&shared),
            metrics: WindowLoaderMetrics::default(),
        };
        let data = RecordingDataPlane::with_limits(
            loader,
            ArrivalTime(0),
            ArrivalTime(4 * SECOND),
            Duration::from_secs(1),
            Duration::from_secs(2),
            max_bytes,
        )
        .unwrap();
        (data, shared)
    }

    fn loaded(
        start: i64,
        end: i64,
        messages: Vec<RawMessage>,
        resident_bytes: usize,
    ) -> LoadedWindow {
        LoadedWindow {
            window: SerializedWindow::new(
                DataWindowTimeRange::new(ArrivalTime(start), ArrivalTime(end)).unwrap(),
                messages,
                resident_bytes,
            )
            .unwrap(),
            diagnostics: WindowLoadDiagnostics {
                source_reads: 1,
                source_bytes: resident_bytes,
                latency_ms: 1.0,
            },
        }
    }

    fn message(time: i64, payload: Bytes) -> RawMessage {
        RawMessage {
            stream_id: StreamId(1),
            arrival_time: ArrivalTime(time),
            payload,
        }
    }

    fn complete_request(
        data: &mut RecordingDataPlane<TestLoader>,
        shared: &Rc<RefCell<TestLoaderState>>,
        loaded: LoadedWindow,
        cursor: ArrivalTime,
    ) {
        shared.borrow_mut().results.push_back(Ok(loaded));
        assert!(data.poll(cursor).unwrap());
    }

    #[test]
    fn complete_windows_publish_once_without_copying_payloads() {
        let (mut data, shared) = data_plane(1024);
        data.ensure_available_through(ArrivalTime(0)).unwrap();
        let backing = Bytes::from_static(b"batch-payload");
        let payload = backing.slice(6..);
        let pointer = payload.as_ptr();
        complete_request(
            &mut data,
            &shared,
            loaded(0, SECOND, vec![message(0, payload)], backing.len()),
            ArrivalTime(0),
        );

        let messages = data.messages_through(ArrivalTime(0), ArrivalTime(SECOND - 1));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload.as_ptr(), pointer);
        assert!(
            data.messages_through(ArrivalTime(0), ArrivalTime(SECOND - 1))
                .is_empty()
        );
        assert!(data.is_complete_through(ArrivalTime(SECOND - 1)));
        assert!(!data.is_complete_through(ArrivalTime(SECOND)));
    }

    #[test]
    fn ram_budget_evicts_old_windows_after_cursor_advances() {
        let (mut data, shared) = data_plane(8);
        for index in 0..3 {
            let start = index * SECOND;
            data.ensure_available_through(ArrivalTime(start)).unwrap();
            complete_request(
                &mut data,
                &shared,
                loaded(start, start + SECOND, vec![], 4),
                ArrivalTime(start),
            );
        }

        let diagnostics = data.diagnostics(ArrivalTime(SECOND + 1));
        assert_eq!(diagnostics.window_count, 2);
        assert_eq!(diagnostics.resident_bytes, 8);
        assert_eq!(diagnostics.eviction_count, 1);
    }

    #[test]
    fn loader_failure_never_reaches_the_store() {
        let (mut data, shared) = data_plane(1024);
        data.ensure_available_through(ArrivalTime(0)).unwrap();
        shared
            .borrow_mut()
            .results
            .push_back(Err(DataLoadError::new("stale or failed load")));

        assert!(data.poll(ArrivalTime(0)).is_err());
        assert_eq!(data.resident_bytes(), 0);
    }
}
