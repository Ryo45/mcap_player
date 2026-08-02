use crate::{
    PreviewMcapError,
    schema::{
        BUILD_INFO_TOPIC, BuildInfoWire, CameraMetadataWire, SignalBucketWire, TRAJECTORY_TOPIC,
        TrajectoryWire, WIRE_SCHEMA_VERSION, parse_camera_topic, parse_signal_topic,
    },
};
use mcap::MessageStream;
use std::collections::BTreeMap;
use viewer_core::{
    ArrivalTime, CameraId, CameraPreviewFrame, PreviewBuildInfo, PreviewImageEncoding,
    SignalBucket, SignalFidelity, SignalId, SignalOverview, SourceFingerprint, TimeRange,
    TimedPosition2,
};

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewArtifact {
    pub build_info: PreviewBuildInfo,
    pub available_range: Option<TimeRange>,
    pub camera_frames: BTreeMap<CameraId, Vec<CameraPreviewFrame>>,
    pub signal_overviews: BTreeMap<SignalId, SignalOverview>,
    pub trajectory: Vec<TimedPosition2>,
}

impl PreviewArtifact {
    pub fn validate_source(&self, expected: &SourceFingerprint) -> Result<(), PreviewMcapError> {
        if self.build_info.source() == expected {
            Ok(())
        } else {
            Err(PreviewMcapError::StalePreview {
                expected: format!("{}:{}", expected.algorithm(), expected.value()),
                actual: format!(
                    "{}:{}",
                    self.build_info.source().algorithm(),
                    self.build_info.source().value()
                ),
            })
        }
    }
}

pub fn read_preview_mcap(bytes: &[u8]) -> Result<PreviewArtifact, PreviewMcapError> {
    let mut build_info = None;
    let mut camera_frames: BTreeMap<CameraId, Vec<CameraPreviewFrame>> = BTreeMap::new();
    let mut signal_buckets: BTreeMap<SignalId, (i64, Vec<SignalBucket>)> = BTreeMap::new();
    let mut trajectory = Vec::new();

    for message in MessageStream::new(bytes)? {
        let message = message?;
        let topic = message.channel.topic.as_ref();
        if topic == BUILD_INFO_TOPIC {
            if build_info.is_some() {
                return Err(PreviewMcapError::DuplicateBuildInfo);
            }
            let wire: BuildInfoWire = serde_json::from_slice(&message.data)?;
            ensure_version(wire.preview_schema_version, topic)?;
            build_info = Some(
                PreviewBuildInfo::new(
                    wire.generator_name,
                    wire.generator_version,
                    wire.source_fingerprint,
                )
                .map_err(|error| PreviewMcapError::invalid(error.to_string()))?,
            );
        } else if let Some(topic_camera_id) = parse_camera_topic(topic) {
            let (metadata, jpeg) = decode_camera_envelope(&message.data)?;
            ensure_version(metadata.schema_version, topic)?;
            if metadata.camera_id != topic_camera_id {
                return Err(PreviewMcapError::invalid(format!(
                    "camera topic id {} does not match payload id {}",
                    topic_camera_id.0, metadata.camera_id.0
                )));
            }
            if metadata.encoding != "jpeg" {
                return Err(PreviewMcapError::invalid("unsupported camera encoding"));
            }
            ensure_header_time(message.log_time, metadata.arrival_time, topic)?;
            let frame = CameraPreviewFrame::new(
                metadata.camera_id,
                metadata.measurement_time,
                metadata.arrival_time,
                metadata.frame_id,
                PreviewImageEncoding::Jpeg,
                metadata.width,
                metadata.height,
                jpeg.to_vec(),
            )
            .map_err(|error| PreviewMcapError::invalid(error.to_string()))?;
            camera_frames
                .entry(topic_camera_id)
                .or_default()
                .push(frame);
        } else if let Some(topic_signal_id) = parse_signal_topic(topic) {
            let wire: SignalBucketWire = serde_json::from_slice(&message.data)?;
            ensure_version(wire.schema_version, topic)?;
            if wire.signal_id != topic_signal_id {
                return Err(PreviewMcapError::invalid(format!(
                    "signal topic id {:?} does not match payload id {:?}",
                    topic_signal_id, wire.signal_id
                )));
            }
            if wire.bucket_ns <= 0
                || wire.bucket_end.0.checked_sub(wire.bucket_start.0) != Some(wire.bucket_ns)
            {
                return Err(PreviewMcapError::invalid("signal bucket width is invalid"));
            }
            ensure_header_time(message.log_time, wire.bucket_start, topic)?;
            let bucket = SignalBucket::new(
                wire.bucket_start,
                wire.bucket_end,
                wire.first,
                wire.last,
                wire.min,
                wire.max,
                wire.count,
            )
            .map_err(|error| PreviewMcapError::invalid(error.to_string()))?;
            let entry = signal_buckets
                .entry(topic_signal_id)
                .or_insert_with(|| (wire.bucket_ns, Vec::new()));
            if entry.0 != wire.bucket_ns {
                return Err(PreviewMcapError::invalid(
                    "signal bucketNs changes within one signal",
                ));
            }
            entry.1.push(bucket);
        } else if topic == TRAJECTORY_TOPIC {
            let wire: TrajectoryWire = serde_json::from_slice(&message.data)?;
            ensure_version(wire.schema_version, topic)?;
            ensure_header_time(message.log_time, wire.time, topic)?;
            trajectory.push(
                TimedPosition2::new(wire.time, wire.position)
                    .map_err(|error| PreviewMcapError::invalid(error.to_string()))?,
            );
        }
    }

    let build_info = build_info.ok_or(PreviewMcapError::MissingBuildInfo)?;
    for frames in camera_frames.values_mut() {
        frames.sort_by_key(|frame| frame.arrival_time());
    }
    trajectory.sort_by_key(|point| point.time());
    let mut signal_overviews = BTreeMap::new();
    for (signal_id, (bucket_ns, mut buckets)) in signal_buckets {
        buckets.sort_by_key(|bucket| bucket.start_time());
        let overview =
            SignalOverview::new(signal_id, SignalFidelity::Envelope { bucket_ns }, buckets)
                .map_err(|error| PreviewMcapError::invalid(error.to_string()))?;
        signal_overviews.insert(signal_id, overview);
    }
    let available_range = data_range(&camera_frames, &signal_overviews, &trajectory)?;
    Ok(PreviewArtifact {
        build_info,
        available_range,
        camera_frames,
        signal_overviews,
        trajectory,
    })
}

