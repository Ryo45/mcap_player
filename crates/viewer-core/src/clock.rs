use crate::ArrivalTime;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackSpeed {
    Quarter,
    Half,
    Normal,
    Double,
}

impl PlaybackSpeed {
    pub const ALL: [Self; 4] = [Self::Quarter, Self::Half, Self::Normal, Self::Double];

    pub const fn factor(self) -> f64 {
        match self {
            Self::Quarter => 0.25,
            Self::Half => 0.5,
            Self::Normal => 1.0,
            Self::Double => 2.0,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Quarter => "0.25x",
            Self::Half => "0.5x",
            Self::Normal => "1x",
            Self::Double => "2x",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackView {
    pub start: ArrivalTime,
    pub end: ArrivalTime,
    pub cursor: ArrivalTime,
    pub playing: bool,
    pub speed: PlaybackSpeed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaybackCommand {
    Toggle,
    SetSpeed(PlaybackSpeed),
    Seek(ArrivalTime),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackLoadState {
    Ready,
    Buffering {
        requested: ArrivalTime,
        committed: ArrivalTime,
    },
    Seeking {
        target: ArrivalTime,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeekFidelity {
    Preview,
    ExactVisible,
    ExactRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeekRequest {
    pub target: ArrivalTime,
    pub fidelity: SeekFidelity,
    pub required_streams: Vec<crate::StreamId>,
}

impl SeekRequest {
    pub fn exact_visible(target: ArrivalTime) -> Self {
        Self {
            target,
            fidelity: SeekFidelity::ExactVisible,
            required_streams: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackClock {
    start: ArrivalTime,
    end: ArrivalTime,
    cursor: ArrivalTime,
    playing: bool,
    speed: PlaybackSpeed,
}

impl PlaybackClock {
    pub fn new(start: ArrivalTime, end: ArrivalTime) -> Self {
        let end = end.max(start);
        Self {
            start,
            end,
            cursor: start,
            playing: false,
            speed: PlaybackSpeed::Normal,
        }
    }

    pub fn cursor(&self) -> ArrivalTime {
        self.cursor
    }
    pub fn start(&self) -> ArrivalTime {
        self.start
    }
    pub fn end(&self) -> ArrivalTime {
        self.end
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    pub fn speed(&self) -> PlaybackSpeed {
        self.speed
    }
    pub fn view(&self) -> PlaybackView {
        PlaybackView {
            start: self.start,
            end: self.end,
            cursor: self.cursor,
            playing: self.playing,
            speed: self.speed,
        }
    }
    pub fn set_speed(&mut self, speed: PlaybackSpeed) {
        self.speed = speed;
    }
    pub fn play(&mut self) {
        self.playing = true;
    }
    pub fn pause(&mut self) {
        self.playing = false;
    }
    pub fn toggle(&mut self) {
        self.playing = !self.playing;
    }

    pub fn seek(&mut self, cursor: ArrivalTime) {
        self.cursor = cursor.clamp(self.start, self.end);
    }

    pub fn cursor_after(&self, elapsed: Duration) -> ArrivalTime {
        if !self.playing {
            return self.cursor;
        }
        let delta = (elapsed.as_nanos() as f64 * self.speed.factor()) as i64;
        ArrivalTime(self.cursor.0.saturating_add(delta)).min(self.end)
    }

    pub fn commit_cursor(&mut self, cursor: ArrivalTime) {
        self.cursor = cursor.clamp(self.start, self.end);
        if self.cursor == self.end {
            self.playing = false;
        }
    }

    pub fn advance(&mut self, elapsed: Duration) -> ArrivalTime {
        let cursor = self.cursor_after(elapsed);
        self.commit_cursor(cursor);
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_pause_speed_and_bounds() {
        let mut clock = PlaybackClock::new(ArrivalTime(100), ArrivalTime(1_000_000_100));
        assert_eq!(clock.advance(Duration::from_secs(1)), ArrivalTime(100));
        clock.set_speed(PlaybackSpeed::Half);
        clock.play();
        assert_eq!(
            clock.advance(Duration::from_millis(500)),
            ArrivalTime(250_000_100)
        );
        assert_eq!(
            clock.advance(Duration::from_secs(4)),
            ArrivalTime(1_000_000_100)
        );
        assert!(!clock.is_playing());
        clock.seek(ArrivalTime(-1));
        assert_eq!(clock.cursor(), ArrivalTime(100));
        assert_eq!(
            clock.view(),
            PlaybackView {
                start: ArrivalTime(100),
                end: ArrivalTime(1_000_000_100),
                cursor: ArrivalTime(100),
                playing: false,
                speed: PlaybackSpeed::Half,
            }
        );
    }

    #[test]
    fn candidate_cursor_is_non_mutating_until_committed() {
        let mut clock = PlaybackClock::new(ArrivalTime(100), ArrivalTime(1_000_000_100));
        clock.play();
        let candidate = clock.cursor_after(Duration::from_millis(250));
        assert_eq!(candidate, ArrivalTime(250_000_100));
        assert_eq!(clock.cursor(), ArrivalTime(100));
        assert!(clock.is_playing());

        clock.commit_cursor(candidate);
        assert_eq!(clock.cursor(), candidate);
        assert!(clock.is_playing());

        let end = clock.cursor_after(Duration::from_secs(10));
        clock.commit_cursor(end);
        assert_eq!(clock.cursor(), clock.end());
        assert!(!clock.is_playing());
    }
}
