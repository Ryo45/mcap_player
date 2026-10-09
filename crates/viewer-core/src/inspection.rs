//! Inspector-only schema-driven view models. Never continuous/canonical decoded message state.
use crate::message::cdr::{
    common::Reader,
    numeric::{WireType, scalar_size},
};
use crate::{DecodeError, MeasurementTime, RawMessage, RosMessageSchema, StreamDescriptor};
use bytes::Bytes;
use std::sync::Arc;

pub const INSPECTOR_MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
pub const INSPECTOR_PAGE_FIELDS: usize = 64;
const MAX_TRAVERSAL: usize = 2_000_000;

#[derive(Debug)]
pub enum InspectorDecodeError {
    Decode(DecodeError),
    PayloadTooLarge { size: usize, limit: usize },
    InvalidPage,
    TraversalLimit,
}
impl std::fmt::Display for InspectorDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => error.fmt(f),
            Self::PayloadTooLarge { size, limit } => write!(
                f,
                "Inspector payload {size} bytes exceeds {limit} byte budget"
            ),
            Self::InvalidPage => write!(f, "invalid Inspector page cursor"),
            Self::TraversalLimit => write!(f, "Inspector traversal budget exceeded"),
        }
    }
}
impl std::error::Error for InspectorDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}
impl From<DecodeError> for InspectorDecodeError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}
#[derive(Debug)]
struct Layout {
    name: String,
    wire: LayoutWire,
}
#[derive(Debug)]
enum LayoutWire {
    Scalar(String),
    String,
    Record(Vec<(String, Arc<Layout>)>),
    Array {
        element: Arc<Layout>,
        count: Option<usize>,
    },
}
#[derive(Debug)]
pub struct InspectorSchema {
    layout: Arc<Layout>,
    stamp: Option<(crate::NumericFieldExtractor, crate::NumericFieldExtractor)>,
}
impl InspectorSchema {
    pub fn for_stream(stream: &StreamDescriptor) -> Result<Self, InspectorDecodeError> {
        let schema = RosMessageSchema::for_stream(stream)?;
        let root = WireType::Message(schema.root.clone());
        let layout = compile(&schema, &root, 0, &mut 65_536)?;
        // Header stamp is a payload fact, not the query/playback timeline. Only a typed ROS Header.
        let has_header=schema.definitions[&schema.root].iter().any(|field|field.name=="header" && matches!(&field.kind,WireType::Message(name) if name=="std_msgs/msg/Header"))
            && schema.definitions.get("std_msgs/msg/Header").is_some_and(|fields|fields.iter().any(|field|field.name=="stamp" && matches!(&field.kind,WireType::Message(name) if name=="builtin_interfaces/msg/Time")))
            && schema.definitions.get("builtin_interfaces/msg/Time").is_some_and(|fields|fields.iter().any(|field|field.name=="sec" && matches!(&field.kind,WireType::Scalar(name) if name=="int32")) && fields.iter().any(|field|field.name=="nanosec" && matches!(&field.kind,WireType::Scalar(name) if name=="uint32")));
        let stamp = if has_header {
            Some((
                schema.compile_numeric("header.stamp.sec")?,
                schema.compile_numeric("header.stamp.nanosec")?,
            ))
        } else {
            None
        };
        Ok(Self { layout, stamp })
    }
}
fn compile(
    schema: &RosMessageSchema,
    kind: &WireType,
    depth: usize,
    budget: &mut usize,
) -> Result<Arc<Layout>, InspectorDecodeError> {
    *budget = budget
        .checked_sub(1)
        .ok_or(InspectorDecodeError::TraversalLimit)?;
    if depth >= 32 {
        return Err(DecodeError::UnsupportedFormat(
            "recursive or too-deep Inspector schema".into(),
        )
        .into());
    }
    let (name, wire) = match kind {
        WireType::Scalar(name) => (name.clone(), LayoutWire::Scalar(name.clone())),
        WireType::String => ("string".into(), LayoutWire::String),
        WireType::Array(element, count) => {
            let element = compile(schema, element, depth + 1, budget)?;
            (
                format!(
                    "{}[{}]",
                    element.name,
                    count.map(|n| n.to_string()).unwrap_or_default()
                ),
                LayoutWire::Array {
                    element,
                    count: *count,
                },
            )
        }
        WireType::Message(name) => {
            let fields = schema.definitions.get(name).ok_or_else(|| {
                DecodeError::UnsupportedFormat(format!("missing definition {name}"))
            })?;
            (
                name.clone(),
                LayoutWire::Record(
                    fields
                        .iter()
                        .map(|field| {
                            Ok((
                                field.name.clone(),
                                compile(schema, &field.kind, depth + 1, budget)?,
                            ))
                        })
                        .collect::<Result<_, InspectorDecodeError>>()?,
                ),
            )
        }
    };
    Ok(Arc::new(Layout { name, wire }))
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectorHeaderStamp {
    pub seconds: i32,
    pub nanoseconds: u32,
}
impl InspectorHeaderStamp {
    pub fn measurement_time(self) -> Option<MeasurementTime> {
        (self.nanoseconds < 1_000_000_000).then_some(MeasurementTime(
            i64::from(self.seconds) * 1_000_000_000 + i64::from(self.nanoseconds),
        ))
    }
}
#[derive(Clone, Debug)]
pub enum InspectorScalar {
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Text {
        preview: String,
        bytes: usize,
        truncated: bool,
    },
}
#[derive(Clone, Debug)]
pub struct InspectorField {
    pub name: String,
    pub ros_type: String,
    pub value: InspectorValue,
}
#[derive(Clone, Debug)]
pub enum InspectorValue {
    Scalar(InspectorScalar),
    Container(InspectorContainer),
}
#[derive(Clone, Debug)]
pub struct InspectorContainer {
    layout: Arc<Layout>,
    offset: usize,
    length: usize,
}
impl InspectorContainer {
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn is_array(&self) -> bool {
        matches!(self.layout.wire, LayoutWire::Array { .. })
    }
    pub fn id(&self) -> (usize, usize) {
        (self.offset, Arc::as_ptr(&self.layout) as usize)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectorPageCursor {
    index: usize,
    offset: usize,
}
#[derive(Clone, Debug)]
pub struct InspectorPage {
    pub start_index: usize,
    pub fields: Vec<InspectorField>,
    pub next: Option<InspectorPageCursor>,
}
#[derive(Debug)]
pub struct InspectorMessage {
    pub stream_id: crate::StreamId,
    pub log_time: crate::ArrivalTime,
    pub publish_time_ns: Option<u64>,
    pub sequence: Option<u32>,
    pub header_stamp: Option<InspectorHeaderStamp>,
    pub root: InspectorContainer,
    payload: Bytes,
}
impl InspectorMessage {
    pub fn new(schema: &InspectorSchema, raw: &RawMessage) -> Result<Self, InspectorDecodeError> {
        if raw.payload.len() > INSPECTOR_MAX_PAYLOAD_BYTES {
            return Err(InspectorDecodeError::PayloadTooLarge {
                size: raw.payload.len(),
                limit: INSPECTOR_MAX_PAYLOAD_BYTES,
            });
        }
        if raw.payload.len() < 4 {
            return Err(DecodeError::Truncated.into());
        }
        if raw.payload[0] != 0 || raw.payload[1] > 1 {
            return Err(DecodeError::InvalidEncapsulation.into());
        }
        let header_stamp = schema
            .stamp
            .as_ref()
            .map(|(sec, nsec)| {
                Ok::<_, DecodeError>(InspectorHeaderStamp {
                    seconds: sec.extract(&raw.payload)? as i32,
                    nanoseconds: nsec.extract(&raw.payload)? as u32,
                })
            })
            .transpose()?;
        let LayoutWire::Record(fields) = &schema.layout.wire else {
            unreachable!("root schema is a record")
        };
        if fields.is_empty() {
            Reader::new(&raw.payload)?.take(1)?;
        }
        // A compact Inspector-owned copy prevents one tiny view from pinning a 256 MiB chunk.
        // Continuous delivery keeps sharing original Bytes; this copy is only for bounded inspection.
        Ok(Self {
            stream_id: raw.stream_id,
            log_time: raw.arrival_time,
            publish_time_ns: raw.publish_time_ns,
            sequence: raw.sequence,
            header_stamp,
            root: InspectorContainer {
                layout: schema.layout.clone(),
                offset: 4,
                length: fields.len(),
            },
            payload: Bytes::copy_from_slice(&raw.payload),
        })
    }
    pub fn retained_payload_bytes(&self) -> usize {
        self.payload.len()
    }
    pub fn page(
        &self,
        container: &InspectorContainer,
        cursor: Option<InspectorPageCursor>,
    ) -> Result<InspectorPage, InspectorDecodeError> {
        let cursor = cursor.unwrap_or(InspectorPageCursor {
            index: 0,
            offset: container.offset,
        });
        if cursor.index > container.length
            || cursor.offset < container.offset
            || cursor.offset > self.payload.len()
        {
            return Err(InspectorDecodeError::InvalidPage);
        }
        let mut reader = Reader::new(&self.payload)?;
        reader.take(cursor.offset - 4)?;
        let mut budget = MAX_TRAVERSAL;
        let end = container.length.min(cursor.index + INSPECTOR_PAGE_FIELDS);
        let mut result = Vec::with_capacity(end - cursor.index);
        for index in cursor.index..end {
            let (name, layout) = match &container.layout.wire {
                LayoutWire::Record(fields) => (&fields[index].0, fields[index].1.clone()),
                LayoutWire::Array { element, .. } => {
                    result.push(read_field(
                        format!("[{index}]"),
                        element,
                        &mut reader,
                        &mut budget,
                        index + 1 < container.length,
                    )?);
                    continue;
                }
                _ => return Err(InspectorDecodeError::InvalidPage),
            };
            result.push(read_field(
                name.clone(),
                &layout,
                &mut reader,
                &mut budget,
                index + 1 < container.length,
            )?);
        }
        Ok(InspectorPage {
            start_index: cursor.index,
            fields: result,
            next: (end < container.length).then_some(InspectorPageCursor {
                index: end,
                offset: reader.position(),
            }),
        })
    }
}
fn read_field(
    name: String,
    layout: &Arc<Layout>,
    reader: &mut Reader<'_>,
    budget: &mut usize,
    need_next_offset: bool,
) -> Result<InspectorField, InspectorDecodeError> {
    let value = match &layout.wire {
        LayoutWire::Scalar(ty) => {
            let bits = reader.scalar_bits(scalar_size(ty).expect("known primitive"))?;
            let scalar = match ty.as_str() {
                "bool" => InspectorScalar::Bool(bits != 0),
                "int8" => InspectorScalar::Signed(bits as i8 as i64),
                "int16" => InspectorScalar::Signed(bits as i16 as i64),
                "int32" => InspectorScalar::Signed(bits as i32 as i64),
                "int64" => InspectorScalar::Signed(bits as i64),
                "float32" => InspectorScalar::Float(f32::from_bits(bits as u32) as f64),
                "float64" => InspectorScalar::Float(f64::from_bits(bits)),
                _ => InspectorScalar::Unsigned(bits),
            };
            InspectorValue::Scalar(scalar)
        }
        LayoutWire::String => {
            let length = reader.u32()? as usize;
            if length == 0 {
                return Err(DecodeError::InvalidLength.into());
            }
            let raw = reader.take(length)?;
            if raw[length - 1] != 0 {
                return Err(DecodeError::InvalidLength.into());
            }
            let text =
                std::str::from_utf8(&raw[..length - 1]).map_err(|_| DecodeError::InvalidUtf8)?;
            let preview: String = text.chars().take(512).collect();
            let truncated = preview.len() < text.len();
            InspectorValue::Scalar(InspectorScalar::Text {
                preview,
                bytes: length - 1,
                truncated,
            })
        }
        LayoutWire::Record(fields) => {
            let offset = reader.position();
            if need_next_offset || fields.is_empty() {
                skip(layout, reader, budget)?;
            }
            InspectorValue::Container(InspectorContainer {
                layout: layout.clone(),
                offset,
                length: fields.len(),
            })
        }
        LayoutWire::Array { element, count } => {
            let length = match count {
                Some(length) => *length,
                None => reader.u32()? as usize,
            };
            let offset = reader.position();
            if need_next_offset || matches!(element.wire, LayoutWire::Scalar(_)) {
                skip_elements(element, length, reader, budget)?;
            }
            InspectorValue::Container(InspectorContainer {
                layout: layout.clone(),
                offset,
                length,
            })
        }
    };
    Ok(InspectorField {
        name,
        ros_type: layout.name.clone(),
        value,
    })
}
fn skip(
    layout: &Layout,
    reader: &mut Reader<'_>,
    budget: &mut usize,
) -> Result<(), InspectorDecodeError> {
    *budget = budget
        .checked_sub(1)
        .ok_or(InspectorDecodeError::TraversalLimit)?;
    match &layout.wire {
        LayoutWire::Scalar(ty) => {
            let size = scalar_size(ty).expect("known primitive");
            reader.align(size)?;
            reader.take(size)?;
        }
        LayoutWire::String => {
            let length = reader.u32()? as usize;
            if length == 0 {
                return Err(DecodeError::InvalidLength.into());
            }
            reader.take(length)?;
        }
        LayoutWire::Record(fields) => {
            if fields.is_empty() {
                reader.take(1)?;
            }
            for (_, field) in fields {
                skip(field, reader, budget)?;
            }
        }
        LayoutWire::Array { element, count } => {
            let length = match count {
                Some(length) => *length,
                None => reader.u32()? as usize,
            };
            skip_elements(element, length, reader, budget)?;
        }
    }
    Ok(())
}
fn skip_elements(
    element: &Layout,
    length: usize,
    reader: &mut Reader<'_>,
    budget: &mut usize,
) -> Result<(), InspectorDecodeError> {
    if let LayoutWire::Scalar(ty) = &element.wire {
        if length > 0 {
            let size = scalar_size(ty).unwrap();
            reader.align(size)?;
            reader.take(length.checked_mul(size).ok_or(DecodeError::InvalidLength)?)?;
        }
    } else {
        if length > *budget {
            return Err(InspectorDecodeError::TraversalLimit);
        }
        for _ in 0..length {
            skip(element, reader, budget)?;
        }
    }
    Ok(())
}
