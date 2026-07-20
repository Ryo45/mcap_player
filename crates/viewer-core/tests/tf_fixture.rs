use std::{fs, path::PathBuf};
use viewer_core::{DomainUpdate, McapSource, PipelineSet, StreamBinding, TransformState};

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../mcap/rosbag2_2026_01_18-17_28_44/camera_bev_telemetry_scan_tf_5s.mcap");
    fs::read(path).expect("camera/scan/TF fixture")
}

#[test]
fn every_scan_resolves_to_world_at_its_measurement_time() {
    let mut source = McapSource::new(fixture()).unwrap();
    let scan = source.catalog().by_topic("/scan").unwrap();
    let dynamic_tf = source.catalog().by_topic("/tf").unwrap();
    let static_tf = source.catalog().by_topic("/tf_static").unwrap();
    let mut pipelines = PipelineSet::new(
        &source.catalog().streams,
        &[
            (scan.id, StreamBinding::LaserScan),
            (
                dynamic_tf.id,
                StreamBinding::Transforms { is_static: false },
            ),
            (static_tf.id, StreamBinding::Transforms { is_static: true }),
        ],
    );
    let (_, end) = source.time_range();
    let mut transforms = TransformState::default();
    let mut scan_count = 0;

    for message in source.read_until(end).unwrap() {
        let mut updates = Vec::new();
        pipelines.decode(message, &mut updates);
        for update in updates {
            match update {
                DomainUpdate::Transforms(batch) => transforms.apply(batch),
                DomainUpdate::PointCloud(scan) => {
                    let world = transforms
                        .transform_points_at(
                            &scan.frame_id,
                            "odom",
                            scan.measurement_time,
                            &scan.points,
                        )
                        .expect("scan-time base_scan -> odom transform");
                    assert_eq!(world.len(), scan.points.len());
                    assert!(world.iter().flatten().all(|value| value.is_finite()));
                    scan_count += 1;
                }
                _ => unreachable!("only scan and TF streams are bound"),
            }
        }
    }

    assert_eq!(scan_count, 44);
    assert_eq!(transforms.static_len(), 8);
    assert_eq!(transforms.dynamic_len(), 3);
}
