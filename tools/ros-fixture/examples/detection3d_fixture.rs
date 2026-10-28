//! Indexed synthetic ROS Detection3DArray recording alongside the unchanged bundled Scene data.
use anyhow::{Context, Result};
use std::{borrow::Cow, fs::File, io::BufWriter, path::PathBuf, sync::Arc};
use viewer_core::{
    BoundingBox3D, Detection3D, Detection3DArray, MessageHeader, ObjectHypothesis,
    ObjectHypothesisWithPose, Pose3D, PoseWithCovariance, decode_path,
    encode_detection3d_array_cdr,
};
use viewer_mcap::FileMessageStream;
const SCHEMA: &str = include_str!("../../../tests/fixtures/ros2-detection3d/schema.msg");
fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("usage: detection3d_fixture NEW_OUTPUT.mcap")?,
    );
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
    let mut writer = mcap::Writer::new(BufWriter::new(
        File::options().write(true).create_new(true).open(&output)?,
    ))?;
    let mut paths = 0;
    let mut frames = 0;
    for message in FileMessageStream::open(&source)? {
        let message = message?;
        writer.write(&message)?;
        if message.channel.topic != "/planning/path" {
            continue;
        }
        paths += 1;
        if paths % 2 == 0 {
            continue;
        }
        let path = decode_path(&message.data)?;
        let header = MessageHeader {
            stamp: path.measurement_time,
            frame_id: path.frame_id,
        };
        let detections = path
            .points
            .iter()
            .step_by(4)
            .take(8)
            .enumerate()
            .map(|(i, p)| {
                let half_yaw = i as f64 * 0.07;
                let pose = Pose3D {
                    position: [p[0], p[1] + 2., p[2] + 0.3],
                    orientation: [0., 0., half_yaw.sin(), half_yaw.cos()],
                };
                let hypothesis = |class, score| ObjectHypothesisWithPose {
                    hypothesis: ObjectHypothesis {
                        class_id: class,
                        score,
                    },
                    pose: PoseWithCovariance {
                        pose: pose.clone(),
                        covariance: std::array::from_fn(
                            |index| if index % 7 == 0 { 0.01 } else { 0. },
                        ),
                    },
                };
                Detection3D {
                    header: header.clone(),
                    id: format!("track-{i:03}"),
                    bbox: BoundingBox3D {
                        center: pose.clone(),
                        size: if i % 2 == 0 {
                            [0.3, 0.3, 0.6]
                        } else {
                            [0.6, 0.35, 0.25]
                        },
                    },
                    results: vec![
                        hypothesis(
                            if i % 2 == 0 {
                                "person".into()
                            } else {
                                "car".into()
                            },
                            0.9,
                        ),
                        hypothesis("unknown".into(), 0.1),
                    ],
                }
            })
            .collect();
        let detection = Detection3DArray { header, detections };
        let mut extra = message.clone();
        let mut channel = message.channel.as_ref().clone();
        channel.id = 50_001;
        channel.topic = "/perception/detections".into();
        channel.schema = Some(Arc::new(mcap::Schema {
            id: 50_001,
            name: "vision_msgs/msg/Detection3DArray".into(),
            encoding: "ros2msg".into(),
            data: Cow::Borrowed(SCHEMA.as_bytes()),
        }));
        extra.channel = Arc::new(channel);
        extra.data = Cow::Owned(encode_detection3d_array_cdr(&detection)?);
        extra.log_time = extra
            .log_time
            .checked_add(5_000_000)
            .context("time overflow")?;
        writer.write(&extra)?;
        frames += 1;
    }
    writer.finish()?;
    println!(
        "Created {} with {frames} Detection3DArray frames, 8 string-ID objects with 2 hypotheses each; original Path unchanged",
        output.display()
    );
    Ok(())
}
