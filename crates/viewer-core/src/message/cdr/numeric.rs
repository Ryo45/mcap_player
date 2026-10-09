//! ros2msg metadata and compiled selected-scalar CDR extraction. No decoded value tree.
use super::common::{DecodeError, Reader};
use crate::{SchemaDefinition, StreamDescriptor};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericField {
    pub path: String,
    pub ros_type: String,
}

#[derive(Clone, Debug)]
enum WireType {
    Scalar(String),
    String,
    Message(String),
    Array(Box<WireType>, Option<usize>),
}
#[derive(Clone, Debug)]
struct Field {
    name: String,
    kind: WireType,
}

/// Parsed source metadata. Only ros2msg is supported by this vertical slice.
#[derive(Clone, Debug)]
pub struct RosMessageSchema {
    root: String,
    definitions: BTreeMap<String, Vec<Field>>,
}

fn unsupported(reason: impl Into<String>) -> DecodeError {
    DecodeError::UnsupportedFormat(reason.into())
}
fn canonical(name: &str) -> String {
    if name.matches('/').count() == 1 {
        name.replacen('/', "/msg/", 1)
    } else {
        name.into()
    }
}
fn scalar_size(name: &str) -> Option<usize> {
    match name {
        "bool" | "byte" | "char" | "int8" | "uint8" => Some(1),
        "int16" | "uint16" => Some(2),
        "int32" | "uint32" | "float32" => Some(4),
        "int64" | "uint64" | "float64" => Some(8),
        _ => None,
    }
}
impl RosMessageSchema {
    pub fn for_stream(stream: &StreamDescriptor) -> Result<Self, DecodeError> {
        if stream.message_encoding != "cdr" {
            return Err(unsupported("requires CDR messages"));
        }
        Self::parse(
            &stream.schema,
            stream
                .schema_definition
                .as_deref()
                .ok_or_else(|| unsupported("source has no schema definition"))?,
        )
    }
    pub fn parse(name: &str, definition: &SchemaDefinition) -> Result<Self, DecodeError> {
        if definition.encoding != "ros2msg" || definition.data.len() > 1024 * 1024 {
            return Err(unsupported("requires ros2msg schema up to 1 MiB"));
        }
        let text = std::str::from_utf8(&definition.data).map_err(|_| DecodeError::InvalidUtf8)?;
        let root = canonical(name);
        let mut definitions = BTreeMap::<String, Vec<Field>>::new();
        let mut current = root.clone();
        definitions.insert(current.clone(), vec![]);
        let mut field_count = 0;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with("===") {
                continue;
            }
            if let Some(name) = line.strip_prefix("MSG:") {
                current = canonical(name.trim());
                if definitions.insert(current.clone(), vec![]).is_some() {
                    return Err(unsupported("duplicate ros2msg definition"));
                }
                continue;
            }
            let mut parts = line.split_whitespace();
            let ty = parts
                .next()
                .ok_or_else(|| unsupported("missing field type"))?;
            let field = parts
                .next()
                .ok_or_else(|| unsupported("missing field name"))?;
            // Constants occupy no bytes. Field defaults do not alter the wire layout.
            if field.contains('=') || parts.clone().next() == Some("=") {
                continue;
            }
            if !field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(unsupported("invalid ros2msg field name"));
            }
            let (base, array) = if let Some(index) = ty.find('[') {
                let length = ty[index + 1..]
                    .strip_suffix(']')
                    .ok_or_else(|| unsupported("invalid array"))?;
                let count = if length.is_empty() || length.starts_with("<=") {
                    None
                } else {
                    Some(
                        length
                            .parse::<usize>()
                            .map_err(|_| DecodeError::InvalidLength)?,
                    )
                };
                (&ty[..index], Some(count))
            } else {
                (ty, None)
            };
            let mut kind = if scalar_size(base).is_some() {
                WireType::Scalar(base.into())
            } else if base == "string" || base.starts_with("string<=") {
                WireType::String
            } else if base == "wstring" || base.starts_with("wstring<=") {
                return Err(unsupported("wstring"));
            } else {
                let qualified = if base.contains('/') {
                    canonical(base)
                } else {
                    format!("{}/msg/{base}", current.split('/').next().unwrap_or(""))
                };
                WireType::Message(qualified)
            };
            if let Some(count) = array {
                kind = WireType::Array(Box::new(kind), count);
            }
            let fields = definitions.get_mut(&current).unwrap();
            if fields.iter().any(|f| f.name == field) {
                return Err(unsupported("duplicate field"));
            }
            fields.push(Field {
                name: field.into(),
                kind,
            });
            field_count += 1;
            if field_count > 8192 {
                return Err(DecodeError::InvalidLength);
            }
        }
        Ok(Self { root, definitions })
    }
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
        let scalar = self.compile(&self.root, &segments, &mut skips, 0, &mut 65_536)?;
        Ok(NumericFieldExtractor { skips, scalar })
    }
    fn compile(
        &self,
        name: &str,
        path: &[&str],
        skips: &mut Vec<Skip>,
        depth: usize,
        budget: &mut usize,
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
                    WireType::Scalar(ty) if path.len() == 1 && ty != "bool" => Ok(ty.clone()),
                    WireType::Message(name) => {
                        self.compile(name, &path[1..], skips, depth + 1, budget)
                    }
                    _ => Err(unsupported("selected field must be numeric scalar")),
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
    pub fn extract(&self, payload: &[u8]) -> Result<f64, DecodeError> {
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
