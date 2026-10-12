//! Parsed ROS message definitions and field types, independent of payload traversal.
use crate::{DecodeError, SchemaDefinition, StreamDescriptor};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) enum WireType {
    Scalar(String),
    String,
    Message(String),
    Array(Box<WireType>, Option<usize>),
}
#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) kind: WireType,
}

/// Parsed source metadata. Only ros2msg is supported by this vertical slice.
#[derive(Clone, Debug)]
pub struct RosMessageSchema {
    pub(crate) root: String,
    pub(crate) definitions: BTreeMap<String, Vec<Field>>,
}

pub(crate) fn unsupported(reason: impl Into<String>) -> DecodeError {
    DecodeError::UnsupportedFormat(reason.into())
}
fn canonical(name: &str) -> String {
    if name.matches('/').count() == 1 {
        name.replacen('/', "/msg/", 1)
    } else {
        name.into()
    }
}
pub(crate) fn scalar_size(name: &str) -> Option<usize> {
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
}
