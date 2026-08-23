//! Bounded derived preview documents.

use crate::{ArrivalTime, CameraId, MeasurementTime, SignalId};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

pub const CURRENT_PREVIEW_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewValidationError {
    message: String,
}

impl PreviewValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PreviewValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PreviewValidationError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "TimeRangeWire")]
pub struct TimeRange {
    start: ArrivalTime,
    end: ArrivalTime,
}

impl TimeRange {
    pub fn new(start: ArrivalTime, end: ArrivalTime) -> Result<Self, PreviewValidationError> {
        if start > end {
            return Err(PreviewValidationError::new(format!(
                "time range start {} must not exceed end {}",
                start.0, end.0
            )));
        }
        Ok(Self { start, end })
    }

    pub fn start(self) -> ArrivalTime {
        self.start
    }

    pub fn end(self) -> ArrivalTime {
        self.end
    }

    pub fn contains(self, time: ArrivalTime) -> bool {
        self.start <= time && time <= self.end
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimeRangeWire {
    start: ArrivalTime,
    end: ArrivalTime,
}

impl TryFrom<TimeRangeWire> for TimeRange {
    type Error = PreviewValidationError;

    fn try_from(value: TimeRangeWire) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DataFidelity {
    Preview,
    Exact,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SignalFidelity {
    Envelope { bucket_ns: i64 },
    Exact,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewBudget {
    pub max_camera_frames: usize,
    pub max_signal_buckets_per_signal: usize,
    pub max_trajectory_points: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub range: TimeRange,
    pub target_time: Option<ArrivalTime>,
    pub camera_ids: Vec<CameraId>,
    pub signal_ids: Vec<SignalId>,
    pub budget: PreviewBudget,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "SignalBucketWire")]
pub struct SignalBucket {
    start_time: ArrivalTime,
    end_time: ArrivalTime,
    first: f64,
    last: f64,
    min: f64,
    max: f64,
    count: u32,
}

impl SignalBucket {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_time: ArrivalTime,
        end_time: ArrivalTime,
        first: f64,
        last: f64,
        min: f64,
        max: f64,
        count: u32,
    ) -> Result<Self, PreviewValidationError> {
        let bucket = Self {
            start_time,
            end_time,
            first,
            last,
            min,
            max,
            count,
        };
        bucket.validate()?;
        Ok(bucket)
    }

    pub fn start_time(self) -> ArrivalTime {
        self.start_time
    }

    pub fn end_time(self) -> ArrivalTime {
        self.end_time
    }

    pub fn first(self) -> f64 {
        self.first
    }

    pub fn last(self) -> f64 {
        self.last
    }

    pub fn min(self) -> f64 {
        self.min
    }

    pub fn max(self) -> f64 {
        self.max
    }

    pub fn count(self) -> u32 {
        self.count
    }

    pub fn validate(&self) -> Result<(), PreviewValidationError> {
        if self.start_time > self.end_time {
            return Err(PreviewValidationError::new(
                "signal bucket startTime must not exceed endTime",
            ));
        }
        if self.count == 0 {
            return Err(PreviewValidationError::new(
                "signal bucket count must be positive",
            ));
        }
        for (name, value) in [
            ("first", self.first),
            ("last", self.last),
            ("min", self.min),
            ("max", self.max),
        ] {
            if !value.is_finite() {
                return Err(PreviewValidationError::new(format!(
                    "signal bucket {name} must be finite"
                )));
            }
        }
        if self.min > self.max {
            return Err(PreviewValidationError::new(
                "signal bucket min must not exceed max",
            ));
        }
        if !(self.min..=self.max).contains(&self.first) {
            return Err(PreviewValidationError::new(
                "signal bucket first must be between min and max",
            ));
        }
        if !(self.min..=self.max).contains(&self.last) {
            return Err(PreviewValidationError::new(
                "signal bucket last must be between min and max",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalBucketWire {
    start_time: ArrivalTime,
    end_time: ArrivalTime,
    first: f64,
    last: f64,
    min: f64,
    max: f64,
    count: u32,
}

impl TryFrom<SignalBucketWire> for SignalBucket {
    type Error = PreviewValidationError;

    fn try_from(value: SignalBucketWire) -> Result<Self, Self::Error> {
        Self::new(
            value.start_time,
            value.end_time,
            value.first,
            value.last,
            value.min,
            value.max,
            value.count,
        )
    }
}

pub fn merge_signal_buckets(
    buckets: &[SignalBucket],
) -> Result<SignalBucket, PreviewValidationError> {
    let Some(first_bucket) = buckets.first().copied() else {
        return Err(PreviewValidationError::new(
            "cannot merge an empty signal bucket list",
        ));
    };
    validate_bucket_order(buckets)?;
    let last_bucket = buckets.last().copied().expect("non-empty checked");
    let mut count = 0_u32;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for bucket in buckets {
        count = count.checked_add(bucket.count).ok_or_else(|| {
            PreviewValidationError::new("merged signal bucket count exceeds u32::MAX")
        })?;
        min = min.min(bucket.min);
        max = max.max(bucket.max);
    }
    SignalBucket::new(
        first_bucket.start_time,
        last_bucket.end_time,
        first_bucket.first,
        last_bucket.last,
        min,
        max,
        count,
    )
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "SignalOverviewWire")]
pub struct SignalOverview {
    signal_id: SignalId,
    fidelity: SignalFidelity,
    buckets: Vec<SignalBucket>,
}

impl SignalOverview {
    pub fn new(
        signal_id: SignalId,
        fidelity: SignalFidelity,
        buckets: Vec<SignalBucket>,
    ) -> Result<Self, PreviewValidationError> {
        if let SignalFidelity::Envelope { bucket_ns } = fidelity
            && bucket_ns <= 0
        {
            return Err(PreviewValidationError::new(
                "signal envelope bucketNs must be positive",
            ));
        }
        validate_bucket_order(&buckets)?;
        Ok(Self {
            signal_id,
            fidelity,
            buckets,
        })
    }

    pub fn signal_id(&self) -> SignalId {
        self.signal_id
    }

    pub fn fidelity(&self) -> SignalFidelity {
        self.fidelity
    }

    pub fn buckets(&self) -> &[SignalBucket] {
        &self.buckets
    }

    pub fn validate(&self) -> Result<(), PreviewValidationError> {
        if let SignalFidelity::Envelope { bucket_ns } = self.fidelity
            && bucket_ns <= 0
        {
            return Err(PreviewValidationError::new(
                "signal envelope bucketNs must be positive",
            ));
        }
        validate_bucket_order(&self.buckets)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignalOverviewWire {
    signal_id: SignalId,
    fidelity: SignalFidelity,
    buckets: Vec<SignalBucket>,
}

impl TryFrom<SignalOverviewWire> for SignalOverview {
    type Error = PreviewValidationError;

    fn try_from(value: SignalOverviewWire) -> Result<Self, Self::Error> {
        Self::new(value.signal_id, value.fidelity, value.buckets)
    }
}

fn validate_bucket_order(buckets: &[SignalBucket]) -> Result<(), PreviewValidationError> {
    for (index, bucket) in buckets.iter().enumerate() {
        bucket.validate()?;
        if index > 0 && buckets[index - 1].end_time > bucket.start_time {
            return Err(PreviewValidationError::new(format!(
                "signal buckets are not time ordered at index {index}"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewImageEncoding {
    Jpeg,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "CameraPreviewFrameWire")]
pub struct CameraPreviewFrame {
    camera_id: CameraId,
    measurement_time: Option<MeasurementTime>,
    arrival_time: ArrivalTime,
    frame_id: String,
    encoding: PreviewImageEncoding,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl CameraPreviewFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        camera_id: CameraId,
        measurement_time: Option<MeasurementTime>,
        arrival_time: ArrivalTime,
        frame_id: String,
        encoding: PreviewImageEncoding,
        width: u32,
        height: u32,
        bytes: Vec<u8>,
    ) -> Result<Self, PreviewValidationError> {
        let frame = Self {
            camera_id,
            measurement_time,
            arrival_time,
            frame_id,
            encoding,
            width,
            height,
            bytes,
        };
        frame.validate()?;
        Ok(frame)
    }

    pub fn camera_id(&self) -> CameraId {
        self.camera_id
    }

    pub fn measurement_time(&self) -> Option<MeasurementTime> {
        self.measurement_time
    }

    pub fn arrival_time(&self) -> ArrivalTime {
        self.arrival_time
    }

    pub fn frame_id(&self) -> &str {
        &self.frame_id
    }

    pub fn encoding(&self) -> PreviewImageEncoding {
        self.encoding
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn validate(&self) -> Result<(), PreviewValidationError> {
        if self.width == 0 || self.height == 0 {
            return Err(PreviewValidationError::new(
                "camera preview width and height must be non-zero",
            ));
        }
        if self.bytes.is_empty() {
            return Err(PreviewValidationError::new(
                "camera preview bytes must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CameraPreviewFrameWire {
    camera_id: CameraId,
    measurement_time: Option<MeasurementTime>,
    arrival_time: ArrivalTime,
    frame_id: String,
    encoding: PreviewImageEncoding,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl TryFrom<CameraPreviewFrameWire> for CameraPreviewFrame {
    type Error = PreviewValidationError;

    fn try_from(value: CameraPreviewFrameWire) -> Result<Self, Self::Error> {
        Self::new(
            value.camera_id,
            value.measurement_time,
            value.arrival_time,
            value.frame_id,
            value.encoding,
            value.width,
            value.height,
            value.bytes,
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "TimedPosition2Wire")]
pub struct TimedPosition2 {
    time: ArrivalTime,
    position: [f32; 2],
}

impl TimedPosition2 {
    pub fn new(time: ArrivalTime, position: [f32; 2]) -> Result<Self, PreviewValidationError> {
        if !position.iter().all(|value| value.is_finite()) {
            return Err(PreviewValidationError::new(
                "trajectory position coordinates must be finite",
            ));
        }
        Ok(Self { time, position })
    }

    pub fn time(self) -> ArrivalTime {
        self.time
    }

    pub fn position(self) -> [f32; 2] {
        self.position
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimedPosition2Wire {
    time: ArrivalTime,
    position: [f32; 2],
}

impl TryFrom<TimedPosition2Wire> for TimedPosition2 {
    type Error = PreviewValidationError;

    fn try_from(value: TimedPosition2Wire) -> Result<Self, Self::Error> {
        Self::new(value.time, value.position)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", try_from = "PreviewSnapshotWire")]
pub struct PreviewSnapshot {
    fidelity: DataFidelity,
    available_range: TimeRange,
    camera_frames: Vec<CameraPreviewFrame>,
    signal_overviews: Vec<SignalOverview>,
    trajectory: Vec<TimedPosition2>,
}

impl PreviewSnapshot {
    pub fn new(
        fidelity: DataFidelity,
        available_range: TimeRange,
        camera_frames: Vec<CameraPreviewFrame>,
        signal_overviews: Vec<SignalOverview>,
        trajectory: Vec<TimedPosition2>,
    ) -> Result<Self, PreviewValidationError> {
        let snapshot = Self {
            fidelity,
            available_range,
            camera_frames,
            signal_overviews,
            trajectory,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn fidelity(&self) -> DataFidelity {
        self.fidelity
    }

    pub fn available_range(&self) -> TimeRange {
        self.available_range
    }

    pub fn camera_frames(&self) -> &[CameraPreviewFrame] {
        &self.camera_frames
    }

    pub fn signal_overviews(&self) -> &[SignalOverview] {
        &self.signal_overviews
    }

    pub fn trajectory(&self) -> &[TimedPosition2] {
        &self.trajectory
    }

    pub fn validate(&self) -> Result<(), PreviewValidationError> {
        for frame in &self.camera_frames {
            frame.validate()?;
        }
        for overview in &self.signal_overviews {
            overview.validate()?;
        }
        for (index, position) in self.trajectory.iter().enumerate() {
            if !position.position.iter().all(|value| value.is_finite()) {
                return Err(PreviewValidationError::new(format!(
                    "trajectory position at index {index} must be finite"
                )));
            }
            if index > 0 && self.trajectory[index - 1].time > position.time {
                return Err(PreviewValidationError::new(format!(
                    "trajectory is not time ordered at index {index}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewSnapshotWire {
    fidelity: DataFidelity,
    available_range: TimeRange,
    #[serde(default)]
    camera_frames: Vec<CameraPreviewFrame>,
    #[serde(default)]
    signal_overviews: Vec<SignalOverview>,
    #[serde(default)]
    trajectory: Vec<TimedPosition2>,
}

impl TryFrom<PreviewSnapshotWire> for PreviewSnapshot {
    type Error = PreviewValidationError;

    fn try_from(value: PreviewSnapshotWire) -> Result<Self, Self::Error> {
        Self::new(
            value.fidelity,
            value.available_range,
            value.camera_frames,
            value.signal_overviews,
            value.trajectory,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range() -> TimeRange {
        TimeRange::new(ArrivalTime(10), ArrivalTime(20)).unwrap()
    }

    fn bucket(start: i64, end: i64, first: f64, last: f64, min: f64, max: f64) -> SignalBucket {
        SignalBucket::new(
            ArrivalTime(start),
            ArrivalTime(end),
            first,
            last,
            min,
            max,
            2,
        )
        .unwrap()
    }

    #[test]
    fn time_range_accepts_ordered_and_rejects_reversed_values() {
        assert!(range().contains(ArrivalTime(15)));
        assert!(TimeRange::new(ArrivalTime(20), ArrivalTime(20)).is_ok());
        assert!(TimeRange::new(ArrivalTime(20), ArrivalTime(10)).is_err());
        assert!(serde_json::from_str::<TimeRange>(r#"{"start":20,"end":10}"#).is_err());
    }

    #[test]
    fn signal_bucket_validates_values_and_finiteness() {
        let value = bucket(10, 11, 2.0, 3.0, 1.0, 4.0);
        assert_eq!(value.count(), 2);
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                SignalBucket::new(ArrivalTime(10), ArrivalTime(11), invalid, 2.0, 1.0, 3.0, 1,)
                    .is_err()
            );
        }
        assert!(
            SignalBucket::new(ArrivalTime(10), ArrivalTime(11), 0.0, 2.0, 1.0, 3.0, 1,).is_err()
        );
    }

    #[test]
    fn merges_signal_buckets_without_losing_envelope_semantics() {
        let merged = merge_signal_buckets(&[
            bucket(10, 12, 2.0, 3.0, 1.0, 4.0),
            bucket(12, 14, 5.0, 6.0, -1.0, 7.0),
        ])
        .unwrap();
        assert_eq!(merged.start_time(), ArrivalTime(10));
        assert_eq!(merged.end_time(), ArrivalTime(14));
        assert_eq!(merged.first(), 2.0);
        assert_eq!(merged.last(), 6.0);
        assert_eq!(merged.min(), -1.0);
        assert_eq!(merged.max(), 7.0);
        assert_eq!(merged.count(), 4);
        assert!(merge_signal_buckets(&[]).is_err());
        let overflow = SignalBucket::new(
            ArrivalTime(10),
            ArrivalTime(11),
            1.0,
            1.0,
            1.0,
            1.0,
            u32::MAX,
        )
        .unwrap();
        assert!(merge_signal_buckets(&[overflow, bucket(11, 12, 1.0, 1.0, 1.0, 1.0)]).is_err());
    }

    #[test]
    fn camera_rejects_zero_size_and_empty_bytes() {
        let make = |width, height, bytes| {
            CameraPreviewFrame::new(
                CameraId(0),
                None,
                ArrivalTime(10),
                "camera".to_owned(),
                PreviewImageEncoding::Jpeg,
                width,
                height,
                bytes,
            )
        };
        assert!(make(640, 480, vec![1]).is_ok());
        assert!(make(0, 480, vec![1]).is_err());
        assert!(make(640, 0, vec![1]).is_err());
        assert!(make(640, 480, vec![]).is_err());
    }

    #[test]
    fn trajectory_rejects_non_finite_and_reversed_time() {
        assert!(TimedPosition2::new(ArrivalTime(10), [f32::NAN, 0.0]).is_err());
        assert!(TimedPosition2::new(ArrivalTime(10), [0.0, f32::INFINITY]).is_err());
        let reversed = PreviewSnapshot::new(
            DataFidelity::Preview,
            range(),
            vec![],
            vec![],
            vec![
                TimedPosition2::new(ArrivalTime(12), [0.0, 0.0]).unwrap(),
                TimedPosition2::new(ArrivalTime(11), [1.0, 0.0]).unwrap(),
            ],
        );
        assert!(reversed.is_err());
    }

    #[test]
    fn empty_and_partial_snapshots_are_valid() {
        let empty =
            PreviewSnapshot::new(DataFidelity::Preview, range(), vec![], vec![], vec![]).unwrap();
        assert!(empty.camera_frames().is_empty());

        let signal_only = PreviewSnapshot::new(
            DataFidelity::Preview,
            range(),
            vec![],
            vec![
                SignalOverview::new(
                    SignalId::Speed,
                    SignalFidelity::Envelope { bucket_ns: 10 },
                    vec![bucket(10, 11, 1.0, 2.0, 1.0, 2.0)],
                )
                .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        assert_eq!(signal_only.signal_overviews().len(), 1);
        let signal_json = serde_json::to_string(&signal_only).unwrap();
        assert!(signal_json.contains("\"bucketNs\":10"));
        assert_eq!(
            serde_json::from_str::<PreviewSnapshot>(&signal_json).unwrap(),
            signal_only
        );

        let camera_only = PreviewSnapshot::new(
            DataFidelity::Preview,
            range(),
            vec![
                CameraPreviewFrame::new(
                    CameraId(0),
                    Some(MeasurementTime(9)),
                    ArrivalTime(10),
                    "camera".to_owned(),
                    PreviewImageEncoding::Jpeg,
                    1,
                    1,
                    vec![1],
                )
                .unwrap(),
            ],
            vec![],
            vec![],
        )
        .unwrap();
        let json = serde_json::to_string(&camera_only).unwrap();
        let decoded: PreviewSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, camera_only);
    }

    #[test]
    fn signal_overview_rejects_unordered_buckets() {
        let result = SignalOverview::new(
            SignalId::Speed,
            SignalFidelity::Envelope { bucket_ns: 10 },
            vec![
                bucket(12, 14, 1.0, 1.0, 1.0, 1.0),
                bucket(10, 11, 1.0, 1.0, 1.0, 1.0),
            ],
        );
        assert!(result.is_err());
    }
}
