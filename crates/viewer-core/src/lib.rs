#![forbid(unsafe_code)]

//! Source-neutral recording contracts, playback planning, and feature-controller state.
//!
//! Internal modules follow the data flow instead of exposing one flat implementation namespace:
//!
//! - `message`: time primitives and concrete ROS CDR codecs;
//! - `recording`: catalogs, requirements, restore plans, and serialized records;
//! - `playback`: clocks, bounded windows, and stage timing;
//! - `feature`: concrete continuous state, reducers, and transactional runtime ownership;
//! - `derived`: bounded Plot/Preview products and bookmark artifacts;
//! - `presentation`: CPU-visible presentation state and frontend-facing snapshots.
//!
//! Public consumers continue to use the explicit root exports below. The internal modules stay
//! private so their layout describes ownership without becoming a second supported API surface.

mod derived;
mod feature;
mod message;
mod playback;
mod presentation;
mod recording;

pub use derived::{
    Bookmark, BookmarkDocument, BookmarkId, BookmarkValidationError,
    CURRENT_BOOKMARK_SCHEMA_VERSION, CURRENT_PREVIEW_SCHEMA_VERSION, CameraPreviewFrame,
    DataFidelity, LoadedOdometrySignals, LoadedSignal, PlotSeries, PreviewBudget, PreviewBuildInfo,
    PreviewImageEncoding, PreviewRequest, PreviewSnapshot, PreviewValidationError, SignalBucket,
    SignalFidelity, SignalId, SignalOverview, SignalOverviewReducer, SignalSample,
    SourceFingerprint, SourceFingerprintValidationError, TimeRange, TimedPosition2,
    arrival_time_from_plot_x, cursor_seconds, merge_signal_buckets,
};
pub use feature::{
    CameraController, CameraFrame, CameraFrameMetadata, CameraId, CameraState, CameraStatus,
    DYNAMIC_TF_HISTORY, FeatureRestoreError, FeatureRestoreErrorKind, FeatureRuntime,
    OdometryController, PathController, PathFrame, PathState, PlaybackPerformance,
    PointCloudAttributes, PointCloudController, PointCloudFrame, PointCloudState,
    ProcessingCounters, TelemetryFrame, TelemetryState, TransformBatch, TransformController,
    TransformState,
};
pub use message::{
    ArrivalTime, CompressedImage, DecodeError, DecodedCompressedImage, DecodedPointCloud2,
    LaserScan, MAX_POINT_CLOUD2_MESSAGE_BYTES, MAX_POINT_CLOUD2_POINTS, MeasurementTime, Odometry,
    PathMessage, TransformStamped, decode_compressed_image, decode_compressed_image_bytes,
    decode_laser_scan, decode_odometry, decode_path, decode_point_cloud2, decode_tf_message,
    encode_compressed_image_cdr, encode_laser_scan_cdr, encode_odometry_cdr, encode_path_cdr,
    encode_tf_message_cdr,
};
pub use playback::{
    DataWindowError, DataWindowTimeRange, FetchDemand, FetchIntent, FetchPlanner, FetchProfile,
    MemoryWindowStore, PlaybackClock, PlaybackCommand, PlaybackEffect, PlaybackLoadState,
    PlaybackSpeed, PlaybackView, SerializedWindow, StageTiming,
};
pub use presentation::{
    BevSnapshot, CalibrationError, CameraCalibration, CameraCalibrationSet, CameraPresentation,
    DiagnosticsPresentation, FrameTransformError, OverlayStatus, PathProjectionPolicy,
    PointCloudColorMode, PointCloudColoring, PresentationMetrics, PresentationSnapshot,
    ProjectedPlan, ProjectionError, SceneDiagnostics, ScenePresentationInput,
    ScenePresentationState, SceneSnapshot, TelemetryPresentation, ViewerPresentation,
    ViewerPresentationInput,
};
pub use recording::{
    CameraRoute, PlaybackRequirements, RawMessage, RecordingTimeRange, RestoreInput, RestorePlan,
    RestorePlanError, RestorePlanner, RestoreRead, RestoreSemantics, SchemaDefinition, SessionPlan,
    SessionPlanError, SourceCapabilities, SourceCatalog, StreamDescriptor, StreamId,
    StreamTimingSummary, WorkspaceBindings,
};

pub use message::{NumericField, NumericFieldExtractor, RosMessageSchema};

pub use recording::{SchemaExplorationError, TopicTree, TopicTreeEntry};

mod inspection;
pub use inspection::{
    INSPECTOR_MAX_PAYLOAD_BYTES, INSPECTOR_PAGE_FIELDS, InspectorContainer, InspectorDecodeError,
    InspectorField, InspectorHeaderStamp, InspectorMessage, InspectorPage, InspectorPageCursor,
    InspectorScalar, InspectorSchema, InspectorValue,
};

pub use message::{
    DiagnosticArray, DiagnosticKeyValue, DiagnosticLevel, DiagnosticStatus,
    MAX_DIAGNOSTIC_MESSAGE_BYTES, decode_diagnostic_array,
};

mod health;
pub use health::{
    DiagnosticAggregator, DiagnosticEvent, DiagnosticsReduceError, DiagnosticsSnapshot,
    TopicTimingFacts, summarize_topic_timing,
};

mod platform_geometry;
pub use platform_geometry::{PlatformGeometry, PlatformGeometryError};
