use crate::session::SpeedSignalRequest;
use anyhow::{Context, Result};
use memmap2::Mmap;
use std::{
    fs::File,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, TryRecvError},
};
use viewer_core::{ArrivalTime, LoadedSignal, load_speed_signal};

pub(crate) struct PlotLoader {
    generation: u64,
    state: PlotLoadState,
    result_sender: Sender<PlotLoadResult>,
    result_receiver: Receiver<PlotLoadResult>,
}

enum PlotLoadState {
    Idle,
    Loading {
        generation: u64,
    },
    Ready {
        generation: u64,
        signal: Option<LoadedSignal>,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

struct PlotLoadResult {
    generation: u64,
    result: std::result::Result<Option<LoadedSignal>, String>,
}

impl Default for PlotLoader {
    fn default() -> Self {
        let (result_sender, result_receiver) = std::sync::mpsc::channel();
        Self {
            generation: 0,
            state: PlotLoadState::Idle,
            result_sender,
            result_receiver,
        }
    }
}

impl PlotLoader {
    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.state = PlotLoadState::Idle;
        self.discard_pending_results();
    }

    pub(crate) fn start_speed_overview(&mut self, request: SpeedSignalRequest) -> Result<()> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let sender = self.result_sender.clone();
        let worker = std::thread::Builder::new()
            .name("plot-loader".to_owned())
            .spawn(move || {
                let result =
                    load_speed_from_path(&request.path, request.origin, request.max_points)
                        .map_err(|error| error.to_string());
                let _ = sender.send(PlotLoadResult { generation, result });
            });
        match worker {
            Ok(_) => {
                self.state = PlotLoadState::Loading { generation };
                Ok(())
            }
            Err(error) => {
                let error = format!("start plot-loading worker: {error}");
                self.state = PlotLoadState::Failed {
                    generation,
                    error: error.clone(),
                };
                Err(anyhow::Error::msg(error))
            }
        }
    }

