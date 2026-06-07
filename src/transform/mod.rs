pub mod cdr;
pub mod pcd;
// pub mod decode;

use crate::input::LoadedPacket;
use anyhow::Result;

// pub enum ParsedMessage{
//     PointCloud2(Vec<pcd::LidarPointVertex),
//     // Image()
//     None,
// }

// pub struct Transformer;

// impl Transformer {
//     fn register_topic(topic : str, )
//     pub fn update(packet: LoadedPacket) -> ParsedMessage{
//         if packet.topic.contains("livox") {
//             if let Ok(vertices) = pcd::parse_livox_custom_msg(packet) {
//                 return ParsedMessage::PointCloud(vertices);
//             }
//         } else if packet.topic.contains("points") || packet.topic.contains("lidar") {
//             if let Ok(vertices) = pcd::parse_pointcloud2(packet) {
//                 return ParsedMessage::PointCloud(vertices);
//             }
//         }
//         ParsedMessage::None
//     }
// }