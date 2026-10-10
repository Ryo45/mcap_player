use super::catalog::{LiveDeliveryClass, LiveStreamRole, LiveStreamSpec};
use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    fmt,
    sync::Mutex,
};
use viewer_core::{ArrivalTime, RawMessage, StreamId};

pub(crate) const DEFAULT_ORDERED_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LiveDiagnostics {
    pub(crate) received_messages: u64,
    pub(crate) received_serialized_bytes: u64,
    pub(crate) payload_copy_bytes: u64,
    pub(crate) adapter_errors: u64,
    pub(crate) coalesced_messages: u64,
    pub(crate) coalesced_payload_bytes: u64,
    pub(crate) ordered_queue_overflows: u64,
    pub(crate) queue_high_water_mark: usize,
    pub(crate) resident_messages: usize,
    pub(crate) latest_arrival: Option<ArrivalTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveAdmissionError(String);

impl LiveAdmissionError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LiveAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LiveAdmissionError {}

pub(crate) struct LiveAdmission {
    state: Mutex<AdmissionState>,
}

struct AdmissionState {
    streams: BTreeMap<StreamId, StreamQueue>,
    ordered_capacity: usize,
    payload_copy_bytes: u64,
    adapter_errors: u64,
    queue_high_water_mark: usize,
    latest_arrival: Option<ArrivalTime>,
    error: Option<LiveAdmissionError>,
}

struct StreamQueue {
    spec: LiveStreamSpec,
    pending: PendingMessages,
    received_messages: u64,
    received_serialized_bytes: u64,
    coalesced_messages: u64,
    coalesced_payload_bytes: u64,
    ordered_queue_overflows: u64,
}

enum PendingMessages {
    Latest(Option<RawMessage>),
    Ordered(VecDeque<RawMessage>),
}

impl LiveAdmission {
    pub(crate) fn new(
        specs: Vec<LiveStreamSpec>,
        ordered_capacity: usize,
    ) -> Result<Self, LiveAdmissionError> {
        if ordered_capacity == 0 {
            return Err(LiveAdmissionError::new(
                "ordered Live queue capacity must be positive",
            ));
        }
        let mut streams = BTreeMap::new();
        for spec in specs {
            let stream_id = spec.stream.id;
            let pending = match spec.delivery {
                LiveDeliveryClass::Latest => PendingMessages::Latest(None),
                LiveDeliveryClass::Ordered => PendingMessages::Ordered(VecDeque::new()),
            };
            if streams
                .insert(
                    stream_id,
                    StreamQueue {
                        spec,
                        pending,
                        received_messages: 0,
                        received_serialized_bytes: 0,
                        coalesced_messages: 0,
                        coalesced_payload_bytes: 0,
                        ordered_queue_overflows: 0,
                    },
                )
                .is_some()
            {
                return Err(LiveAdmissionError::new(format!(
                    "duplicate Live stream id {}",
                    stream_id.0
                )));
            }
        }
        Ok(Self {
            state: Mutex::new(AdmissionState {
                streams,
                ordered_capacity,
                payload_copy_bytes: 0,
                adapter_errors: 0,
                queue_high_water_mark: 0,
                latest_arrival: None,
                error: None,
            }),
        })
    }

    /// Admits one serialized message before any feature decode.
    pub(crate) fn admit(
        &self,
        message: RawMessage,
        payload_copy_bytes: u64,
    ) -> Result<(), LiveAdmissionError> {
        let mut state = self.state.lock().expect("Live admission state poisoned");
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        state.payload_copy_bytes = state.payload_copy_bytes.saturating_add(payload_copy_bytes);
        let ordered_capacity = state.ordered_capacity;
        let arrival = message.arrival_time;
        let Some(stream) = state.streams.get_mut(&message.stream_id) else {
            let error = LiveAdmissionError::new(format!(
                "message arrived for unselected Live stream {}",
                message.stream_id.0
            ));
            state.error = Some(error.clone());
            return Err(error);
        };
        stream.received_messages = stream.received_messages.saturating_add(1);
        stream.received_serialized_bytes = stream
            .received_serialized_bytes
            .saturating_add(message.payload.len() as u64);

        match &mut stream.pending {
            PendingMessages::Latest(slot) => {
                if let Some(previous) = slot.replace(message) {
                    stream.coalesced_messages = stream.coalesced_messages.saturating_add(1);
                    stream.coalesced_payload_bytes = stream
                        .coalesced_payload_bytes
                        .saturating_add(previous.payload.len() as u64);
                }
            }
            PendingMessages::Ordered(queue) => {
                if queue.len() == ordered_capacity {
                    stream.ordered_queue_overflows =
                        stream.ordered_queue_overflows.saturating_add(1);
                    if stream.spec.role == LiveStreamRole::StaticTf {
                        let error = LiveAdmissionError::new(format!(
                            "static TF Live queue overflow on {} (capacity {ordered_capacity})",
                            stream.spec.stream.topic
                        ));
                        state.error = Some(error.clone());
                        return Err(error);
                    }
                    queue.pop_front();
                }
                queue.push_back(message);
            }
        }
        state.latest_arrival = Some(arrival);
        let resident = state.streams.values().map(StreamQueue::resident_len).sum();
        state.queue_high_water_mark = state.queue_high_water_mark.max(resident);
        Ok(())
    }

    pub(crate) fn take_ready(&self) -> Vec<RawMessage> {
        let mut state = self.state.lock().expect("Live admission state poisoned");
        let mut ready = Vec::new();
        for stream in state.streams.values_mut() {
            match &mut stream.pending {
                PendingMessages::Latest(slot) => ready.extend(slot.take()),
                PendingMessages::Ordered(queue) => ready.extend(queue.drain(..)),
            }
        }
        ready.sort_by_key(|message| (message.arrival_time, message.stream_id));
        ready
    }

    pub(crate) fn diagnostics(&self) -> LiveDiagnostics {
        let state = self.state.lock().expect("Live admission state poisoned");
        LiveDiagnostics {
            received_messages: state
                .streams
                .values()
                .map(|stream| stream.received_messages)
                .sum(),
            received_serialized_bytes: state
                .streams
                .values()
                .map(|stream| stream.received_serialized_bytes)
                .sum(),
            payload_copy_bytes: state.payload_copy_bytes,
            adapter_errors: state.adapter_errors,
            coalesced_messages: state
                .streams
                .values()
                .map(|stream| stream.coalesced_messages)
                .sum(),
            coalesced_payload_bytes: state
                .streams
                .values()
                .map(|stream| stream.coalesced_payload_bytes)
                .sum(),
            ordered_queue_overflows: state
                .streams
                .values()
                .map(|stream| stream.ordered_queue_overflows)
                .sum(),
            queue_high_water_mark: state.queue_high_water_mark,
            resident_messages: state.streams.values().map(StreamQueue::resident_len).sum(),
            latest_arrival: state.latest_arrival,
        }
    }

    pub(crate) fn error(&self) -> Option<LiveAdmissionError> {
        self.state
            .lock()
            .expect("Live admission state poisoned")
            .error
            .clone()
    }

    #[cfg(feature = "ros2-live")]
    pub(crate) fn set_source_error(&self, error: impl Into<String>) {
        self.state
            .lock()
            .expect("Live admission state poisoned")
            .error = Some(LiveAdmissionError::new(error));
    }

    #[cfg(feature = "ros2-live")]
    pub(crate) fn record_adapter_error(&self) {
        let mut state = self.state.lock().expect("Live admission state poisoned");
        state.adapter_errors = state.adapter_errors.saturating_add(1);
    }
}

impl StreamQueue {
    fn resident_len(&self) -> usize {
        self.pending.len()
    }
}

impl PendingMessages {
    fn len(&self) -> usize {
        match self {
            Self::Latest(message) => usize::from(message.is_some()),
            Self::Ordered(messages) => messages.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{StreamDescriptor, StreamTimingSummary};

    fn spec(id: u32, role: LiveStreamRole) -> LiveStreamSpec {
        LiveStreamSpec::new(
            &StreamDescriptor {
                schema_definition: None,
                id: StreamId(id),
                topic: format!("/stream/{id}"),
                schema: role.schema().into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            },
            role,
        )
    }

    fn message(stream_id: u32, arrival: i64, bytes: usize) -> RawMessage {
        RawMessage {
            publish_time_ns: None,
            sequence: None,
            stream_id: StreamId(stream_id),
            arrival_time: ArrivalTime(arrival),
            payload: vec![stream_id as u8; bytes].into(),
        }
    }

    #[test]
    fn camera_latest_coalesces_before_decode() {
        let admission = LiveAdmission::new(vec![spec(1, LiveStreamRole::Camera)], 4).unwrap();
        admission.admit(message(1, 10, 3), 3).unwrap();
        admission.admit(message(1, 20, 5), 5).unwrap();

        let ready = admission.take_ready();
        assert_eq!(ready[0].arrival_time, ArrivalTime(20));
        let diagnostics = admission.diagnostics();
        assert_eq!(diagnostics.received_messages, 2);
        assert_eq!(diagnostics.coalesced_messages, 1);
        assert_eq!(diagnostics.coalesced_payload_bytes, 3);
        assert_eq!(diagnostics.payload_copy_bytes, 8);
        assert_eq!(diagnostics.latest_arrival, Some(ArrivalTime(20)));
        assert_eq!(diagnostics.resident_messages, 0);
    }

    #[test]
    fn semantic_and_tf_streams_preserve_order_with_a_fixed_bound() {
        let admission = LiveAdmission::new(
            vec![
                spec(2, LiveStreamRole::Path),
                spec(3, LiveStreamRole::DynamicTf),
            ],
            3,
        )
        .unwrap();
        for arrival in 1..=5 {
            admission.admit(message(2, arrival, 1), 0).unwrap();
        }
        admission.admit(message(3, 8, 1), 0).unwrap();
        admission.admit(message(3, 7, 1), 0).unwrap();
        assert_eq!(
            admission
                .take_ready()
                .iter()
                .map(|message| (message.stream_id.0, message.arrival_time.0))
                .collect::<Vec<_>>(),
            vec![(2, 3), (2, 4), (2, 5), (3, 7), (3, 8)]
        );
        assert_eq!(admission.diagnostics().ordered_queue_overflows, 2);
        assert_eq!(admission.diagnostics().queue_high_water_mark, 5);
    }

    #[test]
    fn static_tf_overflow_is_terminal_and_does_not_replace_pending_state() {
        let admission = LiveAdmission::new(vec![spec(4, LiveStreamRole::StaticTf)], 1).unwrap();
        admission.admit(message(4, 1, 1), 0).unwrap();
        let error = admission.admit(message(4, 2, 1), 0).unwrap_err();
        assert!(error.to_string().contains("static TF"));
        assert_eq!(admission.error(), Some(error));
        assert_eq!(admission.take_ready()[0].arrival_time, ArrivalTime(1));
    }

    #[test]
    fn resident_backlog_never_grows_with_input_duration() {
        let admission = LiveAdmission::new(
            vec![
                spec(1, LiveStreamRole::Camera),
                spec(2, LiveStreamRole::Odometry),
            ],
            DEFAULT_ORDERED_QUEUE_CAPACITY,
        )
        .unwrap();
        for arrival in 0..10_000 {
            admission.admit(message(1, arrival, 1024), 0).unwrap();
            admission.admit(message(2, arrival, 128), 0).unwrap();
            assert!(
                admission.diagnostics().resident_messages <= DEFAULT_ORDERED_QUEUE_CAPACITY + 1
            );
        }
        let diagnostics = admission.diagnostics();
        assert_eq!(diagnostics.received_messages, 20_000);
        assert_eq!(diagnostics.queue_high_water_mark, 257);
    }
}
