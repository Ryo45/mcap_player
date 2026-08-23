use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageTiming {
    pub last_ms: f64,
    pub average_ms: f64,
    pub max_ms: f64,
    samples: u64,
}

impl StageTiming {
    pub fn record(&mut self, elapsed: Duration) {
        let milliseconds = elapsed.as_secs_f64() * 1_000.0;
        self.last_ms = milliseconds;
        self.max_ms = self.max_ms.max(milliseconds);
        self.samples = self.samples.saturating_add(1);
        self.average_ms += (milliseconds - self.average_ms) / self.samples as f64;
    }
}
