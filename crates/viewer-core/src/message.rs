//! Canonical serialized messages, time types, and concrete ROS CDR codecs.

pub(crate) mod cdr;
pub(crate) mod schema;
mod time;
pub use schema::RosMessageSchema;

pub use cdr::{
    CompressedImage, DecodeError, DecodedCompressedImage, DecodedPointCloud2, DecodedRawImage,
    LaserScan, MAX_POINT_CLOUD2_MESSAGE_BYTES, MAX_POINT_CLOUD2_POINTS, Odometry, PathMessage,
    TransformStamped, decode_compressed_image, decode_compressed_image_bytes, decode_laser_scan,
    decode_odometry, decode_path, decode_point_cloud2, decode_raw_image_bytes, decode_tf_message,
    encode_compressed_image_cdr, encode_laser_scan_cdr, encode_odometry_cdr, encode_path_cdr,
    encode_raw_image_cdr, encode_tf_message_cdr,
};
pub use time::{ArrivalTime, MeasurementTime};

pub use cdr::{NumericField, NumericFieldExtractor};

pub use cdr::{
    DiagnosticArray, DiagnosticKeyValue, DiagnosticLevel, DiagnosticStatus,
    MAX_DIAGNOSTIC_MESSAGE_BYTES, decode_diagnostic_array,
};

pub use cdr::{
    BoundingBox3D, Detection3D, Detection3DArray, MAX_DETECTION_MESSAGE_BYTES,
    MAX_DETECTION_RESULTS, MAX_DETECTIONS, MessageHeader, ObjectHypothesis,
    ObjectHypothesisWithPose, Pose3D, PoseWithCovariance, decode_detection3d_array,
    encode_detection3d_array_cdr,
};

pub use cdr::{
    FieldDecodeBudget, FieldDecodeLimits, MessageScalar, ScalarFieldExtractor,
    SelectedFieldExtractor, SelectedValue,
};
