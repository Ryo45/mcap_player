//! A bounded, independent Recording query. No samples enter continuous FeatureRuntime.
use crate::{
    data_plane::{WebWindowLoader, WindowLoader},
    playback::WebPlayback,
};
use viewer_core::{
    ArrivalTime, DataWindowTimeRange, LoadedOdometrySignals, SignalId, SignalOverviewReducer,
    SignalSample, decode_odometry,
};

pub(super) struct OdometryQuery {
    loader: WebWindowLoader,
    next: ArrivalTime,
    end: ArrivalTime,
    reducer: SignalOverviewReducer,
    yaw_reducer: SignalOverviewReducer,
    pub result: LoadedOdometrySignals,
    pub error: Option<String>,
    pub loading: bool,
    messages: usize,
    streams: std::collections::BTreeSet<viewer_core::StreamId>,
}
impl OdometryQuery {
    pub fn new(session: &WebPlayback, topic: &str) -> Result<Self, String> {
        if !session
            .catalog()
            .streams
            .iter()
            .any(|s| s.topic == topic && s.schema == "nav_msgs/msg/Odometry")
        {
            return Err(format!("No Odometry stream at {topic}"));
        }
        let start = session.clock().view().start;
        // Initial browser query is explicit and bounded to the first minute.
        let end = ArrivalTime(
            session
                .clock()
                .view()
                .end
                .0
                .saturating_add(1)
                .min(start.0.saturating_add(60_000_000_000)),
        );
        Ok(Self {
            loader: session.signal_query_loader(topic),
            next: start,
            end,
            reducer: SignalOverviewReducer::new(start, end, 2048),
            yaw_reducer: SignalOverviewReducer::new(start, end, 2048),
            result: LoadedOdometrySignals::default(),
            error: None,
            loading: true,
            messages: 0,
            streams: session
                .catalog()
                .streams
                .iter()
                .filter(|s| s.topic == topic && s.schema == "nav_msgs/msg/Odometry")
                .map(|s| s.id)
                .collect(),
        })
    }
    pub fn request_count(&self) -> u64 {
        self.loader.metrics().load_requests
    }
    pub fn poll(&mut self) {
        if !self.loading {
            return;
        }
        if let Some(batch) = self.loader.poll() {
            match batch {
                Ok(batch) => {
                    if self.loader.metrics().source_bytes > 256 * 1024 * 1024
                        || self.loader.metrics().decompressed_bytes > 512 * 1024 * 1024
                    {
                        self.fail("Plot exceeds serialized/decompression work budget".into());
                        return;
                    }
                    self.next = batch.window.range.end_exclusive;
                    for message in &batch.window.messages {
                        if !self.streams.contains(&message.stream_id) {
                            continue;
                        }
                        self.messages += 1;
                        if self.messages > 100_000 {
                            self.fail("Plot exceeds 100,000 input messages".into());
                            return;
                        }
                        let odometry = match decode_odometry(&message.payload) {
                            Ok(o) => o,
                            Err(e) => {
                                self.fail(format!("Odometry decode: {e}"));
                                return;
                            }
                        };
                        self.yaw_reducer.push(SignalSample {
                            measurement_time: Some(odometry.measurement_time),
                            arrival_time: message.arrival_time,
                            value: odometry.angular_velocity[2],
                        });
                        self.reducer.push(SignalSample {
                            measurement_time: Some(odometry.measurement_time),
                            arrival_time: message.arrival_time,
                            value: odometry.linear_velocity[0].hypot(odometry.linear_velocity[1]),
                        });
                    }
                    // Do not publish new allocations on every playback tick; this is one query result.
                    if self.next >= self.end {
                        self.result = LoadedOdometrySignals {
                            speed: self.reducer.finish(SignalId::Speed),
                            yaw_rate: self.yaw_reducer.finish(SignalId::YawRate),
                        };
                        self.loading = false;
                    }
                }
                Err(error) => self.fail(error.to_string()),
            }
        }
        if self.loading && self.loader.is_idle() {
            let end = match self.loader {
                WebWindowLoader::Remote(_) => {
                    ArrivalTime(self.next.0.saturating_add(1_000_000_000).min(self.end.0))
                }
                WebWindowLoader::LocalFile(_) => self.end,
            };
            let range = DataWindowTimeRange {
                start: self.next,
                end_exclusive: end,
            };
            if let Err(error) = self.loader.request(range) {
                self.fail(error.to_string());
            }
        }
    }
    fn fail(&mut self, error: String) {
        self.loader.cancel();
        self.result = LoadedOdometrySignals::default();
        self.error = Some(error);
        self.loading = false;
    }
}
impl Drop for OdometryQuery {
    fn drop(&mut self) {
        self.loader.cancel();
    }
}
