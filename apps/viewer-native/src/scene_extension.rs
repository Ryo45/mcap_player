//! Presentation-owned extension generation; never mutates playback or feature truth.
use viewer_core::{PathState, SceneOverlay, TransformState};
use viewer_extension::VisualizationExtension;

pub(crate) struct SceneExtensionState {
    extension: Box<dyn VisualizationExtension>,
    input_revision: Option<u64>,
    placement_key: Option<(u64, String)>,
    placement_valid: bool,
    output: SceneOverlay,
    placed: SceneOverlay,
    revision: u64,
    error: Option<String>,
    failed: bool,
}

impl SceneExtensionState {
    pub(crate) fn new(extension: Box<dyn VisualizationExtension>) -> Self {
        Self {
            extension,
            input_revision: None,
            placement_key: None,
            placement_valid: false,
            output: SceneOverlay::default(),
            placed: SceneOverlay::default(),
            revision: 0,
            error: None,
            failed: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.input_revision = None;
        self.placement_key = None;
        self.placement_valid = false;
        self.output = SceneOverlay::default();
        self.set_placed(SceneOverlay::default());
        let result = self.extension.reset();
        self.failed = result.is_err();
        self.error = result.err().map(|e| e.to_string());
    }

    pub(crate) fn update(
        &mut self,
        path: &PathState,
        transforms: &TransformState,
        fixed_frame: &str,
    ) {
        if self.failed {
            return;
        }
        if self.input_revision != Some(path.revision()) {
            self.input_revision = Some(path.revision());
            self.placement_key = None;
            self.placement_valid = false;
            self.error = None;
            self.output = SceneOverlay::default();
            if let Some(frame) = path.latest() {
                match self.extension.update(frame) {
                    Ok(output) => self.output = output,
                    Err(error) => {
                        self.set_placed(SceneOverlay::default());
                        let error = format!("{}: {error}", self.extension.descriptor().id);
                        log::warn!("Scene extension disabled until seek/source reset: {error}");
                        self.error = Some(error);
                        self.failed = true;
                        return;
                    }
                }
            } else {
                self.set_placed(SceneOverlay::default());
            }
        }
        if self
            .placement_key
            .as_ref()
            .is_some_and(|(revision, frame)| {
                frame == fixed_frame && (self.placement_valid || *revision == transforms.revision())
            })
        {
            return;
        }
        self.placement_key = Some((transforms.revision(), fixed_frame.to_owned()));
        self.placement_valid = false;
        let Some(frame) = path.latest() else {
            return;
        };
        // Snapshot geometry is frozen at acquisition time, not frame-locked. Do not lose a
        // successfully placed overlay when bounded TF history later evicts that old time.
        if self.output.lines().is_empty() && self.output.boxes().is_empty() {
            self.error = None;
            self.set_placed(SceneOverlay::default());
            self.placement_valid = true;
            return;
        }
        let Some(transform) =
            transforms.resolve_at(&frame.frame_id, fixed_frame, frame.measurement_time)
        else {
            self.set_placed(SceneOverlay::default());
            self.error = Some(format!("missing TF {} → {fixed_frame}", frame.frame_id));
            return;
        };
        match self.output.transformed(transform) {
            Ok(placed) => {
                self.error = None;
                self.set_placed(placed);
                self.placement_valid = true;
            }
            Err(error) => {
                self.set_placed(SceneOverlay::default());
                self.error = Some(error.to_string());
            }
        }
    }

    fn set_placed(&mut self, placed: SceneOverlay) {
        if self.placed != placed {
            self.placed = placed;
            self.revision = self.revision.wrapping_add(1);
        }
    }
    pub(crate) fn overlay(&self) -> &SceneOverlay {
        &self.placed
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    use viewer_core::{ArrivalTime, MeasurementTime, PathFrame, SceneLine, SceneObjectId};
    use viewer_extension::ExtensionDescriptor;

    struct RustLine {
        descriptor: ExtensionDescriptor,
        calls: Rc<Cell<usize>>,
        resets: Rc<Cell<usize>>,
        fail: bool,
    }
    impl VisualizationExtension for RustLine {
        fn descriptor(&self) -> &ExtensionDescriptor {
            &self.descriptor
        }
        fn reset(&mut self) -> anyhow::Result<()> {
            self.resets.set(self.resets.get() + 1);
            Ok(())
        }
        fn update(&mut self, frame: &PathFrame) -> anyhow::Result<SceneOverlay> {
            self.calls.set(self.calls.get() + 1);
            anyhow::ensure!(!self.fail, "test trap");
            let p = frame.points[0].map(|v| v as f32);
            Ok(SceneOverlay::new(
                vec![SceneLine {
                    id: SceneObjectId(7),
                    start: p,
                    end: [p[0], p[1], p[2] + 1.],
                    color: [1., 0., 0.],
                }],
                vec![],
            )?)
        }
    }
    fn state(fail: bool) -> (SceneExtensionState, Rc<Cell<usize>>, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        let resets = Rc::new(Cell::new(0));
        let extension = RustLine {
            descriptor: ExtensionDescriptor {
                id: "native-test".into(),
                api_version: 2,
                topic: "/planning/path".into(),
                schema: "nav_msgs/msg/Path".into(),
                input: "path_positions_v1".into(),
            },
            calls: calls.clone(),
            resets: resets.clone(),
            fail,
        };
        (SceneExtensionState::new(Box::new(extension)), calls, resets)
    }
    fn apply(path: &mut PathState, x: f64, time: i64) {
        path.apply(PathFrame {
            arrival_time: ArrivalTime(time),
            measurement_time: MeasurementTime(time),
            frame_id: "odom".into(),
            points: vec![[x, 0., 0.]],
        });
    }
    #[test]
    fn rust_extension_reuses_unchanged_output_and_resets_seek_source_generations() {
        let (mut extension, calls, resets) = state(false);
        let tf = TransformState::default();
        let mut path = PathState::default();
        apply(&mut path, 1., 1);
        extension.update(&path, &tf, "odom");
        assert_eq!(calls.get(), 1);
        assert_eq!(extension.overlay().lines()[0].start[0], 1.);
        let revision = extension.revision();
        for _ in 0..100 {
            extension.update(&path, &tf, "odom");
        }
        assert_eq!(calls.get(), 1);
        assert_eq!(extension.revision(), revision);
        apply(&mut path, 1., 2); // New message, identical guest geometry: no GPU upload revision.
        extension.update(&path, &tf, "odom");
        assert_eq!(calls.get(), 2);
        assert_eq!(extension.revision(), revision);
        apply(&mut path, 3., 3);
        extension.update(&path, &tf, "odom");
        assert_eq!(extension.overlay().lines()[0].start[0], 3.);
        // Committed backward seek: a fresh feature candidate can reuse low revisions.
        extension.reset();
        path = PathState::default();
        apply(&mut path, -1., 1);
        extension.update(&path, &tf, "odom");
        assert_eq!(extension.overlay().lines()[0].start[0], -1.);
        // Committed source replacement with no predecessor must remove old objects.
        extension.reset();
        extension.update(&PathState::default(), &tf, "odom");
        assert!(extension.overlay().lines().is_empty());
        assert_eq!(resets.get(), 2);
    }
    #[test]
    fn missing_tf_clears_geometry_without_reexecuting_guest() {
        let (mut extension, calls, _) = state(false);
        let tf = TransformState::default();
        let mut path = PathState::default();
        apply(&mut path, 1., 1);
        extension.update(&path, &tf, "odom");
        assert_eq!(extension.overlay().lines().len(), 1);
        extension.update(&path, &tf, "missing");
        assert!(extension.overlay().lines().is_empty());
        assert!(extension.error().unwrap().contains("missing TF"));
        extension.update(&path, &tf, "odom");
        assert_eq!(extension.overlay().lines().len(), 1);
        assert_eq!(calls.get(), 1);
    }
    #[test]
    fn acquisition_pose_survives_tf_history_eviction_and_missing_tf_retries() {
        use viewer_core::{TransformBatch, TransformStamped};
        let (mut extension, calls, _) = state(false);
        let mut path = PathState::default();
        path.apply(PathFrame {
            arrival_time: ArrivalTime(1),
            measurement_time: MeasurementTime(1),
            frame_id: "robot".into(),
            points: vec![[1., 0., 0.]],
        });
        let mut tf = TransformState::default();
        extension.update(&path, &tf, "odom");
        assert!(extension.error().is_some());
        let batch = |time, x| TransformBatch {
            arrival_time: ArrivalTime(time),
            is_static: false,
            transforms: vec![TransformStamped {
                measurement_time: MeasurementTime(time),
                frame_id: "odom".into(),
                child_frame_id: "robot".into(),
                translation: [x, 0., 0.],
                rotation: [0., 0., 0., 1.],
            }],
        };
        tf.apply(batch(1, 10.));
        extension.update(&path, &tf, "odom");
        assert_eq!(extension.overlay().lines()[0].start[0], 11.);
        let revision = extension.revision();
        tf.apply(batch(3_000_000_000, 20.));
        assert!(
            tf.transform_points_at("robot", "odom", MeasurementTime(1), &[[1., 0., 0.]])
                .is_none()
        );
        extension.update(&path, &tf, "odom");
        assert_eq!(extension.overlay().lines()[0].start[0], 11.);
        assert_eq!(extension.revision(), revision);
        assert_eq!(calls.get(), 1);
        assert!(extension.error().is_none());
    }

    #[test]
    fn extension_failure_is_latched_without_mutating_feature_truth() {
        let (mut extension, calls, _) = state(true);
        let mut path = PathState::default();
        apply(&mut path, 1., 1);
        let revision = path.revision();
        extension.update(&path, &TransformState::default(), "odom");
        assert!(extension.overlay().lines().is_empty());
        assert!(extension.error().unwrap().contains("test trap"));
        extension.update(&path, &TransformState::default(), "odom");
        assert_eq!(calls.get(), 1);
        assert_eq!(path.revision(), revision);
        extension.reset();
        extension.update(&path, &TransformState::default(), "odom");
        assert_eq!(calls.get(), 2);
    }
}

#[cfg(test)]
mod recording_tests {
    use super::*;
    use crate::{
        session::ViewerSession,
        workspace::{NativeWorkspace, WorkspaceLayout},
    };
    use viewer_core::{FeatureRestoreError, FeatureRestoreErrorKind, PlaybackCommand, StreamId};

    #[test]
    #[ignore = "build independent path-boxes guest first"]
    fn actual_wasm_indexed_seek_and_failed_restore_preserve_transaction() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let recording = root.join("target/scene-extension-independent/staggered/recording.mcap");
        let mut workspace = NativeWorkspace::load_bundled_or_fallback(WorkspaceLayout::Scene);
        workspace
            .enable_scene_extension("/perception/objects")
            .unwrap();
        let requirements = workspace.data_requirements();
        let mut session = ViewerSession::open(
            &recording,
            "".into(),
            &requirements.playback,
            workspace.bindings(),
        )
        .unwrap();
        workspace.configure_session(session.plan());
        let guest =
            viewer_extension::WasmExtension::load(&root.join("examples/path-boxes/extension.json"))
                .unwrap();
        let mut overlay = SceneExtensionState::new(Box::new(guest));
        let display_stream = session.plan().path_stream().unwrap().id;
        let input_stream = session
            .plan()
            .path_stream_for_topic("/perception/objects")
            .unwrap()
            .id;
        assert_ne!(display_stream, input_stream);
        assert!(session.plan().selected_stream_ids().contains(&input_stream));
        let reference = viewer_mcap::McapSource::open_file(&recording).unwrap();
        let view = session.playback_view().unwrap();
        let first_path_time = viewer_mcap::FileMessageStream::open(&recording)
            .unwrap()
            .find_map(|message| {
                let message = message.unwrap();
                (message.channel.topic == "/planning/path").then_some(viewer_core::ArrivalTime(
                    i64::try_from(message.log_time).unwrap(),
                ))
            })
            .unwrap();
        for target in [view.end, first_path_time, view.start, view.end] {
            session
                .apply_playback_command(PlaybackCommand::Seek(target), |target, messages| {
                    workspace.restore_messages(target, messages)
                })
                .unwrap();
            overlay.reset();
            let decoded = workspace.counters().decoded;
            overlay.update(
                workspace.extension_path().unwrap().state(),
                workspace.transforms().state(),
                "odom",
            );
            assert!(overlay.error().is_none(), "{:?}", overlay.error());
            assert_eq!(
                overlay.overlay().boxes().is_empty(),
                workspace
                    .extension_path()
                    .unwrap()
                    .state()
                    .latest()
                    .is_none_or(|frame| frame.points.is_empty())
            );
            assert_eq!(
                workspace.counters().decoded,
                decoded,
                "guest must share host decode"
            );
            let expected = reference
                .latest_before(&[input_stream], target)
                .unwrap()
                .messages;
            let actual = workspace.extension_path().unwrap().state().latest();
            assert_eq!(
                actual.map(|frame| frame.arrival_time),
                expected.last().map(|message| message.arrival_time)
            );
            if let (Some(input), Some(display)) = (actual, workspace.path().state().latest()) {
                assert_eq!(input.frame_id, display.frame_id);
                assert_ne!(input.arrival_time, display.arrival_time);
                assert!((input.points[0][1] - display.points[0][1] - 2.).abs() < 1e-8);
                assert_eq!(
                    overlay.overlay().boxes()[0].id,
                    viewer_core::SceneObjectId(0)
                );
                let expected_center = workspace
                    .transforms()
                    .state()
                    .resolve_at(&input.frame_id, "odom", input.measurement_time)
                    .unwrap()
                    .transform_point([
                        input.points[0][0] as f32,
                        input.points[0][1] as f32,
                        input.points[0][2] as f32 + 0.25,
                    ]);
                assert!(
                    overlay.overlay().boxes()[0]
                        .center
                        .iter()
                        .zip(expected_center)
                        .all(|(a, b)| (*a - b).abs() < 1e-5)
                );
            }
            assert_eq!(session.playback_view().unwrap().cursor, target);
        }
        let previous = overlay.overlay().boxes().to_vec();
        let cursor = session.playback_view().unwrap().cursor;
        let feature_revision = workspace.path().state().revision();
        let input_revision = workspace.extension_path().unwrap().state().revision();
        assert!(
            session
                .apply_playback_command(PlaybackCommand::Seek(view.start), |target, _| Err(
                    FeatureRestoreError {
                        stream_id: StreamId(u32::MAX),
                        arrival_time: target,
                        kind: FeatureRestoreErrorKind::Unrouted
                    }
                ))
                .is_err()
        );
        assert_eq!(session.playback_view().unwrap().cursor, cursor);
        assert_eq!(workspace.path().state().revision(), feature_revision);
        assert_eq!(
            workspace.extension_path().unwrap().state().revision(),
            input_revision
        );
        overlay.update(
            workspace.extension_path().unwrap().state(),
            workspace.transforms().state(),
            "odom",
        );
        assert_eq!(overlay.overlay().boxes(), previous);
        // Source commit clears output even when the next source reuses a Path revision.
        overlay.reset();
        assert!(overlay.overlay().boxes().is_empty());
        overlay.update(&PathState::default(), &TransformState::default(), "odom");
        assert!(overlay.overlay().boxes().is_empty());
    }
}
