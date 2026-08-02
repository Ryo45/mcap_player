use crate::{PreviewArtifact, PreviewMcapError};
use viewer_core::{
    CameraPreviewFrame, DataFidelity, PreviewRequest, PreviewSnapshot, SignalOverview,
    TimedPosition2, merge_signal_buckets,
};

impl PreviewArtifact {
    pub fn query(&self, request: &PreviewRequest) -> Result<PreviewSnapshot, PreviewMcapError> {
        let available_range = self.available_range.unwrap_or(request.range);
        let mut camera_frames = Vec::new();
        if let Some(target) = request.target_time {
            for camera_id in request
                .camera_ids
                .iter()
                .take(request.budget.max_camera_frames)
            {
                if let Some(frames) = self.camera_frames.get(camera_id)
                    && let Some(frame) = closest_preview_frame(frames, target)
                {
                    camera_frames.push(frame.clone());
                }
            }
        }

        let mut signal_overviews = Vec::new();
        for signal_id in &request.signal_ids {
            let Some(overview) = self.signal_overviews.get(signal_id) else {
                continue;
            };
            let filtered: Vec<_> = overview
                .buckets()
                .iter()
                .copied()
                .filter(|bucket| {
                    bucket.end_time() >= request.range.start()
                        && bucket.start_time() <= request.range.end()
                })
                .collect();
            let buckets = merge_to_budget(&filtered, request.budget.max_signal_buckets_per_signal)?;
            signal_overviews.push(
                SignalOverview::new(*signal_id, overview.fidelity(), buckets)
                    .map_err(|error| PreviewMcapError::invalid(error.to_string()))?,
            );
        }

        let trajectory = sample_trajectory(
            self.trajectory
                .iter()
                .copied()
                .filter(|point| request.range.contains(point.time()))
                .collect(),
            request.budget.max_trajectory_points,
        );
        PreviewSnapshot::new(
            DataFidelity::Preview,
            available_range,
            camera_frames,
            signal_overviews,
            trajectory,
        )
        .map_err(|error| PreviewMcapError::invalid(error.to_string()))
    }
}

fn closest_preview_frame(
    frames: &[CameraPreviewFrame],
    target: viewer_core::ArrivalTime,
) -> Option<&CameraPreviewFrame> {
    let index = frames.partition_point(|frame| frame.arrival_time() <= target);
    if index > 0 {
        frames.get(index - 1)
    } else {
        frames.first()
    }
}

fn merge_to_budget(
    buckets: &[viewer_core::SignalBucket],
    budget: usize,
) -> Result<Vec<viewer_core::SignalBucket>, PreviewMcapError> {
    if budget == 0 || buckets.is_empty() {
        return Ok(Vec::new());
    }
    if buckets.len() <= budget {
        return Ok(buckets.to_vec());
    }
    let mut result = Vec::with_capacity(budget);
    for output_index in 0..budget {
        let start = output_index * buckets.len() / budget;
        let end = (output_index + 1) * buckets.len() / budget;
        result.push(
            merge_signal_buckets(&buckets[start..end])
                .map_err(|error| PreviewMcapError::invalid(error.to_string()))?,
        );
    }
    Ok(result)
}

fn sample_trajectory(points: Vec<TimedPosition2>, budget: usize) -> Vec<TimedPosition2> {
    if budget == 0 {
        return Vec::new();
    }
    if points.len() <= budget {
        return points;
    }
    if budget == 1 {
        return vec![points[0]];
    }
    (0..budget)
        .map(|index| points[index * (points.len() - 1) / (budget - 1)])
        .collect()
}