fn decode_camera_envelope(data: &[u8]) -> Result<(CameraMetadataWire, &[u8]), PreviewMcapError> {
    let length_bytes: [u8; 4] = data
        .get(..4)
        .ok_or_else(|| PreviewMcapError::invalid("camera envelope is truncated"))?
        .try_into()
        .expect("four-byte slice");
    let metadata_len = u32::from_le_bytes(length_bytes) as usize;
    let metadata_end = 4_usize
        .checked_add(metadata_len)
        .ok_or_else(|| PreviewMcapError::invalid("camera metadata length overflows"))?;
    let metadata = data
        .get(4..metadata_end)
        .ok_or_else(|| PreviewMcapError::invalid("camera metadata is truncated"))?;
    let jpeg = data
        .get(metadata_end..)
        .ok_or_else(|| PreviewMcapError::invalid("camera envelope is truncated"))?;
    if jpeg.is_empty() {
        return Err(PreviewMcapError::invalid("camera JPEG is empty"));
    }
    Ok((serde_json::from_slice(metadata)?, jpeg))
}

fn ensure_version(version: u32, topic: &str) -> Result<(), PreviewMcapError> {
    if version == WIRE_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(PreviewMcapError::invalid(format!(
            "unsupported schema version {version} on {topic}"
        )))
    }
}

fn ensure_header_time(
    log_time: u64,
    payload_time: ArrivalTime,
    topic: &str,
) -> Result<(), PreviewMcapError> {
    let payload_time = u64::try_from(payload_time.0)
        .map_err(|_| PreviewMcapError::invalid("negative payload timestamp"))?;
    if log_time == payload_time {
        Ok(())
    } else {
        Err(PreviewMcapError::invalid(format!(
            "MCAP log_time does not match payload time on {topic}"
        )))
    }
}

fn data_range(
    cameras: &BTreeMap<CameraId, Vec<CameraPreviewFrame>>,
    signals: &BTreeMap<SignalId, SignalOverview>,
    trajectory: &[TimedPosition2],
) -> Result<Option<TimeRange>, PreviewMcapError> {
    let camera_times = cameras
        .values()
        .flat_map(|frames| frames.iter().map(|frame| frame.arrival_time()));
    let signal_times = signals.values().flat_map(|overview| {
        overview
            .buckets()
            .iter()
            .flat_map(|bucket| [bucket.start_time(), bucket.end_time()])
    });
    let trajectory_times = trajectory.iter().map(|point| point.time());
    let mut times = camera_times.chain(signal_times).chain(trajectory_times);
    let Some(first) = times.next() else {
        return Ok(None);
    };
    let (start, end) = times.fold((first, first), |(start, end), time| {
        (start.min(time), end.max(time))
    });
    TimeRange::new(start, end)
        .map(Some)
        .map_err(|error| PreviewMcapError::invalid(error.to_string()))
}
