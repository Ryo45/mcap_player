use crate::ArrivalTime;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
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

    pub fn advance(&mut self, elapsed: Duration) -> ArrivalTime {
        if !self.playing {
            return self.cursor;
        }
        let delta = (elapsed.as_nanos() as f64 * self.speed.factor()) as i64;
        self.cursor = ArrivalTime(self.cursor.0.saturating_add(delta)).min(self.end);
        if self.cursor == self.end {
            self.playing = false;
        }
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
    }
}
