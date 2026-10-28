use crate::{ArrivalTime, Detection3DArray};

#[derive(Clone, Debug, PartialEq)]
pub struct Detection3DFrame {
    pub arrival_time: ArrivalTime,
    pub message: Detection3DArray,
}

#[derive(Clone, Debug, Default)]
pub struct Detection3DState {
    revision: u64,
    latest: Option<Detection3DFrame>,
}
impl Detection3DState {
    pub fn latest(&self) -> Option<&Detection3DFrame> {
        self.latest.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn apply(&mut self, frame: Detection3DFrame) {
        if self
            .latest
            .as_ref()
            .is_some_and(|old| old.arrival_time > frame.arrival_time)
        {
            return;
        }
        self.latest = Some(frame);
        self.revision = self.revision.wrapping_add(1);
    }
    pub(crate) fn restore_candidate(&self) -> Self {
        Self {
            revision: self.revision.wrapping_add(1),
            latest: None,
        }
    }
}
