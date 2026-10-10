//! Concrete ROS Live source planning, bounded admission, and upstream-rclrs adapter.
//!
//! The source-neutral catalog/admission code is built in ordinary tests. ROS native dependencies
//! remain behind `ros2-live`; DynamicMessage-to-CDR conversion is a temporary Phase 1 transport
//! adapter and does not leak beyond `RawMessage`.

mod admission;
mod catalog;
#[cfg(feature = "ros2-live")]
mod dynamic_cdr;

#[cfg(feature = "ros2-live")]
pub(crate) use catalog::{live_catalog, selected_stream_specs};

#[cfg(feature = "ros2-live")]
mod ros {
    use super::{
        admission::{DEFAULT_ORDERED_QUEUE_CAPACITY, LiveAdmission, LiveDiagnostics},
        catalog::{LiveStreamRole, LiveStreamSpec},
        dynamic_cdr::encode_dynamic_message,
    };
    use rclrs::{
        Context, CreateBasicExecutor, DynamicMessage, DynamicSubscription, ExecutorCommands,
        QOS_PROFILE_SENSOR_DATA, QoSDurabilityPolicy, QoSHistoryPolicy, QoSReliabilityPolicy,
        RclrsErrorFilter, SpinOptions, SubscriptionOptions,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::{self, JoinHandle},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use viewer_core::{ArrivalTime, RawMessage};

    pub(crate) struct RosLiveHandle {
        admission: Arc<LiveAdmission>,
        commands: Arc<ExecutorCommands>,
        stop: Arc<AtomicBool>,
        executor_thread: Option<JoinHandle<()>>,
    }

    impl RosLiveHandle {
        pub(crate) fn start(specs: Vec<LiveStreamSpec>, reliable: bool) -> Result<Self, String> {
            let admission = Arc::new(
                LiveAdmission::new(specs.clone(), DEFAULT_ORDERED_QUEUE_CAPACITY)
                    .map_err(|error| error.to_string())?,
            );
            if specs.is_empty() {
                return Err("Live SessionPlan selected no continuous streams".into());
            }
            let context = Context::default_from_env()
                .map_err(|error| format!("initialize ROS 2 context: {error}"))?;
            let mut executor = context.create_basic_executor();
            let commands = Arc::clone(executor.commands());
            let node = executor
                .create_node("mcap_player_viewer")
                .map_err(|error| format!("create ROS 2 Live node: {error}"))?;
            let subscriptions = create_subscriptions(&node, &specs, reliable, &admission)?;
            let stop = Arc::new(AtomicBool::new(false));
            let thread_admission = Arc::clone(&admission);
            let thread_stop = Arc::clone(&stop);
            let executor_thread = thread::Builder::new()
                .name("viewer-ros2-executor".into())
                .spawn(move || {
                    // Node and subscriptions must outlive every executor spin.
                    let _node = node;
                    let _subscriptions = subscriptions;
                    while !thread_stop.load(Ordering::Acquire) && thread_admission.error().is_none()
                    {
                        if let Err(error) = executor
                            .spin(SpinOptions::new().timeout(Duration::from_millis(50)))
                            .timeout_ok()
                            .first_error()
                        {
                            thread_admission.set_source_error(format!("ROS executor: {error}"));
                            break;
                        }
                    }
                })
                .map_err(|error| format!("spawn ROS executor thread: {error}"))?;
            Ok(Self {
                admission,
                commands,
                stop,
                executor_thread: Some(executor_thread),
            })
        }

        pub(crate) fn take_ready(&self) -> Vec<RawMessage> {
            self.admission.take_ready()
        }

        pub(crate) fn diagnostics(&self) -> LiveDiagnostics {
            self.admission.diagnostics()
        }

        pub(crate) fn error(&self) -> Option<String> {
            self.admission.error().map(|error| error.to_string())
        }
    }

    impl Drop for RosLiveHandle {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.commands.halt_spinning();
            if let Some(thread) = self.executor_thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn create_subscriptions(
        node: &rclrs::Node,
        specs: &[LiveStreamSpec],
        reliable: bool,
        admission: &Arc<LiveAdmission>,
    ) -> Result<Vec<DynamicSubscription>, String> {
        let mut subscriptions = Vec::<DynamicSubscription>::with_capacity(specs.len());
        for spec in specs {
            let mut options = SubscriptionOptions::new(&spec.stream.topic);
            options.qos = qos_for(spec.role, reliable);
            let topic_type = spec
                .stream
                .schema
                .as_str()
                .try_into()
                .map_err(|error: rclrs::DynamicMessageError| error.to_string())?;
            let stream_id = spec.stream.id;
            let role = spec.role;
            let callback_admission = Arc::clone(admission);
            let callback = move |message: DynamicMessage, _info: rclrs::MessageInfo| {
                // Arrival is captured before introspection and the Phase 1 CDR reconstruction.
                let Some(arrival_time) = wall_clock_arrival() else {
                    callback_admission.record_adapter_error();
                    return;
                };
                let payload = match encode_dynamic_message(role, &message) {
                    Ok(payload) => payload,
                    Err(_) => {
                        callback_admission.record_adapter_error();
                        return;
                    }
                };
                let copied_bytes = payload.len() as u64;
                let _ = callback_admission.admit(
                    RawMessage {
                        publish_time_ns: None,
                        sequence: None,
                        stream_id,
                        arrival_time,
                        payload: payload.into(),
                    },
                    copied_bytes,
                );
            };
            subscriptions.push(
                node.create_dynamic_subscription(topic_type, options, callback)
                    .map_err(|error| {
                        format!(
                            "subscribe Live stream {} ({}): {error}",
                            spec.stream.topic, spec.stream.schema
                        )
                    })?,
            );
        }
        Ok(subscriptions)
    }

    fn qos_for(role: LiveStreamRole, reliable: bool) -> rclrs::QoSProfile {
        let mut qos = QOS_PROFILE_SENSOR_DATA;
        if role == LiveStreamRole::StaticTf {
            qos.history = QoSHistoryPolicy::KeepLast {
                depth: u32::try_from(DEFAULT_ORDERED_QUEUE_CAPACITY)
                    .expect("Live queue capacity fits QoS depth"),
            };
            qos.reliability = QoSReliabilityPolicy::Reliable;
            qos.durability = QoSDurabilityPolicy::TransientLocal;
        } else if reliable {
            qos.reliability = QoSReliabilityPolicy::Reliable;
        }
        qos
    }

    fn wall_clock_arrival() -> Option<ArrivalTime> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
            .map(ArrivalTime)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sensor_roles_use_sensor_qos_and_reliability_override() {
            assert_eq!(
                qos_for(LiveStreamRole::Camera, false),
                QOS_PROFILE_SENSOR_DATA
            );
            let reliable = qos_for(LiveStreamRole::DynamicTf, true);
            assert_eq!(reliable.reliability, QoSReliabilityPolicy::Reliable);
            assert_eq!(reliable.durability, QoSDurabilityPolicy::Volatile);
        }

        #[test]
        fn static_tf_requests_bounded_transient_local_history() {
            let qos = qos_for(LiveStreamRole::StaticTf, false);
            assert_eq!(qos.reliability, QoSReliabilityPolicy::Reliable);
            assert_eq!(qos.durability, QoSDurabilityPolicy::TransientLocal);
            assert_eq!(
                qos.history,
                QoSHistoryPolicy::KeepLast {
                    depth: DEFAULT_ORDERED_QUEUE_CAPACITY as u32,
                }
            );
        }
    }
}

#[cfg(feature = "ros2-live")]
pub(crate) use ros::RosLiveHandle;

#[cfg(test)]
mod tests {
    use super::{
        admission::LiveAdmission,
        catalog::{LiveStreamRole, LiveStreamSpec},
    };
    use viewer_core::{
        ArrivalTime, CameraController, CameraId, CompressedImage, FeatureRuntime, McapSource,
        MeasurementTime, PathMessage, PlaybackRequirements, RawMessage, SessionPlan, SourceCatalog,
        StreamDescriptor, StreamId, StreamTimingSummary, WorkspaceBindings,
        encode_compressed_image_cdr, encode_path_cdr,
    };

