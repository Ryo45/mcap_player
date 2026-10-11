//! Canonical serialized messages, time types, and concrete ROS CDR codecs.

pub(crate) mod cdr;
mod time;

pub use cdr::{
    CompressedImage, DecodeError, DecodedCompressedImage, LaserScan, Odometry, PathMessage,
    TransformStamped, decode_compressed_image, decode_compressed_image_bytes, decode_laser_scan,
    decode_odometry, decode_path, decode_tf_message, encode_compressed_image_cdr,
    encode_laser_scan_cdr, encode_odometry_cdr, encode_path_cdr, encode_tf_message_cdr,
};
pub use time::{ArrivalTime, MeasurementTime};

pub use cdr::{NumericField, NumericFieldExtractor, RosMessageSchema};

pub use cdr::{
    DiagnosticArray, DiagnosticKeyValue, DiagnosticLevel, DiagnosticStatus,
    MAX_DIAGNOSTIC_MESSAGE_BYTES, decode_diagnostic_array,
};
