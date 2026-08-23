use crate::CameraId;
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresentationSnapshot {
    pub camera_fps: BTreeMap<CameraId, f64>,
    pub total_camera_fps: f64,
    pub jpeg_decode_ms: f64,
    pub upload_ms: f64,
    pub render_ms: f64,
}

/// One-second rolling presentation metrics shared by native and web frontends.
#[derive(Clone, Debug, Default)]
pub struct PresentationMetrics {
    elapsed: Duration,
    camera_frames: BTreeMap<CameraId, u64>,
    jpeg_decode_time: Duration,
    upload_time: Duration,
    render_time: Duration,
    camera_samples: u64,
    render_samples: u64,
    snapshot: PresentationSnapshot,
}

impl PresentationMetrics {
    pub fn record_camera(
        &mut self,
        camera_id: CameraId,
        jpeg_decode_time: Duration,
        upload_time: Duration,
    ) {
        *self.camera_frames.entry(camera_id).or_default() += 1;
        self.jpeg_decode_time = self.jpeg_decode_time.saturating_add(jpeg_decode_time);
        self.upload_time = self.upload_time.saturating_add(upload_time);
        self.camera_samples = self.camera_samples.saturating_add(1);
    }

    pub fn record_render(&mut self, render_time: Duration) {
        self.render_time = self.render_time.saturating_add(render_time);
        self.render_samples = self.render_samples.saturating_add(1);
    }

    pub fn advance(&mut self, elapsed: Duration) {
        self.elapsed = self.elapsed.saturating_add(elapsed);
        if self.elapsed < Duration::from_secs(1) {
            return;
        }

        let seconds = self.elapsed.as_secs_f64();
        self.snapshot.camera_fps = self
            .camera_frames
            .iter()
            .map(|(camera_id, frames)| (*camera_id, *frames as f64 / seconds))
            .collect();
        self.snapshot.total_camera_fps = self.camera_frames.values().sum::<u64>() as f64 / seconds;
        self.snapshot.jpeg_decode_ms =
            average_milliseconds(self.jpeg_decode_time, self.camera_samples);
        self.snapshot.upload_ms = average_milliseconds(self.upload_time, self.camera_samples);
        self.snapshot.render_ms = average_milliseconds(self.render_time, self.render_samples);

        self.elapsed = Duration::ZERO;
        self.camera_frames.clear();
        self.jpeg_decode_time = Duration::ZERO;
        self.upload_time = Duration::ZERO;
        self.render_time = Duration::ZERO;
        self.camera_samples = 0;
        self.render_samples = 0;
    }

    pub fn snapshot(&self) -> &PresentationSnapshot {
        &self.snapshot
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn average_milliseconds(duration: Duration, samples: u64) -> f64 {
    if samples == 0 {
        0.0
    } else {
        duration.as_secs_f64() * 1_000.0 / samples as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_per_camera_rates_and_stage_averages() {
        let mut metrics = PresentationMetrics::default();
        for _ in 0..10 {
            metrics.record_camera(
                CameraId(0),
                Duration::from_millis(2),
                Duration::from_millis(1),
            );
        }
        for _ in 0..5 {
            metrics.record_camera(
                CameraId(1),
                Duration::from_millis(4),
                Duration::from_millis(2),
            );
        }
        metrics.record_render(Duration::from_millis(8));
        metrics.advance(Duration::from_secs(1));

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.camera_fps.get(&CameraId(0)), Some(&10.0));
        assert_eq!(snapshot.camera_fps.get(&CameraId(1)), Some(&5.0));
        assert_eq!(snapshot.total_camera_fps, 15.0);
        assert!((snapshot.jpeg_decode_ms - 2.666_666).abs() < 0.001);
        assert!((snapshot.upload_ms - 1.333_333).abs() < 0.001);
        assert_eq!(snapshot.render_ms, 8.0);
    }
}
