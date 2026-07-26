use crate::{
    BevState, CameraState, DomainUpdate, PointCloudFrame, PointCloudState, TelemetryState,
    TransformState,
};

/// Current accepted values for all decoded playback domains.
#[derive(Clone, Debug, Default)]
pub struct DomainState {
    pub camera: CameraState,
    pub bev: BevState,
    pub telemetry: TelemetryState,
    pub point_cloud: PointCloudState,
    pub transforms: TransformState,
    pub tf_misses: u64,
    pub last_tf_route: Option<String>,
}

impl DomainState {
    pub fn apply(&mut self, update: DomainUpdate) {
        match update {
            DomainUpdate::Camera(frame) => {
                self.camera.apply(frame);
            }
            DomainUpdate::Path(frame) => {
                self.bev.apply(frame);
            }
            DomainUpdate::Telemetry(frame) => {
                self.telemetry.apply(frame);
            }
            DomainUpdate::PointCloud(frame) => self.apply_point_cloud(frame),
            DomainUpdate::Transforms(batch) => self.transforms.apply(batch),
        }
    }

    pub fn apply_all(&mut self, updates: impl IntoIterator<Item = DomainUpdate>) {
        for update in updates {
            self.apply(update);
        }
    }

    pub fn cold_seek(&mut self) {
        self.camera.cold_seek();
        self.bev.cold_seek();
        self.telemetry.cold_seek();
        self.point_cloud.cold_seek();
        self.transforms.clear_dynamic();
    }

    fn apply_point_cloud(&mut self, frame: PointCloudFrame) {
        let target_frame = self
            .telemetry
            .latest()
            .map_or("odom", |telemetry| telemetry.frame_id.as_str());
        let source_frame = frame.frame_id.clone();
        let Some(points) = self.transforms.transform_points_at(
            &source_frame,
            target_frame,
            frame.measurement_time,
            &frame.points,
        ) else {
            self.tf_misses = self.tf_misses.saturating_add(1);
            return;
        };
        let mut frame = frame;
        frame.points = points;
        frame.frame_id = target_frame.to_owned();
        self.last_tf_route = Some(format!("{source_frame} → {target_frame}"));
        self.point_cloud.apply(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrivalTime, MeasurementTime, TransformBatch, TransformStamped};

    #[test]
    fn anchors_scan_when_update_is_applied() {
        let mut state = DomainState::default();
        state.apply(DomainUpdate::Transforms(TransformBatch {
            arrival_time: ArrivalTime(1),
            is_static: true,
            transforms: vec![TransformStamped {
                measurement_time: MeasurementTime(0),
                frame_id: "odom".into(),
                child_frame_id: "scan".into(),
                translation: [2.0, 3.0, 0.5],
                rotation: [0.0, 0.0, 0.0, 1.0],
            }],
        }));
        state.apply(DomainUpdate::PointCloud(PointCloudFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(2),
            frame_id: "scan".into(),
            points: vec![[1.0, 0.0, 0.0]],
        }));

        let cloud = state.point_cloud.latest().unwrap();
        assert_eq!(cloud.frame_id, "odom");
        assert_eq!(cloud.points, vec![[3.0, 3.0, 0.5]]);
        assert_eq!(state.last_tf_route.as_deref(), Some("scan → odom"));
    }
}
