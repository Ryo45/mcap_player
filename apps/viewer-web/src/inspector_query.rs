//! One independent Inspector request and one coalesced demand. No DOM or playback state.
use viewer_core::ArrivalTime;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InspectorRequest {
    pub topic: String,
    pub target: ArrivalTime,
    generation: u64,
}

pub(crate) struct InspectorQueryState {
    topic: Option<String>,
    generation: u64,
    active: Option<InspectorRequest>,
    desired: Option<ArrivalTime>,
    completed: Option<ArrivalTime>,
    automatic: bool,
    force: bool,
}
impl Default for InspectorQueryState {
    fn default() -> Self {
        Self {
            topic: None,
            generation: 0,
            active: None,
            desired: None,
            completed: None,
            automatic: true,
            force: false,
        }
    }
}
impl InspectorQueryState {
    pub fn select(&mut self, topic: Option<String>) {
        self.invalidate();
        self.topic = topic;
    }
    /// Source/topic replacement and successful seek only; a failed seek does not invalidate.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.active = None;
        self.desired = None;
        self.completed = None;
        self.force = false;
    }
    pub fn set_automatic(&mut self, automatic: bool) {
        self.automatic = automatic;
        if !automatic && !self.force {
            self.desired = None;
        }
    }
    pub fn refresh(&mut self, cursor: ArrivalTime) {
        self.force = true;
        self.desired = Some(cursor);
    }
    pub fn next(&mut self, cursor: ArrivalTime) -> Option<InspectorRequest> {
        if self.automatic {
            self.desired = Some(cursor);
        }
        let target = self.desired?;
        if self.active.is_some() || (!self.force && self.completed == Some(target)) {
            return None;
        }
        let request = InspectorRequest {
            topic: self.topic.clone()?,
            target,
            generation: self.generation,
        };
        self.active = Some(request.clone());
        self.force = false;
        self.desired = None;
        Some(request)
    }
    /// The loader generation must also reject stale completions before they reach this method.
    pub fn complete(&mut self, request: &InspectorRequest) -> bool {
        if self.active.as_ref() != Some(request) {
            return false;
        }
        self.active = None;
        self.completed = Some(request.target);
        true
    }
    pub fn active(&self) -> Option<&InspectorRequest> {
        self.active.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct FakeLoader {
        pending: Option<InspectorRequest>,
        starts: usize,
        published: usize,
    }
    impl FakeLoader {
        fn tick(&mut self, state: &mut InspectorQueryState, cursor: i64) {
            if let Some(request) = state.next(ArrivalTime(cursor)) {
                assert!(self.pending.is_none(), "never overlap requests");
                self.pending = Some(request);
                self.starts += 1;
            }
        }
        fn finish(&mut self, state: &mut InspectorQueryState) {
            if state.complete(&self.pending.take().unwrap()) {
                self.published += 1;
            }
        }
    }
    #[test]
    fn slow_reads_publish_and_coalesce_forward_ticks_without_starvation() {
        let mut state = InspectorQueryState::default();
        state.select(Some("/a".into()));
        let mut loader = FakeLoader::default();
        loader.tick(&mut state, 0);
        for time in 1..1000 {
            loader.tick(&mut state, time as i64);
        }
        assert_eq!(loader.starts, 1);
        assert_eq!(state.active().unwrap().target, ArrivalTime(0));
        loader.finish(&mut state);
        loader.tick(&mut state, 1000);
        assert_eq!(loader.pending.as_ref().unwrap().target, ArrivalTime(1000));
        loader.finish(&mut state);
        assert_eq!(loader.published, 2);
        for _ in 1001..3000 {
            loader.tick(&mut state, 1000);
        }
        assert_eq!(loader.starts, 2);
    }
    #[test]
    fn automatic_updates_have_no_fixed_delay_and_manual_refresh_is_explicit() {
        let mut state = InspectorQueryState::default();
        state.select(Some("/a".into()));
        let first = state.next(ArrivalTime(0)).unwrap();
        state.complete(&first);
        assert!(state.next(ArrivalTime(0)).is_none());
        let next = state.next(ArrivalTime(1)).unwrap();
        state.complete(&next);
        state.set_automatic(false);
        assert!(state.next(ArrivalTime(3)).is_none());
        state.refresh(ArrivalTime(3));
        let manual = state.next(ArrivalTime(3)).unwrap();
        state.refresh(ArrivalTime(4));
        assert!(state.next(ArrivalTime(4)).is_none());
        state.complete(&manual);
        let queued = state.next(ArrivalTime(4)).unwrap();
        state.complete(&queued);
        assert!(state.next(ArrivalTime(5)).is_none());
        state.set_automatic(true);
        assert!(state.next(ArrivalTime(5)).is_some());
    }
    #[test]
    fn source_topic_and_seek_invalidation_reject_old_successes_and_errors() {
        let mut state = InspectorQueryState::default();
        for topic in [Some("/a".into()), Some("/b".into()), None] {
            state.select(topic);
            let old = state.next(ArrivalTime(100));
            state.invalidate();
            if let Some(old) = old {
                assert!(!state.complete(&old));
            }
            if state.topic.is_some() {
                let current = state.next(ArrivalTime(0)).unwrap();
                assert!(state.complete(&current));
                assert!(state.next(ArrivalTime(0)).is_none());
            }
        }
    }
}
