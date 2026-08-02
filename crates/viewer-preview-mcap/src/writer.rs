use crate::{
    PreviewMcapError,
    schema::{
        BUILD_INFO_TOPIC, BuildInfoWire, CameraMetadataWire, SignalBucketWire, TRAJECTORY_TOPIC,
        TrajectoryWire, WIRE_SCHEMA_VERSION, camera_topic, signal_topic,
    },
};
use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{
    collections::BTreeMap,
    io::{Seek, Write},
};
use viewer_core::{
    CameraPreviewFrame, PreviewBuildInfo, PreviewImageEncoding, SignalFidelity, SignalOverview,
    TimedPosition2,
};

pub struct PreviewMcapWriter<W: Write + Seek> {
    writer: Writer<W>,
    channels: BTreeMap<String, u16>,
    last_time: BTreeMap<String, u64>,
    sequence: u32,
}

impl<W: Write + Seek> PreviewMcapWriter<W> {
    pub fn new(output: W, build_info: &PreviewBuildInfo) -> Result<Self, PreviewMcapError> {
        build_info
            .validate()
            .map_err(|error| PreviewMcapError::invalid(error.to_string()))?;
        let options = WriteOptions::default()
            .compression(None)
            .profile("mcap-viewer-preview-v0")
            .library("viewer-preview-mcap");
        let writer = Writer::with_options(output, options)?;
        let mut this = Self {
            writer,
            channels: BTreeMap::new(),
            last_time: BTreeMap::new(),
            sequence: 0,
        };
        let payload = serde_json::to_vec(&BuildInfoWire::from(build_info))?;
        this.write_message(BUILD_INFO_TOPIC, "json", 0, &payload)?;
        Ok(this)
    }

    pub fn write_camera_frame(
        &mut self,
        frame: &CameraPreviewFrame,
    ) -> Result<(), PreviewMcapError> {
        frame
            .validate()
            .map_err(|error| PreviewMcapError::invalid(error.to_string()))?;
        if frame.encoding() != PreviewImageEncoding::Jpeg {
            return Err(PreviewMcapError::invalid(
                "preview camera encoding must be JPEG",
            ));
        }
        let metadata = CameraMetadataWire {
            schema_version: WIRE_SCHEMA_VERSION,
            camera_id: frame.camera_id(),
            measurement_time: frame.measurement_time(),
            arrival_time: frame.arrival_time(),
            frame_id: frame.frame_id().to_owned(),
            encoding: "jpeg".to_owned(),
            width: frame.width(),
            height: frame.height(),
        };
        let metadata = serde_json::to_vec(&metadata)?;
        let metadata_len = u32::try_from(metadata.len())
            .map_err(|_| PreviewMcapError::invalid("camera metadata is too large"))?;
        let mut payload = Vec::with_capacity(4 + metadata.len() + frame.bytes().len());
        payload.extend_from_slice(&metadata_len.to_le_bytes());
        payload.extend_from_slice(&metadata);
        payload.extend_from_slice(frame.bytes());
        self.write_message(
            &camera_topic(frame.camera_id()),
            "application/octet-stream",
            to_log_time(frame.arrival_time().0)?,
            &payload,
        )
    }

    pub fn write_signal_overview(
        &mut self,
        overview: &SignalOverview,
    ) -> Result<(), PreviewMcapError> {
        overview
            .validate()
            .map_err(|error| PreviewMcapError::invalid(error.to_string()))?;
        let SignalFidelity::Envelope { bucket_ns } = overview.fidelity() else {
            return Err(PreviewMcapError::invalid(
                "SignalFidelity::Exact cannot be written to preview.mcap",
            ));
        };
        for bucket in overview.buckets() {
            if bucket.end_time().0.checked_sub(bucket.start_time().0) != Some(bucket_ns) {
                return Err(PreviewMcapError::invalid(format!(
                    "signal bucket width does not match bucketNs {bucket_ns}"
                )));
            }
            let payload = serde_json::to_vec(&SignalBucketWire::from_bucket(
                overview.signal_id(),
                bucket_ns,
                *bucket,
            ))?;
            self.write_message(
                &signal_topic(overview.signal_id()),
                "json",
                to_log_time(bucket.start_time().0)?,
                &payload,
            )?;
        }
        Ok(())
    }

    pub fn write_trajectory_point(
        &mut self,
        point: TimedPosition2,
    ) -> Result<(), PreviewMcapError> {
        let payload = serde_json::to_vec(&TrajectoryWire::from(point))?;
        self.write_message(
            TRAJECTORY_TOPIC,
            "json",
            to_log_time(point.time().0)?,
            &payload,
        )
    }

    pub fn finish(mut self) -> Result<(), PreviewMcapError> {
        self.writer.finish()?;
        Ok(())
    }

    fn write_message(
        &mut self,
        topic: &str,
        encoding: &str,
        log_time: u64,
        payload: &[u8],
    ) -> Result<(), PreviewMcapError> {
        if self
            .last_time
            .get(topic)
            .is_some_and(|previous| log_time < *previous)
        {
            return Err(PreviewMcapError::invalid(format!(
                "messages for {topic} are not time ordered"
            )));
        }
        let channel_id = match self.channels.get(topic) {
            Some(channel_id) => *channel_id,
            None => {
                let channel_id = self
                    .writer
                    .add_channel(0, topic, encoding, &BTreeMap::new())?;
                self.channels.insert(topic.to_owned(), channel_id);
                channel_id
            }
        };
        self.writer.write_to_known_channel(
            &MessageHeader {
                channel_id,
                sequence: self.sequence,
                log_time,
                publish_time: log_time,
            },
            payload,
        )?;
        self.sequence = self.sequence.wrapping_add(1);
        self.last_time.insert(topic.to_owned(), log_time);
        Ok(())
    }
}

fn to_log_time(time: i64) -> Result<u64, PreviewMcapError> {
    u64::try_from(time)
        .map_err(|_| PreviewMcapError::invalid("negative time cannot be stored in MCAP"))
}
