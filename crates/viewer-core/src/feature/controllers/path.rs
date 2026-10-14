use super::ProcessingCounters;
use crate::{
    PathFrame, PathState, RawMessage, RestoreSemantics, SessionPlan, StreamId, decode_path,
};

#[derive(Clone)]
pub struct PathController {
    stream: Option<StreamId>,
    state: PathState,
    counters: ProcessingCounters,
}

impl PathController {
    pub const fn restore_semantics() -> RestoreSemantics {
        RestoreSemantics::LatestBefore
    }

    pub fn new(plan: &SessionPlan) -> Self {
        Self {
            stream: plan.path_stream().map(|stream| stream.id),
            state: PathState::default(),
            counters: ProcessingCounters::default(),
        }
    }

    pub fn process(&mut self, message: &RawMessage) -> bool {
        if Some(message.stream_id) != self.stream {
            return false;
        }
        let _ = self.decode_and_apply(message);
        true
    }

    pub fn restore(&mut self, message: &RawMessage) -> Result<bool, crate::DecodeError> {
        if Some(message.stream_id) != self.stream {
            return Ok(false);
        }
        self.decode_and_apply(message)?;
        Ok(true)
    }

    fn decode_and_apply(&mut self, message: &RawMessage) -> Result<(), crate::DecodeError> {
        match decode_path(&message.payload) {
            Ok(path) => {
                self.state.apply(PathFrame {
                    measurement_time: path.measurement_time,
                    arrival_time: message.arrival_time,
                    frame_id: path.frame_id,
                    points: path.points,
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

    pub fn reset_for_restore(&mut self) {
        self.state.cold_seek();
    }

    pub fn state(&self) -> &PathState {
        &self.state
    }

    pub fn counters(&self) -> ProcessingCounters {
        self.counters
    }
}
