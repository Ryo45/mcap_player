//! Concrete CDR codecs for the ROS messages consumed by viewer features.

mod image;
mod laser_scan;
mod odometry;
mod path;
mod transform;
pub(crate) mod wire;

pub use image::{
    CompressedImage, DecodedCompressedImage, decode_compressed_image,
    decode_compressed_image_bytes, encode_compressed_image_cdr,
};
pub use laser_scan::{LaserScan, decode_laser_scan, encode_laser_scan_cdr};
pub use odometry::{Odometry, decode_odometry, encode_odometry_cdr};
pub use path::{PathMessage, decode_path, encode_path_cdr};
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
