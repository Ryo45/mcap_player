//! Numeric field discovery and compiled selected-scalar CDR extraction.
use super::wire::{DecodeError, Reader};
use crate::StreamDescriptor;
use crate::message::schema::{RosMessageSchema, WireType, scalar_size, unsupported};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericField {
    pub path: String,
    pub ros_type: String,
}

impl RosMessageSchema {
    pub fn numeric_fields(&self) -> Result<Vec<NumericField>, DecodeError> {
        let mut result = vec![];
        self.enumerate(&self.root, "", &mut vec![], &mut result, &mut 65_536)?;
        Ok(result)
    }
    fn enumerate(
        &self,
        name: &str,
        prefix: &str,
        stack: &mut Vec<String>,
        out: &mut Vec<NumericField>,
        budget: &mut usize,
    ) -> Result<(), DecodeError> {
        if stack.len() >= 32 || stack.iter().any(|s| s == name) {
            return Err(unsupported("recursive schema"));
        }
        stack.push(name.into());
        let fields = self
            .definitions
            .get(name)
            .ok_or_else(|| unsupported(format!("missing definition {name}")))?;
        for field in fields {
            *budget = budget.checked_sub(1).ok_or(DecodeError::InvalidLength)?;
            let path = if prefix.is_empty() {
                field.name.clone()
            } else {
                format!("{prefix}.{}", field.name)
            };
            match &field.kind {
                WireType::Scalar(ty) if ty != "bool" => out.push(NumericField {
                    path,
                    ros_type: ty.clone(),
                }),
                WireType::Message(name) => self.enumerate(name, &path, stack, out, budget)?,
                _ => {}
            }
            if out.len() > 8192 {
                return Err(DecodeError::InvalidLength);
            }
        }
        stack.pop();
        Ok(())
    }
    pub fn compile_numeric(&self, path: &str) -> Result<NumericFieldExtractor, DecodeError> {
        let segments: Vec<_> = path.split('.').collect();
        let mut skips = vec![];
        let scalar = self.compile(&self.root, &segments, &mut skips, 0, &mut 65_536, false)?;
        Ok(NumericFieldExtractor { skips, scalar })
    }
    pub fn compile_scalar(&self, path: &str) -> Result<ScalarFieldExtractor, DecodeError> {
        let segments: Vec<_> = path.split('.').collect();
        let mut skips = vec![];
        let scalar = self.compile(&self.root, &segments, &mut skips, 0, &mut 65_536, true)?;
        Ok(ScalarFieldExtractor(NumericFieldExtractor {
            skips,
            scalar,
        }))
    }

