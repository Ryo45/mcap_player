// Cargo.toml should include at least:
// anyhow = "1.0"
// byteorder = "1.4"
// bytemuck = "1.17"
// thiserror = "1.0"
// (optionally mcap crate for real MCAP reading)

use anyhow::{Result, anyhow};
use byteorder::{LittleEndian, ReadBytesExt};
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::fmt;
use bytemuck::{Pod, Zeroable};

/// ----------------- Domain types -----------------
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LidarPointVertex {
    pub position: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug)]
pub struct LivoxPacket {
    pub header_seq: u32,
    pub header_stamp_sec: u32,
    pub header_stamp_nanosec: u32,
    pub frame_id: String,
    pub timebase: u64,
    pub point_num: u32,
    pub lidar_id: u8,
    pub points: Vec<LivoxPoint>,
}

#[derive(Debug)]
pub struct LivoxPoint {
    pub offset_time: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub reflectivity: u8,
    pub tag: u8,
    pub line: u8,
}

/// DecodedMessage enum: returns from decode pipeline
#[derive(Debug)]
pub enum DecodedMessage {
    Livox(LivoxPacket),
    // Add other message types (Image, Odom, PointCloud2, ...) as needed
}

/// Schema enum for dispatching
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Schema {
    Livox,
    // Image, Odom, PointCloud2, ...
}

impl fmt::Display for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Schema::Livox => write!(f, "Livox"),
        }
    }
}

/// ----------------- CDR Reader (simple, LE-first) -----------------
/// This is a pragmatic CDR reader: it respects alignment on primitive reads.
/// It assumes little-endian encoding (typical for ROS2 on x86). If you need BE
/// detection, add detection of endianness byte.
pub struct CdrReader<'a> {
    cursor: Cursor<&'a [u8]>,
}

impl<'a> CdrReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { cursor: Cursor::new(buf) }
    }

    fn pos(&self) -> u64 { self.cursor.position() }

    fn align(&mut self, align: u64) -> Result<()> {
        let p = self.pos();
        if p % align != 0 {
            let pad = align - (p % align);
            self.cursor.seek(SeekFrom::Current(pad as i64))?;
        }
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.cursor.read_u8()?)
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        self.align(4)?;
        Ok(self.cursor.read_u32::<LittleEndian>()?)
    }

    pub fn read_u64(&mut self) -> Result<u64> {
        self.align(8)?;
        Ok(self.cursor.read_u64::<LittleEndian>()?)
    }

    pub fn read_f32(&mut self) -> Result<f32> {
        self.align(4)?;
        Ok(self.cursor.read_f32::<LittleEndian>()?)
    }

    /// read CDR string (lengthed with u32 including trailing null)
    pub fn read_string(&mut self) -> Result<String> {
        let len = self.read_u32()?; // includes trailing '\0'
        if len == 0 {
            return Ok(String::new());
        }
        // read len bytes, but last is null terminator
        let mut buf = vec![0u8; (len) as usize];
        self.cursor.read_exact(&mut buf)?;
        // remove trailing null if present
        if let Some(&0) = buf.last() {
            buf.pop();
        }
        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    /// read raw bytes (no alignment)
    pub fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        self.cursor.read_exact(&mut v)?;
        Ok(v)
    }

    pub fn remaining_len(&self) -> usize {
        let pos = self.pos() as usize;
        self.cursor.get_ref().len().saturating_sub(pos)
    }

    pub fn seek(&mut self, pos: u64) -> Result<()> {
        self.cursor.seek(SeekFrom::Start(pos))?;
        Ok(())
    }

    pub fn position(&self) -> u64 { self.pos() }
}

