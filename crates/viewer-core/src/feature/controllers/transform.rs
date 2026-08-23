use super::ProcessingCounters;
use crate::{
    DYNAMIC_TF_HISTORY, RawMessage, RestoreSemantics, SessionPlan, StreamId, TransformBatch,
    TransformState, decode_tf_message,
};

#[derive(Clone)]
pub struct TransformController {
    dynamic_stream: Option<StreamId>,
    static_stream: Option<StreamId>,
    state: TransformState,
    counters: ProcessingCounters,
}

impl TransformController {
    pub const fn dynamic_restore_semantics() -> RestoreSemantics {
        RestoreSemantics::History(DYNAMIC_TF_HISTORY)
    }

    pub const fn static_restore_semantics() -> RestoreSemantics {
        RestoreSemantics::Persistent
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            dynamic_stream: plan.dynamic_tf_stream().map(|stream| stream.id),
            static_stream: plan.static_tf_stream().map(|stream| stream.id),
            state: TransformState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        let is_static = if Some(message.stream_id) == self.static_stream {
            true
        } else if Some(message.stream_id) == self.dynamic_stream {
            false
        } else {
            return false;
        };
        let _ = self.decode_and_apply(message, is_static);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        let is_static = if Some(message.stream_id) == self.static_stream {
            true
        } else if Some(message.stream_id) == self.dynamic_stream {
            false
        } else {
            return Ok(false);
        };
        self.decode_and_apply(message, is_static)?;
        Ok(true)
    }

    fn decode_and_apply(
        &mut self,
        message: &RawMessage,
        is_static: bool,
    ) -> Result<(), crate::DecodeError> {
        match decode_tf_message(&message.payload) {
            Ok(transforms) => {
                self.state.apply(TransformBatch {
                    arrival_time: message.arrival_time,
                    is_static,
                    transforms,
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

    pub fn reset_for_restore(&mut self, _target: crate::ArrivalTime) {
        self.state.clear();
    }

    pub fn state(&self) -> &TransformState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}