    fn compile(
        &self,
        name: &str,
        path: &[&str],
        skips: &mut Vec<Skip>,
        depth: usize,
        budget: &mut usize,
        allow_text_and_bool: bool,
    ) -> Result<String, DecodeError> {
        if depth >= 32 || path.is_empty() {
            return Err(unsupported("invalid field path"));
        }
        for field in self
            .definitions
            .get(name)
            .ok_or_else(|| unsupported(format!("missing definition {name}")))?
        {
            if field.name == path[0] {
                return match &field.kind {
                    WireType::Scalar(ty)
                        if path.len() == 1 && (allow_text_and_bool || ty != "bool") =>
                    {
                        Ok(ty.clone())
                    }
                    WireType::Message(name) => self.compile(
                        name,
                        &path[1..],
                        skips,
                        depth + 1,
                        budget,
                        allow_text_and_bool,
                    ),
                    WireType::String if allow_text_and_bool && path.len() == 1 => {
                        Ok("string".into())
                    }
                    _ => Err(unsupported("selected field must be a supported scalar")),
                };
            }
            skips.push(self.skip(&field.kind, depth + 1, budget)?);
        }
        Err(unsupported(format!("unknown field {}", path[0])))
    }
    fn skip(&self, kind: &WireType, depth: usize, budget: &mut usize) -> Result<Skip, DecodeError> {
        if depth >= 32 {
            return Err(unsupported("recursive schema"));
        }
        *budget = budget.checked_sub(1).ok_or(DecodeError::InvalidLength)?;
        Ok(match kind {
            WireType::Scalar(ty) => Skip::Scalar(scalar_size(ty).unwrap()),
            WireType::String => Skip::String,
            WireType::Array(inner, count) => {
                Skip::Array(Box::new(self.skip(inner, depth + 1, budget)?), *count)
            }
            WireType::Message(name) => Skip::Message(
                self.definitions
                    .get(name)
                    .ok_or_else(|| unsupported(format!("missing definition {name}")))?
                    .iter()
                    .map(|f| self.skip(&f.kind, depth + 1, budget))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}
#[derive(Clone, Debug)]
enum Skip {
    Scalar(usize),
    String,
    Array(Box<Skip>, Option<usize>),
    Message(Vec<Skip>),
}
impl Skip {
    fn apply(&self, reader: &mut Reader<'_>, budget: &mut usize) -> Result<(), DecodeError> {
        *budget = budget.checked_sub(1).ok_or(DecodeError::InvalidLength)?;
        match self {
            Self::Scalar(size) => {
                reader.align(*size)?;
                reader.take(*size)?;
            }
            Self::String => {
                let length = reader.u32()? as usize;
                if length == 0 {
                    return Err(DecodeError::InvalidLength);
                }
                reader.take(length)?;
            }
            Self::Message(fields) => {
                // rosidl inserts a uint8 wire member into empty (including constant-only) messages.
                if fields.is_empty() {
                    reader.take(1)?;
                }
                for field in fields {
                    field.apply(reader, budget)?;
                }
            }
            Self::Array(inner, count) => {
                let length = match count {
                    Some(n) => *n,
                    None => reader.u32()? as usize,
                };
                if let Self::Scalar(size) = **inner {
                    if length > 0 {
                        reader.align(size)?;
                        reader.take(length.checked_mul(size).ok_or(DecodeError::InvalidLength)?)?;
                    }
                } else {
                    if length > *budget {
                        return Err(DecodeError::InvalidLength);
                    }
                    for _ in 0..length {
                        inner.apply(reader, budget)?;
                    }
                }
            }
        }
        Ok(())
    }
}
/// Compiled once per selected topic/field. Skips preceding bytes and stops at the scalar.
#[derive(Clone, Debug)]
pub struct NumericFieldExtractor {
    skips: Vec<Skip>,
    scalar: String,
}
impl NumericFieldExtractor {
    pub fn for_stream(stream: &StreamDescriptor, path: &str) -> Result<Self, DecodeError> {
        RosMessageSchema::for_stream(stream)?.compile_numeric(path)
    }
    fn reader<'a>(&self, payload: &'a [u8]) -> Result<Reader<'a>, DecodeError> {
        // Plain XCDR1 only. Parameter-list and XCDR2 require different layouts.
        if payload.len() < 4 {
            return Err(DecodeError::Truncated);
        }
        if payload[0] != 0 || payload[1] > 1 {
            return Err(DecodeError::InvalidEncapsulation);
        }
        let mut reader = Reader::new(payload)?;
        let mut budget = 1_000_000;
        for skip in &self.skips {
            skip.apply(&mut reader, &mut budget)?;
        }
        Ok(reader)
    }
    pub fn extract(&self, payload: &[u8]) -> Result<f64, DecodeError> {
        let mut reader = self.reader(payload)?;
        let size = scalar_size(&self.scalar).unwrap();
        let value = reader.scalar_bits(size)?;
        Ok(match self.scalar.as_str() {
            "int8" => value as i8 as f64,
            "int16" => value as i16 as f64,
            "int32" => value as i32 as f64,
            "int64" => value as i64 as f64,
            "float32" => f32::from_bits(value as u32) as f64,
            "float64" => f64::from_bits(value),
            _ => value as f64,
        })
    }
}

/// Selected scalar source facts; unlike plotting, signed/unsigned values remain exact integers.
#[derive(Clone, Debug, PartialEq)]
pub enum MessageScalar {
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Text(String),
}

#[derive(Clone, Debug)]
pub struct ScalarFieldExtractor(NumericFieldExtractor);
impl ScalarFieldExtractor {
    /// Reuses the numeric extractor's compiled skip plan; never decodes unrelated fields.
    pub fn extract(
        &self,
        payload: &[u8],
        max_text_bytes: usize,
    ) -> Result<MessageScalar, DecodeError> {
        let mut reader = self.0.reader(payload)?;
        if self.0.scalar == "string" {
            return reader
                .string_bounded(max_text_bytes)
                .map(MessageScalar::Text);
        }
        let value =
            reader.scalar_bits(scalar_size(&self.0.scalar).expect("compiled scalar type"))?;
        Ok(match self.0.scalar.as_str() {
            "bool" => {
                if value > 1 {
                    return Err(DecodeError::InvalidLength);
                }
                MessageScalar::Bool(value == 1)
            }
            "int8" => MessageScalar::Signed(value as i8 as i64),
            "int16" => MessageScalar::Signed(value as i16 as i64),
            "int32" => MessageScalar::Signed(value as i32 as i64),
            "int64" => MessageScalar::Signed(value as i64),
            "float32" | "float64" => {
                let number = if self.0.scalar == "float32" {
                    f32::from_bits(value as u32) as f64
                } else {
                    f64::from_bits(value)
                };
                if !number.is_finite() {
                    return Err(DecodeError::InvalidLength);
                }
                MessageScalar::Float(number)
            }
            _ => MessageScalar::Unsigned(value),
        })
    }
}

#[cfg(test)]
mod scalar_tests {
    use super::*;
    use crate::SchemaDefinition;
    fn schema(text: &str) -> RosMessageSchema {
        RosMessageSchema::parse(
            "custom/msg/Status",
            &SchemaDefinition {
                encoding: "ros2msg".into(),
                data: bytes::Bytes::copy_from_slice(text.as_bytes()),
            },
        )
        .unwrap()
    }
    #[test]
    fn selected_scalars_preserve_types_and_skip_large_arrays() {
        let schema = schema("uint8[] unused\nuint64 sequence\nstring state\nbool enabled\n");
        let mut data = vec![0, 1, 0, 0];
        data.extend_from_slice(&4096u32.to_le_bytes());
        data.resize(4104, 7);
        while !(data.len() - 4).is_multiple_of(8) {
            data.push(0);
        }
        data.extend_from_slice(&9_007_199_254_740_993u64.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(b"ok\0");
        data.push(1);
        assert_eq!(
            schema
                .compile_scalar("sequence")
                .unwrap()
                .extract(&data, 8)
                .unwrap(),
            MessageScalar::Unsigned(9_007_199_254_740_993)
        );
        let text = schema.compile_scalar("state").unwrap();
        assert_eq!(
            text.extract(&data, 8).unwrap(),
            MessageScalar::Text("ok".into())
        );
        assert!(text.extract(&data, 1).is_err());
        let flag = schema.compile_scalar("enabled").unwrap();
        assert_eq!(flag.extract(&data, 8).unwrap(), MessageScalar::Bool(true));
        *data.last_mut().unwrap() = 2;
        assert!(flag.extract(&data, 8).is_err());
        data.truncate(8);
        assert!(text.extract(&data, 8).is_err());
        assert!(schema.compile_numeric("state").is_err());
        assert!(schema.compile_numeric("enabled").is_err());
        assert!(schema.compile_scalar("unused").is_err());
    }
}