    fn bindings() -> WorkspaceBindings {
        WorkspaceBindings {
            path_topic: "/planning/path".into(),
            odometry_topic: "/odom".into(),
            point_cloud_topic: "/scan".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
        }
    }

    fn standard_requirements() -> PlaybackRequirements {
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        requirements.require_path();
        requirements.require_odometry();
        requirements.require_point_cloud();
        requirements.require_transforms();
        requirements
    }

    fn camera_spec() -> LiveStreamSpec {
        LiveStreamSpec::new(
            &StreamDescriptor {
                schema_definition: None,
                id: StreamId(1),
                topic: "/camera/live/image/compressed".into(),
                schema: LiveStreamRole::Camera.schema().into(),
                message_encoding: "cdr".into(),
                timing: StreamTimingSummary::default(),
            },
            LiveStreamRole::Camera,
        )
    }

    #[test]
    fn current_camera_live_admission_routes_latest_message_to_camera_controller() {
        let spec = camera_spec();
        let stream_id = spec.stream.id;
        let admission = LiveAdmission::new(vec![spec.clone()], 4).unwrap();
        for (arrival, marker) in [(100, 1), (200, 2)] {
            admission
                .admit(
                    RawMessage {
                        publish_time_ns: None,
                        sequence: None,
                        stream_id,
                        arrival_time: ArrivalTime(arrival),
                        payload: encode_compressed_image_cdr(&CompressedImage {
                            measurement_time: MeasurementTime(arrival),
                            frame_id: "camera".into(),
                            format: "jpeg".into(),
                            jpeg: vec![marker],
                        })
                        .unwrap()
                        .into(),
                    },
                    0,
                )
                .unwrap();
        }
        let catalog = SourceCatalog {
            streams: vec![spec.stream],
            time_range: None,
            capabilities: viewer_core::SourceCapabilities::LIVE,
        };
        let mut requirements = PlaybackRequirements::empty();
        requirements.require_all_cameras();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/live/image/compressed",
            &requirements,
            crate::workspace::NativeWorkspace::default().bindings(),
        )
        .unwrap();
        let mut cameras = CameraController::new(&plan);