    pub(crate) fn poll(&mut self) {
        loop {
            match self.result_receiver.try_recv() {
                Ok(result) => self.apply_result(result),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    pub(crate) fn signal(&self) -> Option<&LoadedSignal> {
        match &self.state {
            PlotLoadState::Ready { generation, signal } if *generation == self.generation => {
                signal.as_ref()
            }
            _ => None,
        }
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(
            self.state,
            PlotLoadState::Loading { generation } if generation == self.generation
        )
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match &self.state {
            PlotLoadState::Failed { generation, error } if *generation == self.generation => {
                Some(error)
            }
            _ => None,
        }
    }

    fn apply_result(&mut self, result: PlotLoadResult) {
        if result.generation != self.generation {
            return;
        }
        match result.result {
            Ok(signal) => {
                self.state = PlotLoadState::Ready {
                    generation: result.generation,
                    signal,
                };
            }
            Err(error) => {
                self.state = PlotLoadState::Failed {
                    generation: result.generation,
                    error,
                };
            }
        }
    }

    fn discard_pending_results(&mut self) {
        while self.result_receiver.try_recv().is_ok() {}
    }
}

fn load_speed_from_path(
    path: &PathBuf,
    origin: ArrivalTime,
    max_points: usize,
) -> Result<Option<LoadedSignal>> {
    let file = File::open(path).with_context(|| format!("open {} for plot", path.display()))?;
    // SAFETY: this worker owns the read-only mapping for its entire scan.
    let mapping =
        unsafe { Mmap::map(&file) }.with_context(|| format!("map {} for plot", path.display()))?;
    load_speed_signal(&mapping, origin, max_points).map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::PlaybackSession;
    use mcap::{WriteOptions, Writer, records::MessageHeader};
    use std::{
        collections::BTreeMap,
        io::Cursor,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn camera_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_front_3s.mcap")
    }

    fn missing_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing-plot-fixture.mcap")
    }

    fn speed_fixture() -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mcap-player-plot-loader-{}-{sequence}.mcap",
            std::process::id()
        ));
        std::fs::write(&path, odometry_mcap()).unwrap();
        path
    }

    fn request(path: PathBuf, origin: ArrivalTime) -> SpeedSignalRequest {
        SpeedSignalRequest {
            path,
            origin,
            max_points: 4_000,
        }
    }

    fn align_cdr(output: &mut Vec<u8>, alignment: usize) {
        let relative = output.len() - 4;
        output.resize(
            output.len() + (alignment - relative % alignment) % alignment,
            0,
        );
    }

    fn push_u32(output: &mut Vec<u8>, value: u32) {
        align_cdr(output, 4);
        output.extend_from_slice(&value.to_le_bytes());
    }

    fn push_string(output: &mut Vec<u8>, value: &str) {
        push_u32(output, (value.len() + 1) as u32);
        output.extend_from_slice(value.as_bytes());
        output.push(0);
    }

    fn push_f64(output: &mut Vec<u8>, value: f64) {
        align_cdr(output, 8);
        output.extend_from_slice(&value.to_le_bytes());
    }

    fn odometry_cdr(measurement_ns: i64, speed: f64) -> Vec<u8> {
        let mut output = vec![0, 1, 0, 0];
        push_u32(&mut output, measurement_ns.div_euclid(1_000_000_000) as u32);
        push_u32(&mut output, measurement_ns.rem_euclid(1_000_000_000) as u32);
        push_string(&mut output, "odom");
        push_string(&mut output, "base_link");
        for value in [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
            push_f64(&mut output, value);
        }
        for _ in 0..36 {
            push_f64(&mut output, 0.0);
        }
        for value in [speed, 0.0, 0.0, 0.0, 0.0, 0.0] {
            push_f64(&mut output, value);
        }
        for _ in 0..36 {
            push_f64(&mut output, 0.0);
        }
        output
    }

    fn odometry_mcap() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer =
                Writer::with_options(&mut bytes, WriteOptions::new().use_chunks(false)).unwrap();
            let schema = writer
                .add_schema("nav_msgs/msg/Odometry", "ros2msg", b"")
                .unwrap();
            let channel = writer
                .add_channel(schema, "/odom", "cdr", &BTreeMap::new())
                .unwrap();
            for (sequence, speed) in [3.0, 5.0].into_iter().enumerate() {
                let arrival = 1_000_000_000 + sequence as u64 * 1_000_000_000;
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: sequence as u32,
                            log_time: arrival,
                            publish_time: arrival - 10,
                        },
                        &odometry_cdr(arrival as i64 - 10, speed),
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
    }

    impl PlotLoader {
        fn poll_until_settled_for_test(&mut self) {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while self.is_loading() && std::time::Instant::now() < deadline {
                self.poll();
                std::thread::yield_now();
            }
            assert!(!self.is_loading(), "plot worker did not settle");
        }

        fn hold_loading_for_test(&mut self) {
            self.state = PlotLoadState::Loading {
                generation: self.generation,
            };
        }
    }

    #[test]
    fn starts_idle_and_clear_returns_to_idle() {
        let mut loader = PlotLoader::default();
        assert!(!loader.is_loading());
        assert!(loader.signal().is_none());
        assert!(loader.error().is_none());
        loader.clear();
        assert!(!loader.is_loading());
        assert!(loader.signal().is_none());
        assert!(loader.error().is_none());
    }

    #[test]
    fn live_mode_clear_does_not_start_a_worker() {
        let mut loader = PlotLoader::default();
        loader
            .start_speed_overview(request(missing_fixture(), ArrivalTime(0)))
            .unwrap();
        assert!(loader.is_loading());

        loader.clear();

        assert!(!loader.is_loading());
        assert!(loader.signal().is_none());
        assert!(loader.error().is_none());
    }

    #[test]
    fn start_enters_loading_and_worker_failure_becomes_failed() {
        let mut loader = PlotLoader::default();
        loader
            .start_speed_overview(request(missing_fixture(), ArrivalTime(0)))
            .unwrap();
        assert!(loader.is_loading());
        loader.poll_until_settled_for_test();
        assert!(!loader.is_loading());
        assert!(loader.signal().is_none());
        assert!(loader.error().is_some_and(|error| error.contains("open")));
    }

    #[test]
    fn successful_worker_result_becomes_ready_even_without_speed_samples() {
        let mut loader = PlotLoader::default();
        loader
            .start_speed_overview(request(camera_fixture(), ArrivalTime(0)))
            .unwrap();
        assert!(loader.is_loading());
        loader.poll_until_settled_for_test();
        assert!(matches!(loader.state, PlotLoadState::Ready { .. }));
        assert!(!loader.is_loading());
        assert!(loader.signal().is_none());
        assert!(loader.error().is_none());
    }

    #[test]
    fn loads_speed_samples_on_the_worker() {
        let path = speed_fixture();
        let mut loader = PlotLoader::default();
        loader
            .start_speed_overview(request(path.clone(), ArrivalTime(1_000_000_000)))
            .unwrap();
        loader.poll_until_settled_for_test();
        let signal = loader.signal().expect("speed signal");
        assert_eq!(signal.samples.len(), 2);
        assert_eq!(signal.samples[0].value, 3.0);
        assert_eq!(signal.samples[1].value, 5.0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_generation_result_is_ignored() {
        let mut loader = PlotLoader::default();
        loader
            .start_speed_overview(request(missing_fixture(), ArrivalTime(0)))
            .unwrap();
        let current_generation = loader.generation;
        loader.apply_result(PlotLoadResult {
            generation: current_generation.wrapping_sub(1),
            result: Err("stale failure".to_owned()),
        });
        assert!(loader.is_loading());
        assert!(loader.error().is_none());
        loader.apply_result(PlotLoadResult {
            generation: current_generation,
            result: Err("current failure".to_owned()),
        });
        assert_eq!(loader.error(), Some("current failure"));
    }

    #[test]
    fn plot_failure_does_not_stop_playback_progress() {
        let mut session = PlaybackSession::open(
            &camera_fixture(),
            "/camera/front/image/compressed".to_owned(),
        )
        .unwrap();
        let start = session.playback_view().unwrap().cursor;
        let mut loader = PlotLoader::default();
        loader
            .start_speed_overview(request(missing_fixture(), start))
            .unwrap();
        session.tick(Duration::from_millis(250)).unwrap();
        loader.poll_until_settled_for_test();
        assert!(session.playback_view().unwrap().cursor > start);
        assert!(loader.error().is_some());
    }

    #[test]
    fn playback_progresses_while_plot_loading_is_pending() {
        let mut session = PlaybackSession::open(
            &camera_fixture(),
            "/camera/front/image/compressed".to_owned(),
        )
        .unwrap();
        let start = session.playback_view().unwrap().cursor;
        let mut loader = PlotLoader::default();
        loader.hold_loading_for_test();

        session.tick(Duration::from_millis(250)).unwrap();

        assert!(loader.is_loading());
        assert!(session.playback_view().unwrap().cursor > start);
    }
}
