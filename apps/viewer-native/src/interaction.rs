#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreviewDragState {
    playing_before_drag: Option<bool>,
}

impl PreviewDragState {
    pub(crate) fn begin(&mut self, playing: bool) -> bool {
        if self.playing_before_drag.is_some() {
            return false;
        }
        self.playing_before_drag = Some(playing);
        playing
    }

    pub(crate) fn finish(&mut self) -> bool {
        self.playing_before_drag.take().unwrap_or(false)
    }

    pub(crate) fn clear(&mut self) {
        self.playing_before_drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_pause_and_resume_intent_is_temporary() {
        let mut state = PreviewDragState::default();
        assert!(state.begin(true));
        assert!(!state.begin(true));
        assert!(state.finish());
        assert!(!state.finish());
        assert!(!state.begin(false));
        assert!(!state.finish());
    }
}
