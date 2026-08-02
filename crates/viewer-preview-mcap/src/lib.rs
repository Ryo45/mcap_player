mod error;
mod fingerprint;
mod query;
mod reader;
pub mod schema;
mod writer;

pub use error::PreviewMcapError;
pub use fingerprint::{SOURCE_FINGERPRINT_ALGORITHM, source_fingerprint};
pub use reader::{PreviewArtifact, read_preview_mcap};
pub use writer::PreviewMcapWriter;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use viewer_core::{
        ArrivalTime, CameraId, CameraPreviewFrame, PreviewBudget, PreviewBuildInfo,
        PreviewImageEncoding, PreviewRequest, SignalBucket, SignalFidelity, SignalId,
        SignalOverview, SourceFingerprint, TimeRange, TimedPosition2,
    };

    fn build_info() -> PreviewBuildInfo {
        PreviewBuildInfo::new(
            "test",
            "0",
            SourceFingerprint::new(SOURCE_FINGERPRINT_ALGORITHM, "fixture").unwrap(),
        )
        .unwrap()
    }

    fn frame(camera: u16, time: i64) -> CameraPreviewFrame {
        CameraPreviewFrame::new(
            CameraId(camera),
            None,
            ArrivalTime(time),
            "camera".to_owned(),
            PreviewImageEncoding::Jpeg,
            2,
            1,
            vec![0xff, 0xd8, 0xff, 0xd9],
        )
        .unwrap()
    }

    fn bucket(start: i64, value: f64) -> SignalBucket {
        SignalBucket::new(
            ArrivalTime(start),
            ArrivalTime(start + 100),
            value,
            value,
            value,
            value,
            1,
        )
        .unwrap()
    }

    fn write_artifact(
        frames: &[CameraPreviewFrame],
        buckets: &[SignalBucket],
        points: &[TimedPosition2],
    ) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = PreviewMcapWriter::new(&mut output, &build_info()).unwrap();
            for frame in frames {
                writer.write_camera_frame(frame).unwrap();
            }
            if !buckets.is_empty() {
                writer
                    .write_signal_overview(
                        &SignalOverview::new(
                            SignalId::Speed,
                            SignalFidelity::Envelope { bucket_ns: 100 },
                            buckets.to_vec(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
            }
            for point in points {
                writer.write_trajectory_point(*point).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    #[test]
    fn build_info_only_round_trips_without_range() {
        let bytes = write_artifact(&[], &[], &[]);
        let artifact = read_preview_mcap(&bytes).unwrap();
        assert_eq!(artifact.build_info, build_info());
        assert_eq!(artifact.available_range, None);
    }

    #[test]
    fn camera_signal_trajectory_and_multiple_cameras_round_trip() {
        let points = [TimedPosition2::new(ArrivalTime(75), [1.0, 2.0]).unwrap()];
        let bytes = write_artifact(&[frame(0, 50), frame(1, 60)], &[bucket(0, 3.0)], &points);
        let artifact = read_preview_mcap(&bytes).unwrap();
        assert_eq!(artifact.camera_frames.len(), 2);
        assert_eq!(
            artifact.signal_overviews[&SignalId::Speed].buckets(),
            &[bucket(0, 3.0)]
        );
        assert_eq!(artifact.trajectory, points);
        assert_eq!(artifact.available_range.unwrap().start(), ArrivalTime(0));
    }

    #[test]
    fn query_selects_prior_then_following_camera_and_merges_signal_budget() {
        let buckets = [bucket(0, 1.0), bucket(100, 3.0), bucket(200, 5.0)];
        let bytes = write_artifact(&[frame(0, 100), frame(0, 200)], &buckets, &[]);
        let artifact = read_preview_mcap(&bytes).unwrap();
        let request = |target| PreviewRequest {
            range: TimeRange::new(ArrivalTime(0), ArrivalTime(300)).unwrap(),
            target_time: Some(ArrivalTime(target)),
            camera_ids: vec![CameraId(0)],
            signal_ids: vec![SignalId::Speed],
            budget: PreviewBudget {
                max_camera_frames: 1,
                max_signal_buckets_per_signal: 2,
                max_trajectory_points: 0,
            },
        };
        let before = artifact.query(&request(50)).unwrap();
        assert_eq!(before.camera_frames()[0].arrival_time(), ArrivalTime(100));
        let prior = artifact.query(&request(250)).unwrap();
        assert_eq!(prior.camera_frames()[0].arrival_time(), ArrivalTime(200));
        assert_eq!(prior.signal_overviews()[0].buckets().len(), 2);
        assert_eq!(prior.signal_overviews()[0].buckets()[1].count(), 2);
    }

    #[test]
    fn source_validation_reports_stale_preview() {
        let artifact = read_preview_mcap(&write_artifact(&[], &[], &[])).unwrap();
        assert!(artifact.validate_source(build_info().source()).is_ok());
        let other = SourceFingerprint::new(SOURCE_FINGERPRINT_ALGORITHM, "other").unwrap();
        assert!(matches!(
            artifact.validate_source(&other),
            Err(PreviewMcapError::StalePreview { .. })
        ));
    }

    #[test]
    fn writer_rejects_non_matching_bucket_width_and_empty_camera() {
        let mut output = Cursor::new(Vec::new());
        let mut writer = PreviewMcapWriter::new(&mut output, &build_info()).unwrap();
        let overview = SignalOverview::new(
            SignalId::Speed,
            SignalFidelity::Envelope { bucket_ns: 99 },
            vec![bucket(0, 1.0)],
        )
        .unwrap();
        assert!(writer.write_signal_overview(&overview).is_err());
        assert!(
            CameraPreviewFrame::new(
                CameraId(0),
                None,
                ArrivalTime(0),
                String::new(),
                PreviewImageEncoding::Jpeg,
                1,
                1,
                vec![]
            )
            .is_err()
        );
    }

    #[test]
    fn truncated_camera_envelope_is_rejected() {
        assert!(decode_test_camera_payload(&[1, 0, 0]).is_err());
    }

    fn decode_test_camera_payload(payload: &[u8]) -> Result<(), PreviewMcapError> {
        let bytes = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            (&schema::camera_topic(CameraId(0)), 0, payload.to_vec()),
        ]);
        read_preview_mcap(&bytes).map(|_| ())
    }

    fn build_info_payload(version: u32) -> Vec<u8> {
        serde_json::to_vec(&schema::BuildInfoWire {
            preview_schema_version: version,
            generator_name: "test".to_owned(),
            generator_version: "0".to_owned(),
            source_fingerprint: build_info().source().clone(),
        })
        .unwrap()
    }

    fn raw_mcap(messages: Vec<(&str, u64, Vec<u8>)>) -> Vec<u8> {
        use std::collections::BTreeMap;
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = mcap::Writer::with_options(
                &mut output,
                mcap::WriteOptions::default().compression(None),
            )
            .unwrap();
            let mut channels = BTreeMap::new();
            for (sequence, (topic, time, payload)) in messages.into_iter().enumerate() {
                let channel = match channels.get(topic) {
                    Some(channel) => *channel,
                    None => {
                        let channel = writer
                            .add_channel(0, topic, "application/octet-stream", &BTreeMap::new())
                            .unwrap();
                        channels.insert(topic.to_owned(), channel);
                        channel
                    }
                };
                writer
                    .write_to_known_channel(
                        &mcap::records::MessageHeader {
                            channel_id: channel,
                            sequence: sequence as u32,
                            log_time: time,
                            publish_time: time,
                        },
                        &payload,
                    )
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn camera_payload(topic_id: u16, payload_id: u16, time: i64) -> (String, Vec<u8>) {
        let metadata = serde_json::to_vec(&schema::CameraMetadataWire {
            schema_version: 1,
            camera_id: CameraId(payload_id),
            measurement_time: None,
            arrival_time: ArrivalTime(time),
            frame_id: "camera".to_owned(),
            encoding: "jpeg".to_owned(),
            width: 1,
            height: 1,
        })
        .unwrap();
        let mut payload = Vec::new();
        payload.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        payload.extend_from_slice(&metadata);
        payload.extend_from_slice(&[0xff, 0xd8, 0xff, 0xd9]);
        (schema::camera_topic(CameraId(topic_id)), payload)
    }

    #[test]
    fn missing_duplicate_and_unknown_build_info_are_rejected() {
        assert!(matches!(
            read_preview_mcap(&raw_mcap(vec![])),
            Err(PreviewMcapError::MissingBuildInfo)
        ));
        let duplicate = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            (schema::BUILD_INFO_TOPIC, 1, build_info_payload(1)),
        ]);
        assert!(matches!(
            read_preview_mcap(&duplicate),
            Err(PreviewMcapError::DuplicateBuildInfo)
        ));
        let future = raw_mcap(vec![(schema::BUILD_INFO_TOPIC, 0, build_info_payload(2))]);
        assert!(read_preview_mcap(&future).is_err());
    }

    #[test]
    fn camera_and_signal_topic_payload_mismatches_are_rejected() {
        let (topic, camera) = camera_payload(0, 1, 10);
        let camera_bytes = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            (&topic, 10, camera),
        ]);
        assert!(read_preview_mcap(&camera_bytes).is_err());

        let signal = serde_json::to_vec(&schema::SignalBucketWire::from_bucket(
            SignalId::Speed,
            100,
            bucket(0, 1.0),
        ))
        .unwrap();
        let wrong_topic = format!("{}unknown", schema::SIGNAL_TOPIC_PREFIX);
        let signal_bytes = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            (&wrong_topic, 0, signal),
        ]);
        let artifact = read_preview_mcap(&signal_bytes).unwrap();
        assert!(artifact.signal_overviews.is_empty());
    }

    #[test]
    fn camera_empty_jpeg_header_time_and_writer_order_are_rejected() {
        let (topic, mut payload) = camera_payload(0, 0, 10);
        let metadata_len = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
        payload.truncate(4 + metadata_len);
        let empty = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            (&topic, 10, payload),
        ]);
        assert!(read_preview_mcap(&empty).is_err());

        let (topic, payload) = camera_payload(0, 0, 10);
        let wrong_header = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            (&topic, 11, payload),
        ]);
        assert!(read_preview_mcap(&wrong_header).is_err());

        let mut output = Cursor::new(Vec::new());
        let mut writer = PreviewMcapWriter::new(&mut output, &build_info()).unwrap();
        writer.write_camera_frame(&frame(0, 20)).unwrap();
        assert!(writer.write_camera_frame(&frame(0, 10)).is_err());
    }

    #[test]
    fn unknown_topics_are_ignored() {
        let bytes = raw_mcap(vec![
            (schema::BUILD_INFO_TOPIC, 0, build_info_payload(1)),
            ("/vendor/future", 1, vec![1, 2, 3]),
        ]);
        let artifact = read_preview_mcap(&bytes).unwrap();
        assert_eq!(artifact.available_range, None);
    }

    #[test]
    fn summary_fingerprint_is_stable_and_path_independent() {
        let bytes = raw_mcap(vec![("/source", 12, vec![1])]);
        let first = source_fingerprint(&bytes).unwrap();
        let second = source_fingerprint(&bytes).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.algorithm(), SOURCE_FINGERPRINT_ALGORITHM);
        assert!(!first.value().contains('/'));
    }

    #[test]
    fn query_filters_signal_range_and_allows_partial_requests() {
        let bytes = write_artifact(
            &[],
            &[bucket(0, 1.0), bucket(100, 2.0), bucket(200, 3.0)],
            &[],
        );
        let artifact = read_preview_mcap(&bytes).unwrap();
        let snapshot = artifact
            .query(&PreviewRequest {
                range: TimeRange::new(ArrivalTime(101), ArrivalTime(199)).unwrap(),
                target_time: None,
                camera_ids: vec![],
                signal_ids: vec![SignalId::Speed],
                budget: PreviewBudget {
                    max_camera_frames: 0,
                    max_signal_buckets_per_signal: 10,
                    max_trajectory_points: 0,
                },
            })
            .unwrap();
        assert_eq!(snapshot.signal_overviews()[0].buckets().len(), 1);
        assert!(snapshot.camera_frames().is_empty());
    }
}
