//! Shared guest SDK codec, NOT a CDR decoder. Explicit little-endian typed input ABI 3.
use crate::{ExtensionInput, MAX_INPUT_BYTES, MAX_INPUT_POINTS};
use anyhow::{Result, ensure};
use viewer_core::{
    ArrivalTime, BoundingBox3D, Detection3D, Detection3DArray, Detection3DFrame,
    MAX_DETECTION_RESULTS, MAX_DETECTIONS, MeasurementTime, MessageHeader, ObjectHypothesis,
    ObjectHypothesisWithPose, PathFrame, Pose3D, PoseWithCovariance,
};

pub enum DecodedInput {
    Path(PathFrame),
    Detections(Detection3DFrame),
}
impl DecodedInput {
    pub fn as_input(&self) -> ExtensionInput<'_> {
        match self {
            Self::Path(p) => ExtensionInput::Path(p),
            Self::Detections(d) => ExtensionInput::Detections(d),
        }
    }
}

struct Writer(Vec<u8>);
impl Writer {
    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() <= MAX_INPUT_BYTES.saturating_sub(self.0.len()),
            "extension input exceeds byte limit"
        );
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn u32(&mut self, value: u32) -> Result<()> {
        self.put(&value.to_le_bytes())
    }
    fn i64(&mut self, value: i64) -> Result<()> {
        self.put(&value.to_le_bytes())
    }
    fn floats(&mut self, values: &[f64]) -> Result<()> {
        for v in values {
            ensure!(v.is_finite(), "non-finite input");
            self.put(&v.to_le_bytes())?;
        }
        Ok(())
    }
    fn string(&mut self, value: &str, max: usize) -> Result<()> {
        ensure!(value.len() <= max, "input string too long");
        self.u32(value.len() as u32)?;
        self.put(value.as_bytes())
    }
    fn header(&mut self, h: &MessageHeader) -> Result<()> {
        self.i64(h.stamp.0)?;
        self.string(&h.frame_id, 256)
    }
    fn pose(&mut self, p: &Pose3D) -> Result<()> {
        self.floats(&p.position)?;
        self.floats(&p.orientation)
    }
}
pub fn encode_input(input: ExtensionInput<'_>) -> Result<Vec<u8>> {
    let mut w = Writer(Vec::new());
    match input {
        ExtensionInput::Path(p) => {
            ensure!(
                p.points.len() <= MAX_INPUT_POINTS,
                "too many Path positions"
            );
            w.u32(1)?;
            w.i64(p.arrival_time.0)?;
            w.i64(p.measurement_time.0)?;
            w.string(&p.frame_id, 256)?;
            w.u32(p.points.len() as u32)?;
            for p in &p.points {
                w.floats(p)?;
            }
        }
        ExtensionInput::Detections(f) => {
            ensure!(
                f.message.detections.len() <= MAX_DETECTIONS,
                "too many detections"
            );
            w.u32(2)?;
            w.i64(f.arrival_time.0)?;
            w.header(&f.message.header)?;
            w.u32(f.message.detections.len() as u32)?;
            for d in &f.message.detections {
                ensure!(
                    d.results.len() <= MAX_DETECTION_RESULTS,
                    "too many hypotheses"
                );
                w.header(&d.header)?;
                w.string(&d.id, 256)?;
                w.pose(&d.bbox.center)?;
                w.floats(&d.bbox.size)?;
                w.u32(d.results.len() as u32)?;
                for result in &d.results {
                    w.string(&result.hypothesis.class_id, 128)?;
                    w.floats(&[result.hypothesis.score])?;
                    w.pose(&result.pose.pose)?;
                    w.floats(&result.pose.covariance)?;
                }
            }
        }
    }
    Ok(w.0)
}
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(n)
            .ok_or_else(|| anyhow::anyhow!("input offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| anyhow::anyhow!("truncated extension input"))?;
        self.position = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn floats<const N: usize>(&mut self) -> Result<[f64; N]> {
        let mut result = [0.; N];
        for v in &mut result {
            *v = f64::from_le_bytes(self.take(8)?.try_into()?);
            ensure!(v.is_finite(), "non-finite input");
        }
        Ok(result)
    }
    fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        ensure!(n <= max, "input count limit");
        Ok(n)
    }
    fn string(&mut self, max: usize) -> Result<String> {
        let n = self.count(max)?;
        Ok(std::str::from_utf8(self.take(n)?)?.to_owned())
    }
    fn header(&mut self) -> Result<MessageHeader> {
        Ok(MessageHeader {
            stamp: MeasurementTime(self.i64()?),
            frame_id: self.string(256)?,
        })
    }
    fn pose(&mut self) -> Result<Pose3D> {
        Ok(Pose3D {
            position: self.floats()?,
            orientation: self.floats()?,
        })
    }
}
pub fn decode_input(bytes: &[u8]) -> Result<DecodedInput> {
    ensure!(bytes.len() <= MAX_INPUT_BYTES, "input exceeds byte limit");
    let mut r = Reader { bytes, position: 0 };
    let kind = r.u32()?;
    let arrival_time = ArrivalTime(r.i64()?);
    let result = match kind {
        1 => {
            let measurement_time = MeasurementTime(r.i64()?);
            let frame_id = r.string(256)?;
            let n = r.count(MAX_INPUT_POINTS)?;
            let points = (0..n).map(|_| r.floats()).collect::<Result<Vec<_>>>()?;
            DecodedInput::Path(PathFrame {
                arrival_time,
                measurement_time,
                frame_id,
                points,
            })
        }
        2 => {
            let header = r.header()?;
            let n = r.count(MAX_DETECTIONS)?;
            let mut detections = Vec::with_capacity(n);
            for _ in 0..n {
                let header = r.header()?;
                let id = r.string(256)?;
                let bbox = BoundingBox3D {
                    center: r.pose()?,
                    size: r.floats()?,
                };
                let n = r.count(MAX_DETECTION_RESULTS)?;
                let mut results = Vec::with_capacity(n);
                for _ in 0..n {
                    results.push(ObjectHypothesisWithPose {
                        hypothesis: ObjectHypothesis {
                            class_id: r.string(128)?,
                            score: r.floats::<1>()?[0],
                        },
                        pose: PoseWithCovariance {
                            pose: r.pose()?,
                            covariance: r.floats()?,
                        },
                    });
                }
                detections.push(Detection3D {
                    header,
                    id,
                    bbox,
                    results,
                });
            }
            DecodedInput::Detections(Detection3DFrame {
                arrival_time,
                message: Detection3DArray { header, detections },
            })
        }
        _ => anyhow::bail!("unknown input kind"),
    };
    ensure!(r.position == bytes.len(), "trailing extension input");
    Ok(result)
}

pub const BOX_BYTES: usize = 60;
/// Encode already validated guest objects. GPU resources never cross this interface.
pub fn encode_boxes(overlay: &viewer_core::SceneOverlay) -> Result<Vec<u8>> {
    ensure!(
        overlay.lines().is_empty(),
        "Wasm output currently supports Boxes only"
    );
    let mut bytes = Vec::with_capacity(overlay.boxes().len() * BOX_BYTES);
    for b in overlay.boxes() {
        bytes.extend_from_slice(&b.id.0.to_le_bytes());
        for v in b
            .center
            .into_iter()
            .chain(b.orientation)
            .chain(b.size)
            .chain(b.color)
        {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    Ok(bytes)
}
