//! Concrete CDR codecs for the ROS messages consumed by viewer features.

mod common;
mod image;
mod laser_scan;
mod odometry;
mod path;
mod transform;

pub use common::DecodeError;
pub use image::{
    CompressedImage, DecodedCompressedImage, decode_compressed_image,
    decode_compressed_image_bytes, encode_compressed_image_cdr,
};
pub use laser_scan::{LaserScan, decode_laser_scan};
pub use odometry::{Odometry, decode_odometry};
pub use path::{PathMessage, decode_path};
pub use transform::{TransformStamped, decode_tf_message, encode_tf_message_cdr};

#[cfg(test)]
mod tests;
