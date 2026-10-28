//! Concrete CDR codecs for the ROS messages consumed by viewer features.

mod image;
mod laser_scan;
mod odometry;
mod path;
mod point_cloud2;
mod transform;
pub(crate) mod wire;

pub use image::{
    CompressedImage, DecodedCompressedImage, DecodedRawImage, decode_compressed_image,
    decode_compressed_image_bytes, decode_raw_image_bytes, encode_compressed_image_cdr,
    encode_raw_image_cdr,
};
pub use laser_scan::{LaserScan, decode_laser_scan, encode_laser_scan_cdr};
pub use odometry::{Odometry, decode_odometry, encode_odometry_cdr};
pub use path::{PathMessage, decode_path, encode_path_cdr};
pub use point_cloud2::{
    DecodedPointCloud2, MAX_POINT_CLOUD2_MESSAGE_BYTES, MAX_POINT_CLOUD2_POINTS,
    decode_point_cloud2,
};
pub use transform::{TransformStamped, decode_tf_message, encode_tf_message_cdr};
pub use wire::DecodeError;

#[cfg(test)]
mod tests;

pub use numeric::{NumericField, NumericFieldExtractor};
pub(crate) mod numeric;

mod diagnostic;
pub use diagnostic::{
    DiagnosticArray, DiagnosticKeyValue, DiagnosticLevel, DiagnosticStatus,
    MAX_DIAGNOSTIC_MESSAGE_BYTES, decode_diagnostic_array,
};

mod detection3d;
pub use detection3d::{
    BoundingBox3D, Detection3D, Detection3DArray, MAX_DETECTION_MESSAGE_BYTES,
    MAX_DETECTION_RESULTS, MAX_DETECTIONS, MessageHeader, ObjectHypothesis,
    ObjectHypothesisWithPose, Pose3D, PoseWithCovariance, decode_detection3d_array,
    encode_detection3d_array_cdr,
};
