//! Transactional ownership boundary for continuous feature state.

use crate::{
    ArrivalTime, CameraController, CameraId, OdometryController, PathController,
    PlaybackPerformance, PointCloudController, ProcessingCounters, RawMessage, SessionPlan,
    StageTiming, StreamId, TransformController,
};
use std::{error::Error, fmt, time::Duration};
use web_time::Instant;

/// Shared continuous materialization boundary: route/admit serialized inputs, coalesce where
/// allowed, decode accepted inputs once, then let consumers borrow concrete feature state.
///
/// Camera stores shared compressed-image materialization here; platform CameraBaseImageTracker
/// shares JPEG pixel decode across views while renderers retain GPU ownership. TF history and
/// point-cloud materialization likewise belong to this runtime rather than individual panels.
///
/// Plot, Preview and Exact Range Query deliberately remain outside this type. Platform-specific
/// source I/O, clocks and buffering also remain with the application adapters.
#[derive(Clone)]
pub struct FeatureRuntime {
    cameras: CameraController,
    path: PathController,
    odometry: OdometryController,
    transforms: TransformController,
    point_cloud: Option<PointCloudController>,
    unknown_streams: u64,
    processing_time: StageTiming,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureRestoreErrorKind {
    Decode {
        feature: &'static str,
        source: crate::DecodeError,
    },
    Unrouted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureRestoreError {
    pub stream_id: StreamId,
    pub arrival_time: ArrivalTime,
    pub kind: FeatureRestoreErrorKind,
}

impl fmt::Display for FeatureRestoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            FeatureRestoreErrorKind::Decode { feature, source } => write!(
                formatter,
                "restore decode failed for {feature} stream {} at {} ns: {source}",
                self.stream_id.0, self.arrival_time.0
            ),
            FeatureRestoreErrorKind::Unrouted => write!(
                formatter,
                "restore message for stream {} at {} ns did not route to a continuous feature",
                self.stream_id.0, self.arrival_time.0
            ),
        }
    }
}

impl Error for FeatureRestoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            FeatureRestoreErrorKind::Decode { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl FeatureRuntime {
    pub fn new(plan: &SessionPlan, enable_point_cloud: bool) -> Self {
        Self {
            cameras: CameraController::new(plan),
            path: PathController::new(plan),
            odometry: OdometryController::new(plan),
            transforms: TransformController::new(plan),
            point_cloud: enable_point_cloud.then(|| PointCloudController::new(plan)),
            unknown_streams: 0,
            processing_time: StageTiming::default(),
        }
    }

    /// Reduces forward messages using the existing tolerant playback semantics.
    ///
    /// A routed malformed message increments that controller's error counter and leaves its prior
    /// state visible. Camera admission remains serialized and coalesces before decode.
    pub fn process_messages(&mut self, elapsed: Duration, messages: &[RawMessage]) {
        let started = Instant::now();
        for message in messages {
            // Dispatch to every controller, even after one accepts the message.
            let camera_matched = self.cameras.admit(message);
            let path_matched = self.path.process(message);
            let odometry_matched = self.odometry.process(message);
            let transform_matched = self.transforms.process(message);
            let point_cloud_matched = if let Some(point_cloud) = &mut self.point_cloud {
                point_cloud.process(message)
            } else {
                false
            };
            let matched = camera_matched
                || path_matched
                || odometry_matched
                || transform_matched
                || point_cloud_matched;
            if !matched {
                self.unknown_streams = self.unknown_streams.saturating_add(1);
            }
        }
        self.cameras.advance(elapsed);
        self.processing_time.record(started.elapsed());
    }

    /// Builds a complete seek candidate without mutating the authoritative runtime.
    /// Messages must already be selected for the target by the source's restore planner.
    pub fn stage_restore(
        &self,
        _target: ArrivalTime,
        messages: &[RawMessage],
    ) -> Result<Self, FeatureRestoreError> {
        let started = Instant::now();
        // Retain routing and cumulative statistics, but never clone decoded frames or history
        // that the restore immediately discards. The authoritative state stays untouched.
        let mut candidate = Self {
            cameras: self.cameras.restore_candidate(),
            path: self.path.restore_candidate(),
            odometry: self.odometry.restore_candidate(),
            transforms: self.transforms.restore_candidate(),
            point_cloud: self
                .point_cloud
                .as_ref()
                .map(PointCloudController::restore_candidate),
            unknown_streams: self.unknown_streams,
            processing_time: self.processing_time,
        };

        for message in messages {
            let mut matched = false;
            matched |= restore_one("Camera", message, || candidate.cameras.restore(message))?;
            matched |= restore_one("Path", message, || candidate.path.restore(message))?;
            matched |= restore_one("Odometry", message, || candidate.odometry.restore(message))?;
            matched |= restore_one("Transform", message, || {
                candidate.transforms.restore(message)
            })?;
            if let Some(point_cloud) = &mut candidate.point_cloud {
                matched |= restore_one("Scene", message, || point_cloud.restore(message))?;
            }
            if !matched {
                return Err(FeatureRestoreError {
                    stream_id: message.stream_id,
                    arrival_time: message.arrival_time,
                    kind: FeatureRestoreErrorKind::Unrouted,
                });
            }
        }
        candidate.processing_time.record(started.elapsed());
        Ok(candidate)
    }

    pub fn commit_restore(&mut self, candidate: Self) {
        *self = candidate;
    }

    pub fn restore_transactional(
        &mut self,
        target: ArrivalTime,
        messages: &[RawMessage],
    ) -> Result<(), FeatureRestoreError> {
        let candidate = self.stage_restore(target, messages)?;
        self.commit_restore(candidate);
        Ok(())
    }

    pub fn cameras(&self) -> &CameraController {
        &self.cameras
    }

    pub fn path(&self) -> &PathController {
        &self.path
    }

    pub fn odometry(&self) -> &OdometryController {
        &self.odometry
    }

    pub fn transforms(&self) -> &TransformController {
        &self.transforms
    }

    pub fn point_cloud(&self) -> Option<&PointCloudController> {
        self.point_cloud.as_ref()
    }

    pub fn point_cloud_mut(&mut self) -> Option<&mut PointCloudController> {
        self.point_cloud.as_mut()
    }

    pub fn set_scheduling_priority(&mut self, camera: Option<CameraId>) {
        self.cameras.set_focused_camera(camera);
    }

    pub fn scheduling_priority(&self) -> Option<CameraId> {
        self.cameras.focused_camera()
    }

    pub fn counters(&self) -> ProcessingCounters {
        let mut counters = ProcessingCounters {
            unknown_streams: self.unknown_streams,
            ..ProcessingCounters::default()
        };
        counters.merge(self.cameras.counters());
        counters.merge(self.path.counters());
        counters.merge(self.odometry.counters());
        counters.merge(self.transforms.counters());
        if let Some(point_cloud) = &self.point_cloud {
            counters.merge(point_cloud.counters());
        }
        counters
    }

    pub fn processing_time(&self) -> StageTiming {
        self.processing_time
    }

    pub fn playback_performance(&self, source_read: StageTiming) -> PlaybackPerformance {
        PlaybackPerformance::from_controllers(source_read, self.processing_time, &self.cameras)
    }
}

fn restore_one(
    feature: &'static str,
    message: &RawMessage,
    apply: impl FnOnce() -> Result<bool, crate::DecodeError>,
) -> Result<bool, FeatureRestoreError> {
    apply().map_err(|error| FeatureRestoreError {
        stream_id: message.stream_id,
        arrival_time: message.arrival_time,
        kind: FeatureRestoreErrorKind::Decode {
            feature,
            source: error,
        },
    })
}
