use std::{error::Error, fmt, time::Duration};
use viewer_core::{
    ArrivalTime, DataWindowTimeRange, FetchDemand, FetchIntent, FetchPlanner, FetchProfile,
    MemoryWindowStore, PlaybackSpeed, RawMessage, SerializedWindow,
};

#[cfg(target_arch = "wasm32")]
use crate::local::BrowserMcapWindowLoader;
use crate::remote::RemoteWindowLoader;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WindowLoaderMetrics {
    pub load_requests: u64,
    pub source_reads: u64,
    pub source_bytes: u64,
    pub decompressed_bytes: u64,
    pub per_message_copied_bytes: u64,
    pub messages_loaded: u64,
    pub request_latency_ms: f64,
    pub last_window_latency_ms: f64,
    pub last_processing_ms: f64,
    pub stale_results_discarded: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WindowLoadDiagnostics {
    pub source_reads: u64,
    pub source_bytes: usize,
    pub decompressed_bytes: usize,
    pub per_message_copied_bytes: usize,
    pub latency_ms: f64,
    pub processing_ms: f64,
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

pub(crate) enum WebWindowLoader {
    Remote(RemoteWindowLoader),
    #[cfg(target_arch = "wasm32")]
    LocalFile(BrowserMcapWindowLoader),
}

impl WindowLoader for WebWindowLoader {
    fn request(&mut self, range: DataWindowTimeRange) -> Result<(), DataLoadError> {
        match self {
            Self::Remote(loader) => loader.request(range),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => loader.request(range),
        }
    }

    fn poll(&mut self) -> Option<Result<LoadedWindow, DataLoadError>> {
        match self {
            Self::Remote(loader) => WindowLoader::poll(loader),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => WindowLoader::poll(loader),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Remote(loader) => loader.cancel(),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => loader.cancel(),
        }
    }

    fn is_idle(&self) -> bool {
        match self {
            Self::Remote(loader) => loader.is_idle(),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => loader.is_idle(),
        }
    }

    fn metrics(&self) -> &WindowLoaderMetrics {
        match self {
            Self::Remote(loader) => loader.metrics(),
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(loader) => loader.metrics(),
        }
    }
}

impl WebWindowLoader {
    #[cfg(test)]
    pub(crate) fn remote_mut(&mut self) -> &mut RemoteWindowLoader {
        match self {
            Self::Remote(loader) => loader,
            #[cfg(target_arch = "wasm32")]
            Self::LocalFile(_) => panic!("test expected a remote loader"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RecordingDataPlaneDiagnostics {
    pub load_requests: u64,
    pub source_reads: u64,
    pub source_bytes: u64,
    pub messages_loaded: u64,
    pub last_window_latency_ms: f64,
    pub last_processing_ms: f64,
    pub stale_results_discarded: u64,
    pub buffer_underrun_count: u64,
    pub window_count: usize,
    pub resident_bytes: usize,
    pub logical_payload_bytes: usize,
    pub retention_ratio: Option<f64>,
    pub decompressed_bytes: u64,
    pub per_message_copied_bytes: u64,
    pub target_ahead: Duration,
    pub actual_buffer_ahead: Duration,
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
    start: ArrivalTime,
    end_exclusive: ArrivalTime,
    buffer_underrun_count: u64,
    delivery_started: bool,
    failed: Option<DataPlaneError>,
}

impl<L: WindowLoader> RecordingDataPlane<L> {
    pub(crate) fn new(
        loader: L,
        start: ArrivalTime,
        end_exclusive: ArrivalTime,
    ) -> Result<Self, DataPlaneError> {
        Self::with_profile(loader, start, end_exclusive, FetchProfile::default())
    }

    fn with_profile(
        loader: L,
        start: ArrivalTime,
        end_exclusive: ArrivalTime,
        profile: FetchProfile,
    ) -> Result<Self, DataPlaneError> {
        if start >= end_exclusive {
            return Err(DataPlaneError::new("recording data range is empty"));
        }
        Ok(Self {
            planner: FetchPlanner::new(profile),
            store: MemoryWindowStore::new(start, profile.max_resident_bytes())
                .map_err(|error| DataPlaneError::new(error.to_string()))?,
            loader,
            start,
            end_exclusive,
            buffer_underrun_count: 0,
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
        committed: ArrivalTime,
        required_through: ArrivalTime,
        playback_speed: PlaybackSpeed,
        intent: FetchIntent,
    ) -> Result<(), DataPlaneError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if !self.loader.is_idle() {
            return Ok(());
        }
        let demand = FetchDemand {
            cursor: committed,
            required_through,
            complete_until: self.store.complete_until(),
            end_exclusive: self.end_exclusive,
            playback_speed,
            intent,
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

    pub(crate) fn begin_seek(&mut self, target: ArrivalTime) -> Result<(), DataPlaneError> {
        if target < self.start || target >= self.end_exclusive {
            return Err(DataPlaneError::new(format!(
                "seek target {} is outside recording [{}, {})",
                target.0, self.start.0, self.end_exclusive.0
            )));
        }
        self.loader.cancel();
        self.store.reset(target);
        self.delivery_started = false;
        self.failed = None;
        Ok(())
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

    pub(crate) fn note_buffer_underrun(&mut self) {
        self.buffer_underrun_count = self.buffer_underrun_count.saturating_add(1);
    }

    pub(crate) fn diagnostics(
        &self,
        cursor: ArrivalTime,
        playback_speed: PlaybackSpeed,
    ) -> RecordingDataPlaneDiagnostics {
        let loader = self.loader.metrics();
        let logical_payload_bytes = self.store.logical_payload_bytes();
        RecordingDataPlaneDiagnostics {
            load_requests: loader.load_requests,
            source_reads: loader.source_reads,
            source_bytes: loader.source_bytes,
            messages_loaded: loader.messages_loaded,
            last_window_latency_ms: loader.last_window_latency_ms,
            last_processing_ms: loader.last_processing_ms,
            stale_results_discarded: loader.stale_results_discarded,
            buffer_underrun_count: self.buffer_underrun_count,
            window_count: self.store.window_count(),
            resident_bytes: self.store.resident_bytes(),
            logical_payload_bytes,
            retention_ratio: (logical_payload_bytes != 0)
                .then(|| self.store.resident_bytes() as f64 / logical_payload_bytes as f64),
            decompressed_bytes: loader.decompressed_bytes,
            per_message_copied_bytes: loader.per_message_copied_bytes,
            target_ahead: self.planner.profile().target_ahead(playback_speed),
            actual_buffer_ahead: self.buffer_ahead(cursor),
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
        cancel_count: u64,
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
            let mut shared = self.shared.borrow_mut();
            shared.requested = None;
            shared.cancel_count = shared.cancel_count.saturating_add(1);
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
        let profile =
            FetchProfile::new(Duration::from_secs(1), Duration::from_secs(2), max_bytes).unwrap();
        let data = RecordingDataPlane::with_profile(
            loader,
            ArrivalTime(0),
            ArrivalTime(4 * SECOND),
            profile,
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
                decompressed_bytes: 0,
                per_message_copied_bytes: 0,
                latency_ms: 1.0,
                processing_ms: 0.0,
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
        data.ensure_available_through(
            ArrivalTime(0),
            ArrivalTime(0),
            PlaybackSpeed::Normal,
            FetchIntent::PlaybackAhead,
        )
        .unwrap();
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
        let diagnostics = data.diagnostics(ArrivalTime(0), PlaybackSpeed::Normal);
        assert_eq!(diagnostics.logical_payload_bytes, 7);
        assert_eq!(diagnostics.resident_bytes, backing.len());
        assert_eq!(
            diagnostics.retention_ratio,
            Some(backing.len() as f64 / 7.0)
        );
        assert_eq!(diagnostics.target_ahead, Duration::from_secs(2));
        assert_eq!(diagnostics.actual_buffer_ahead, Duration::from_secs(1));
        assert_eq!(diagnostics.buffer_underrun_count, 0);
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
            data.ensure_available_through(
                ArrivalTime(start),
                ArrivalTime(start),
                PlaybackSpeed::Normal,
                FetchIntent::PlaybackAhead,
            )
            .unwrap();
            complete_request(
                &mut data,
                &shared,
                loaded(start, start + SECOND, vec![], 4),
                ArrivalTime(start),
            );
        }

        let diagnostics = data.diagnostics(ArrivalTime(SECOND + 1), PlaybackSpeed::Normal);
        assert_eq!(diagnostics.window_count, 2);
        assert_eq!(diagnostics.resident_bytes, 8);
        assert_eq!(diagnostics.eviction_count, 1);
    }

    #[test]
    fn loader_failure_never_reaches_the_store() {
        let (mut data, shared) = data_plane(1024);
        data.ensure_available_through(
            ArrivalTime(0),
            ArrivalTime(0),
            PlaybackSpeed::Normal,
            FetchIntent::PlaybackAhead,
        )
        .unwrap();
        shared
            .borrow_mut()
            .results
            .push_back(Err(DataLoadError::new("stale or failed load")));

        assert!(data.poll(ArrivalTime(0)).is_err());
        assert_eq!(data.resident_bytes(), 0);
    }

    #[test]
    fn seek_cancels_the_old_request_and_rebases_the_next_required_window() {
        let (mut data, shared) = data_plane(1024);
        data.ensure_available_through(
            ArrivalTime(0),
            ArrivalTime(0),
            PlaybackSpeed::Normal,
            FetchIntent::PlaybackAhead,
        )
        .unwrap();
        assert_eq!(
            shared.borrow().requested,
            Some(DataWindowTimeRange::new(ArrivalTime(0), ArrivalTime(SECOND)).unwrap())
        );

        data.begin_seek(ArrivalTime(2 * SECOND)).unwrap();
        assert_eq!(shared.borrow().cancel_count, 1);
        assert!(shared.borrow().requested.is_none());
        assert!(!data.is_complete_through(ArrivalTime(2 * SECOND)));

        data.ensure_available_through(
            ArrivalTime(2 * SECOND),
            ArrivalTime(2 * SECOND),
            PlaybackSpeed::Normal,
            FetchIntent::RequiredOnly,
        )
        .unwrap();
        assert_eq!(
            shared.borrow().requested,
            Some(
                DataWindowTimeRange::new(ArrivalTime(2 * SECOND), ArrivalTime(3 * SECOND)).unwrap()
            )
        );
    }

    #[test]
    fn data_plane_uses_speed_scaled_target_ahead_with_one_loader_request() {
        let (mut data, shared) = data_plane(1024);
        for index in 0..3 {
            let start = index * SECOND;
            data.ensure_available_through(
                ArrivalTime(0),
                ArrivalTime(start),
                PlaybackSpeed::Normal,
                FetchIntent::PlaybackAhead,
            )
            .unwrap();
            complete_request(
                &mut data,
                &shared,
                loaded(start, start + SECOND, vec![], 0),
                ArrivalTime(0),
            );
        }

        data.ensure_available_through(
            ArrivalTime(0),
            ArrivalTime(0),
            PlaybackSpeed::Normal,
            FetchIntent::PlaybackAhead,
        )
        .unwrap();
        assert!(shared.borrow().requested.is_none());

        data.ensure_available_through(
            ArrivalTime(0),
            ArrivalTime(0),
            PlaybackSpeed::Double,
            FetchIntent::PlaybackAhead,
        )
        .unwrap();
        assert_eq!(
            shared.borrow().requested,
            Some(
                DataWindowTimeRange::new(ArrivalTime(3 * SECOND), ArrivalTime(4 * SECOND)).unwrap()
            )
        );
        let diagnostics = data.diagnostics(ArrivalTime(0), PlaybackSpeed::Double);
        assert_eq!(diagnostics.target_ahead, Duration::from_secs(4));
        assert_eq!(diagnostics.actual_buffer_ahead, Duration::from_secs(3));
    }
}
