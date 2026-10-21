use std::{path::Path, time::Duration};
use viewer_core::{
    ArrivalTime, CameraController, PlaybackRequirements, SessionPlan, WorkspaceBindings,
};
use viewer_mcap::McapSource;
use viewer_renderer::decode_camera_frame;

#[test]
fn all_camera_formats_share_selection_pixels_and_exact_backward_restore() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/camera-formats/camera_formats.mcap");
    let mut source = McapSource::open_file(&path).unwrap();
    let bindings: WorkspaceBindings = bindings();
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_all_cameras();
    let plan =
        SessionPlan::build(source.catalog(), "/camera/rgb8", &requirements, &bindings).unwrap();
    assert_eq!(plan.camera_routes().len(), 7);
    source.select_streams(plan.selected_stream_ids()).unwrap();
    let mut cameras = CameraController::new(&plan);
    let start = source.time_range().start;
    for target in [
        ArrivalTime(start.0 + 2_000_000_000),
        ArrivalTime(start.0 + 500_000_000),
    ] {
        cameras.reset_for_restore();
        for route in plan.camera_routes() {
            let message = source
                .latest_before(&[route.stream.id], target)
                .unwrap()
                .messages
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(message.arrival_time, target);
            cameras.restore(&message).unwrap();
            let frame = cameras.state().latest_for(route.camera_id).unwrap();
            let decoded = decode_camera_frame(frame).unwrap();
            assert_eq!((decoded.width, decoded.height), (320, 240));
            let red = 20 + ((target.0 - start.0) / 100_000_000) as u8 * 3;
            let expected = if route.stream.topic.ends_with("mono8") {
                [red, red, red, 255]
            } else {
                [red, 80, 160, 255]
            };
            for pixel in decoded.rgba.chunks_exact(4) {
                for (actual, expected) in pixel.iter().zip(expected) {
                    let tolerance = if route.stream.topic.ends_with("jpeg") {
                        3
                    } else {
                        0
                    };
                    assert!(
                        actual.abs_diff(expected) <= tolerance,
                        "{}: {pixel:?}",
                        route.stream.topic
                    );
                }
            }
            #[cfg(feature = "native-jpeg")]
            {
                let native = viewer_renderer::NativeCameraDecoder::new()
                    .unwrap()
                    .decode_frame(frame, (160, 120))
                    .unwrap();
                assert_eq!(native.geometry.source_size, (320, 240));
                assert_eq!(native.geometry.pixel_size, (160, 120));
                assert!(native.geometry.matches_target((160, 120)));
                assert!(!native.geometry.matches_target((320, 240)));
                assert_eq!(&native.image.rgba[..4], &decoded.rgba[..4]);
            }
        }
    }
    cameras.advance(Duration::ZERO);
    assert_eq!(cameras.counters().errors, 0);
}
fn bindings() -> WorkspaceBindings {
    WorkspaceBindings {
        path_topic: "/planning/path".into(),
        odometry_topic: "/odom".into(),
        point_cloud_topic: "/scan".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
    }
}
