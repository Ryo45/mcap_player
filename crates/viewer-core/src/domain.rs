use crate::{BevState, CameraState, DomainUpdate, PointCloudState, TelemetryState, TransformState};

/// Current accepted values for all decoded playback domains.
#[derive(Clone, Debug, Default)]
pub struct DomainState {
    pub camera: CameraState,
    pub bev: BevState,
    pub telemetry: TelemetryState,
    pub point_cloud: PointCloudState,
    pub transforms: TransformState,
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
            DomainUpdate::PointCloud(frame) => {
                self.point_cloud.apply(frame);
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrivalTime, MeasurementTime, PointCloudFrame};

    #[test]
    fn stores_scan_in_its_source_frame_without_requiring_tf() {
        let mut state = DomainState::default();
        state.apply(DomainUpdate::PointCloud(PointCloudFrame {
            measurement_time: MeasurementTime(10),
            arrival_time: ArrivalTime(2),
            frame_id: "scan".into(),
            points: vec![[1.0, 0.0, 0.0]],
        }));

        let cloud = state.point_cloud.latest().unwrap();
        assert_eq!(cloud.frame_id, "scan");
        assert_eq!(cloud.points, vec![[1.0, 0.0, 0.0]]);
    }
}