/// ----------------- Livox decoder -----------------
/// This expects 'payload' to be the ROS2 CDR-encoded message body (the message payload bytes).
/// Many recorders write exactly that as MCAP payload. We decode header fields via CDR rules,
/// then parse the point array which is typically packed (19 bytes per point) or with 1-byte padding (20).
pub fn decode_livox_from_cdr(payload: &[u8]) -> Result<LivoxPacket> {
    let mut cr = CdrReader::new(payload);

    // read std_msgs/Header (seq, stamp.sec, stamp.nanosec, frame_id)
    let header_seq = cr.read_u32()?;
    let header_stamp_sec = cr.read_u32()?;
    let header_stamp_nanosec = cr.read_u32()?;
    let frame_id = cr.read_string()?;

    // Now read timebase (uint64) - some livox variants split into two u32s; handle both possibilities
    // Try reading u64 aligned; if remaining < 8, fallback to two u32s combination
    let timebase = if cr.remaining_len() >= 8 {
        cr.read_u64()?
    } else {
        // fallback - try two u32s
        let lo = cr.read_u32().unwrap_or(0) as u64;
        let hi = cr.read_u32().unwrap_or(0) as u64;
        (hi << 32) | lo
    };

    // point_num
    let point_num = cr.read_u32()?;

    // lidar_id (uint8) + reserved 3 bytes
    let lidar_id = cr.read_u8()?;
    // read reserved bytes - depending on alignment these may be padded; safe to read 3 raw bytes
    // Note: do NOT call read_u32 because it will align again.
    let mut rsv = [0u8; 3];
    cr.cursor.read_exact(&mut rsv)?;

    // Now we are at the start of points region.
    let mut points = Vec::with_capacity(point_num as usize);

    let remaining = cr.remaining_len();
    // compute expected step by dividing remaining bytes by point_num
    let step = if point_num > 0 {
        let step_calc = remaining as u64 / point_num as u64;
        step_calc as usize
    } else { 0 };

    // Accept either 19 or 20 (or other) but be defensive.
    if point_num > 0 && (step == 19 || step == 20) {
        // read per-point
        for _ in 0..point_num {
            // read raw fields in little-endian order
            let offset_time = cr.cursor.read_u32::<LittleEndian>()?;
            let x = cr.cursor.read_f32::<LittleEndian>()?;
            let y = cr.cursor.read_f32::<LittleEndian>()?;
            let z = cr.cursor.read_f32::<LittleEndian>()?;
            let reflectivity = cr.cursor.read_u8()?;
            let tag = cr.cursor.read_u8()?;
            let line = cr.cursor.read_u8()?;
            if step == 20 {
                // skip padding byte
                let _pad = cr.cursor.read_u8()?;
            }
            points.push(LivoxPoint {
                offset_time,
                x,
                y,
                z,
                reflectivity,
                tag,
                line,
            });
        }
    } else {
        // fallback: try reading until exhaustion using best-effort parsing (19-byte sliding)
        // This is slower but more robust for malformed variations.
        let mut bytes_left = cr.remaining_len();
        while bytes_left >= 19 {
            let offset_time = cr.cursor.read_u32::<LittleEndian>()?;
            let x = cr.cursor.read_f32::<LittleEndian>()?;
            let y = cr.cursor.read_f32::<LittleEndian>()?;
            let z = cr.cursor.read_f32::<LittleEndian>()?;
            let reflectivity = cr.cursor.read_u8()?;
            let tag = cr.cursor.read_u8()?;
            let line = cr.cursor.read_u8()?;
            points.push(LivoxPoint {
                offset_time, x, y, z, reflectivity, tag, line,
            });
            bytes_left = cr.remaining_len();
            // if there's a single padding byte between points, try to detect it:
            // if bytes_left % 19 != 0 && bytes_left % 20 == 0 we may attempt to step by 1
            // (left as future improvement)
        }
    }

    Ok(LivoxPacket {
        header_seq,
        header_stamp_sec,
        header_stamp_nanosec,
        frame_id,
        timebase,
        point_num,
        lidar_id,
        points,
    })
}

/// Convert LivoxPacket into GPU-ready vertices
pub fn livox_to_vertices(pkt: &LivoxPacket) -> Vec<LidarPointVertex> {
    pkt.points.iter().map(|p| LidarPointVertex {
        position: [p.x, p.y, p.z],
        intensity: (p.reflectivity as f32) / 255.0,
    }).collect()
}

/// ----------------- Decoder Registry & Pipeline -----------------

pub struct DecoderRegistry {
    topic_map: HashMap<String, Schema>,
}

impl DecoderRegistry {
    pub fn new() -> Self {
        Self { topic_map: HashMap::new() }
    }

    pub fn register(&mut self, topic: &str, schema: Schema) {
        self.topic_map.insert(topic.to_string(), schema);
    }

    /// The top-level decode pipeline:
    /// Given topic and payload bytes (MCAP payload), attempt to decode into DecodedMessage.
    pub fn decode(&self, topic: &str, payload: &[u8]) -> Result<DecodedMessage> {
        let schema = self.topic_map.get(topic)
            .ok_or_else(|| anyhow!("Unknown topic: {}", topic))?;
        match schema {
            Schema::Livox => {
                let livox = decode_livox_from_cdr(payload)?;
                Ok(DecodedMessage::Livox(livox))
            }
        }
    }
}

/// ----------------- Example usage -----------------
fn main() -> Result<()> {
    // Example: create registry, register topics you expect
    let mut reg = DecoderRegistry::new();
    reg.register("/livox/lidar", Schema::Livox);

    // In real use: payload & topic come from MCAP records.
    // Here we assume `sample_bytes` contains a raw payload from MCAP for a Livox message.
    let sample_bytes: Vec<u8> = load_sample_payload(); // implement this to read from file for testing

    // decode
    let decoded = reg.decode("/livox/lidar", &sample_bytes)?;
    match decoded {
        DecodedMessage::Livox(pkt) => {
            println!("Decoded Livox: points={}", pkt.points.len());
            let verts = livox_to_vertices(&pkt);
            // upload verts to GPU buffer via bytemuck::cast_slice(&verts)
            println!("Converted to {} GPU vertices", verts.len());
        }
    }

    Ok(())
}

// stub loader for example
fn load_sample_payload() -> Vec<u8> {
    // For real tests, load from disk (MCAP payload) or unit test fixture
    Vec::new()
}