        cameras.admit(&admission.take_ready()[0]);
        cameras.advance(std::time::Duration::ZERO);

        let latest = cameras.state().latest_for(CameraId(0)).unwrap();
        assert_eq!(latest.arrival_time, ArrivalTime(200));
        assert_eq!(latest.jpeg.as_ref(), [2]);
        assert_eq!(admission.diagnostics().coalesced_messages, 1);
        assert_eq!(cameras.counters().decoded, 1);
    }

    #[test]
    fn admission_and_direct_delivery_reduce_to_the_same_feature_state() {
        let requirements = standard_requirements();
        let bindings = bindings();
        let live_catalog = super::catalog::live_catalog(
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();
        let plan = SessionPlan::build(
            &live_catalog,
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();

        let fixture = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap"),
        )
        .unwrap();
        let mut source = McapSource::from_bytes(fixture.into()).unwrap();
        let recording_catalog = source.catalog().clone();
        let selected_topics = plan.selected_topics();
        let recording_streams = selected_topics
            .iter()
            .filter_map(|topic| recording_catalog.by_topic(topic).map(|stream| stream.id))
            .collect::<Vec<_>>();
        source.select_streams(recording_streams).unwrap();
        let end = source.time_range().end_inclusive();
        let remapped = source
            .read_through(end)
            .unwrap()
            .into_iter()
            .filter_map(|mut message| {
                let topic = &recording_catalog.by_id(message.stream_id)?.topic;
                message.stream_id = live_catalog.by_topic(topic)?.id;
                Some(message)
            })
            .collect::<Vec<_>>();

        let mut direct = FeatureRuntime::new(&plan, true);
        direct.process_messages(std::time::Duration::from_secs(10), &remapped);
        let admission =
            LiveAdmission::new(super::catalog::selected_stream_specs(&plan), 1_024).unwrap();
        for message in remapped {
            admission.admit(message, 0).unwrap();
        }
        let mut admitted = FeatureRuntime::new(&plan, true);
        admitted.process_messages(std::time::Duration::from_secs(10), &admission.take_ready());

        assert_eq!(direct.counters().decoded, admitted.counters().decoded);
        assert_eq!(direct.counters().errors, admitted.counters().errors);
        assert_eq!(direct.counters().unknown_streams, 0);
        assert_eq!(admitted.counters().unknown_streams, 0);
        assert_eq!(
            admission.diagnostics().coalesced_messages,
            direct.counters().dropped
        );
        assert_eq!(
            direct.cameras().state().latest_for(CameraId(0)).cloned(),
            admitted.cameras().state().latest_for(CameraId(0)).cloned()
        );
        assert_eq!(
            direct.path().state().latest(),
            admitted.path().state().latest()
        );
        assert_eq!(
            direct.odometry().state().latest(),
            admitted.odometry().state().latest()
        );
        assert_eq!(
            direct.scene().unwrap().point_cloud().latest(),
            admitted.scene().unwrap().point_cloud().latest()
        );
        assert_eq!(
            direct.transforms().state().static_len(),
            admitted.transforms().state().static_len()
        );
        assert_eq!(
            direct.transforms().state().dynamic_len(),
            admitted.transforms().state().dynamic_len()
        );
    }

    #[test]
    fn malformed_semantic_message_does_not_hide_the_prior_valid_message() {
        let requirements = standard_requirements();
        let bindings = bindings();
        let catalog = super::catalog::live_catalog(
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();
        let path_stream = plan.path_stream().unwrap().id;
        let admission =
            LiveAdmission::new(super::catalog::selected_stream_specs(&plan), 4).unwrap();
        admission
            .admit(
                RawMessage {
                    publish_time_ns: None,
                    sequence: None,
                    stream_id: path_stream,
                    arrival_time: ArrivalTime(10),
                    payload: encode_path_cdr(&PathMessage {
                        measurement_time: MeasurementTime(10),
                        frame_id: "map".into(),
                        points: vec![[1.0, 2.0]],
                    })
                    .unwrap()
                    .into(),
                },
                0,
            )
            .unwrap();
        admission
            .admit(
                RawMessage {
                    publish_time_ns: None,
                    sequence: None,
                    stream_id: path_stream,
                    arrival_time: ArrivalTime(20),
                    payload: vec![0xff].into(),
                },
                0,
            )
            .unwrap();
        let mut runtime = FeatureRuntime::new(&plan, true);
        runtime.process_messages(std::time::Duration::ZERO, &admission.take_ready());

        let latest = runtime.path().state().latest().unwrap();
        assert_eq!(latest.arrival_time, ArrivalTime(10));
        assert_eq!(latest.points, [[-2.0, 1.0]]);
        assert_eq!(runtime.path().counters().errors, 1);
    }

    #[cfg(feature = "ros2-live")]
    #[test]
    #[ignore = "requires a sourced ROS 2 installation"]
    fn dropping_ros_handle_stops_and_joins_the_executor() {
        drop(super::RosLiveHandle::start(vec![camera_spec()], false).unwrap());
    }

    #[cfg(feature = "ros2-live")]
    #[test]
    #[ignore = "requires a sourced ROS 2 Jazzy environment and ros2 bag"]
    fn rosbag_multi_stream_dynamic_smoke_and_reopen() {
        use std::{collections::BTreeSet, process::Command, thread, time::Instant};

        let requirements = standard_requirements();
        let bindings = bindings();
        let catalog = super::catalog::live_catalog(
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();
        let plan = SessionPlan::build(
            &catalog,
            "/camera/front/image/compressed",
            &requirements,
            &bindings,
        )
        .unwrap();
        let specs = super::catalog::selected_stream_specs(&plan);

        drop(super::RosLiveHandle::start(specs.clone(), false).unwrap());
        let source = super::RosLiveHandle::start(specs.clone(), false).unwrap();
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/camera-jpeg/camera_7_5s.mcap");
        let qos_overrides = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/ros2-live-qos.yaml");
        let mut player = Command::new("ros2")
            .args([
                "bag",
                "play",
                "--input",
                fixture.to_str().unwrap(),
                "mcap",
                "--qos-profile-overrides-path",
                qos_overrides.to_str().unwrap(),
                "--delay",
                "2",
                "--rate",
                "5",
                "--disable-keyboard-controls",
            ])
            .spawn()
            .unwrap();

        let expected = specs
            .iter()
            .map(|spec| spec.stream.id)
            .collect::<BTreeSet<_>>();
        let deadline = Instant::now() + std::time::Duration::from_secs(12);
        let mut seen = BTreeSet::new();
        let mut runtime = FeatureRuntime::new(&plan, true);
        while Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(50));
            if let Some(error) = source.error() {
                let _ = player.kill();
                let _ = player.wait();
                panic!("Live source failed during replay: {error}");
            }
            let ready = source.take_ready();
            seen.extend(ready.iter().map(|message| message.stream_id));
            runtime.process_messages(std::time::Duration::from_millis(50), &ready);
            if expected.is_subset(&seen) && player.try_wait().unwrap().is_some() {
                break;
            }
        }
        if player.try_wait().unwrap().is_none() {
            player.kill().unwrap();
            player.wait().unwrap();
        }
        let ready = source.take_ready();
        seen.extend(ready.iter().map(|message| message.stream_id));
        runtime.process_messages(std::time::Duration::from_millis(50), &ready);

        assert_eq!(seen, expected);
        assert!(runtime.cameras().state().latest_for(CameraId(0)).is_some());
        assert!(runtime.path().state().latest().is_some());
        assert!(runtime.odometry().state().latest().is_some());
        assert!(runtime.scene().unwrap().point_cloud().latest().is_some());
        assert!(runtime.transforms().state().static_len() > 0);
        assert!(runtime.transforms().state().dynamic_len() > 0);
        assert_eq!(runtime.counters().errors, 0);
        let diagnostics = source.diagnostics();
        assert_eq!(diagnostics.adapter_errors, 0);
        assert!(diagnostics.payload_copy_bytes > 0);
        assert!(diagnostics.coalesced_messages > 0);
        assert!(
            diagnostics.queue_high_water_mark
                <= 1 + super::admission::DEFAULT_ORDERED_QUEUE_CAPACITY * 5
        );
    }
}
