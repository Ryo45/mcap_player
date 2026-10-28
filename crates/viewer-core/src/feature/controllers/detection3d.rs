use super::{ProcessingCounters, latest::latest_in_batch};
use crate::{
    DecodeError, Detection3DFrame, Detection3DState, RawMessage, StreamId, decode_detection3d_array,
};

#[derive(Clone)]
pub struct Detection3DController {
    stream: StreamId,
    state: Detection3DState,
    counters: ProcessingCounters,
}
impl Detection3DController {
    pub(crate) fn for_stream(stream: StreamId) -> Self {
        Self {
            stream,
            state: Detection3DState::default(),
            counters: ProcessingCounters::default(),
        }
    }
    pub(crate) fn accepts(&self, stream: StreamId) -> bool {
        self.stream == stream
    }
    pub(crate) fn process_latest(&mut self, messages: &[RawMessage]) {
        if let Some((message, dropped)) = latest_in_batch(messages, self.stream) {
            self.counters.dropped = self.counters.dropped.saturating_add(dropped);
            let _ = self.decode_and_apply(message);
        }
    }
    pub(crate) fn restore(&mut self, message: &RawMessage) -> Result<bool, DecodeError> {
        if !self.accepts(message.stream_id) {
            return Ok(false);
        }
        self.decode_and_apply(message)?;
        Ok(true)
    }
    fn decode_and_apply(&mut self, message: &RawMessage) -> Result<(), DecodeError> {
        match decode_detection3d_array(&message.payload) {
            Ok(data) => {
                self.state.apply(Detection3DFrame {
                    arrival_time: message.arrival_time,
                    message: data,
                });
                self.counters.decoded = self.counters.decoded.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                self.counters.errors = self.counters.errors.saturating_add(1);
                Err(error)
            }
        }
    }
    pub(crate) fn restore_candidate(&self) -> Self {
        Self {
            stream: self.stream,
            state: self.state.restore_candidate(),
            counters: self.counters,
        }
    }
    pub fn state(&self) -> &Detection3DState {
        &self.state
    }
    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}
