use crate::interaction::PreviewDragState;
use memmap2::Mmap;
use std::{fs, fs::File, path::Path};
use viewer_core::{
    ArrivalTime, PreviewBudget, PreviewRequest, PreviewSnapshot, SignalId, SourceFingerprint,
};
use viewer_preview_mcap::{PreviewArtifact, read_preview_mcap, source_fingerprint};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum PreviewStatus {
    #[default]
    Unavailable,
    Ready,
    Warning(String),
}

#[derive(Default)]
pub(crate) struct PreviewCoordinator {
    artifact: Option<PreviewArtifact>,
    latest_snapshot: Option<PreviewSnapshot>,
    pub(crate) drag: PreviewDragState,
    status: PreviewStatus,
}

impl PreviewCoordinator {
    pub(crate) fn clear(&mut self) {
        self.artifact = None;
        self.latest_snapshot = None;
        self.drag.clear();
        self.status = PreviewStatus::Unavailable;
    }

    pub(crate) fn load_for_source(&mut self, source_path: &Path, expected: &SourceFingerprint) {
        self.clear();
        let preview_path = source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("preview.mcap");
        if !preview_path.exists() {
            self.status = PreviewStatus::Warning(format!(
                "Preview unavailable: {} was not found",
                preview_path.display()
            ));
            return;
        }
        let result = fs::read(&preview_path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| read_preview_mcap(&bytes).map_err(|error| error.to_string()))
            .and_then(|artifact| {
                artifact
                    .validate_source(expected)
                    .map_err(|error| error.to_string())?;
                Ok(artifact)
            });
        match result {
            Ok(artifact) => {
                self.artifact = Some(artifact);
                self.status = PreviewStatus::Ready;
                self.update(None);
            }
            Err(error) => {
                self.status = PreviewStatus::Warning(format!("Preview disabled: {error}"));
            }
        }
    }

    pub(crate) fn update(&mut self, target_time: Option<ArrivalTime>) {
        let Some(artifact) = &self.artifact else {
            self.latest_snapshot = None;
            return;
        };
        let Some(range) = artifact.available_range else {
            self.latest_snapshot = None;
            return;
        };
        let request = PreviewRequest {
            range,
            target_time,
            camera_ids: artifact.camera_frames.keys().copied().collect(),
            signal_ids: artifact.signal_overviews.keys().copied().collect(),
            budget: PreviewBudget {
                max_camera_frames: artifact.camera_frames.len(),
                max_signal_buckets_per_signal: 4_000,
                max_trajectory_points: 2_000,
            },
        };
        match artifact.query(&request) {
            Ok(snapshot) => self.latest_snapshot = Some(snapshot),
            Err(error) => {
                self.latest_snapshot = None;
                self.status = PreviewStatus::Warning(format!("Preview query failed: {error}"));
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<&PreviewSnapshot> {
        self.latest_snapshot.as_ref()
    }

    pub(crate) fn speed_overview(&self) -> Option<&viewer_core::SignalOverview> {
        self.latest_snapshot
            .as_ref()?
            .signal_overviews()
            .iter()
            .find(|overview| overview.signal_id() == SignalId::Speed)
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        match &self.status {
            PreviewStatus::Warning(message) => Some(message),
            PreviewStatus::Unavailable | PreviewStatus::Ready => None,
        }
    }
}

pub(crate) fn fingerprint_source(path: &Path) -> Result<SourceFingerprint, String> {
    let file = File::open(path).map_err(|error| format!("open source for fingerprint: {error}"))?;
    // SAFETY: this is a read-only mapping held only while calculating summary identity. The viewer
    // never mutates an opened source file; concurrent external mutation is unsupported.
    let mapped = unsafe { Mmap::map(&file) }
        .map_err(|error| format!("memory-map source for fingerprint: {error}"))?;
    source_fingerprint(&mapped).map_err(|error| error.to_string())
}
