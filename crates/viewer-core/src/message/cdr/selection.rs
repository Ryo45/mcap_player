//! Schema-compiled projections through arrays. Array elements consume their full wire record;
//! the root projection stops after its selected value. No unrelated value tree is materialized.
use super::{
    numeric::{Skip, cdr_reader, read_scalar},
    wire::{DecodeError, Reader},
};
use crate::message::schema::{WireType, unsupported};
use crate::{MessageScalar, RosMessageSchema};
#[derive(Clone, Debug, PartialEq)]
pub enum SelectedValue {
    Scalar(MessageScalar),
    Array(Vec<SelectedValue>),
}
#[derive(Clone, Copy, Debug)]
pub struct FieldDecodeLimits {
    pub max_array_len: usize,
    pub max_values: usize,
    pub max_text_bytes: usize,
    pub max_total_text_bytes: usize,
}
pub struct FieldDecodeBudget {
    limits: FieldDecodeLimits,
    values: usize,
    text: usize,
    skip_work: usize,
}
impl FieldDecodeBudget {
    pub fn new(limits: FieldDecodeLimits) -> Self {
        Self {
            values: limits.max_values,
            text: limits.max_total_text_bytes,
            skip_work: 1_000_000,
            limits,
        }
    }
    fn value(&mut self) -> Result<(), DecodeError> {
        self.values = self
            .values
            .checked_sub(1)
            .ok_or(DecodeError::InvalidLength)?;
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct SelectedFieldExtractor(Selection);
#[derive(Clone, Debug)]
enum Selection {
    Scalar(String),
    Record {
        before: Vec<Skip>,
        selected: Box<Selection>,
        after: Vec<Skip>,
    },
    Array {
        count: Option<usize>,
        element: Box<Selection>,
    },
}
impl RosMessageSchema {
    pub fn compile_field(&self, path: &str) -> Result<SelectedFieldExtractor, DecodeError> {
        if path.is_empty() || path.len() > 128 {
            return Err(unsupported("invalid selected path"));
        }
        Ok(SelectedFieldExtractor(self.select_record(
            &self.root,
            &path.split('.').collect::<Vec<_>>(),
            false,
            0,
            &mut 65_536,
        )?))
    }
    fn select_record(
        &self,
        name: &str,
        path: &[&str],
        complete: bool,
        depth: usize,
        budget: &mut usize,
    ) -> Result<Selection, DecodeError> {
        if depth >= 32 || path.is_empty() {
            return Err(unsupported("invalid selected path/depth"));
        }
        let fields = self
            .definitions
            .get(name)
            .ok_or_else(|| unsupported(format!("missing definition {name}")))?;
        let array = path[0].ends_with("[]");
        let name = path[0].strip_suffix("[]").unwrap_or(path[0]);
        let index = fields
            .iter()
            .position(|f| f.name == name)
            .ok_or_else(|| unsupported(format!("unknown field {name}")))?;
        let before = fields[..index]
            .iter()
            .map(|f| self.skip(&f.kind, depth + 1, budget))
            .collect::<Result<_, _>>()?;
        let selected = if array {
            let WireType::Array(inner, count) = &fields[index].kind else {
                return Err(unsupported("[] requires an array"));
            };
            Selection::Array {
                count: *count,
                element: Box::new(self.select_value(inner, &path[1..], true, depth + 1, budget)?),
            }
        } else {
            self.select_value(&fields[index].kind, &path[1..], complete, depth + 1, budget)?
        };
        let after = if complete {
            fields[index + 1..]
                .iter()
                .map(|f| self.skip(&f.kind, depth + 1, budget))
                .collect::<Result<_, _>>()?
        } else {
            vec![]
        };
        Ok(Selection::Record {
            before,
            selected: Box::new(selected),
            after,
        })
    }
    fn select_value(
        &self,
        kind: &WireType,
        path: &[&str],
        complete: bool,
        depth: usize,
        budget: &mut usize,
    ) -> Result<Selection, DecodeError> {
        if depth >= 32 {
            return Err(unsupported("selected projection too deep"));
        }
        Ok(match kind {
            WireType::Scalar(ty) if path.is_empty() => Selection::Scalar(ty.clone()),
            WireType::String if path.is_empty() => Selection::Scalar("string".into()),
            WireType::Message(name) => {
                self.select_record(name, path, complete, depth + 1, budget)?
            }
            _ => return Err(unsupported("select scalar fields; traverse arrays with []")),
        })
    }
}
impl SelectedFieldExtractor {
    pub fn extract(
        &self,
        payload: &[u8],
        budget: &mut FieldDecodeBudget,
    ) -> Result<SelectedValue, DecodeError> {
        self.0.read(&mut cdr_reader(payload)?, budget)
    }
}
impl Selection {
    fn read(
        &self,
        reader: &mut Reader<'_>,
        budget: &mut FieldDecodeBudget,
    ) -> Result<SelectedValue, DecodeError> {
        match self {
            Self::Record {
                before,
                selected,
                after,
            } => {
                for skip in before {
                    skip.apply(reader, &mut budget.skip_work)?;
                }
                let value = selected.read(reader, budget)?;
                for skip in after {
                    skip.apply(reader, &mut budget.skip_work)?;
                }
                Ok(value)
            }
            Self::Scalar(ty) => {
                budget.value()?;
                let value = read_scalar(ty, reader, budget.limits.max_text_bytes.min(budget.text))?;
                if let MessageScalar::Text(text) = &value {
                    budget.text = budget
                        .text
                        .checked_sub(text.len())
                        .ok_or(DecodeError::InvalidLength)?;
                }
                Ok(SelectedValue::Scalar(value))
            }
            Self::Array { count, element } => {
                budget.value()?;
                let count = match count {
                    Some(n) => *n,
                    None => reader.u32()? as usize,
                };
                if count > budget.limits.max_array_len || count > budget.values {
                    return Err(DecodeError::InvalidLength);
                }
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(element.read(reader, budget)?);
                }
                Ok(SelectedValue::Array(values))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SchemaDefinition;
    fn schema(text: &str) -> RosMessageSchema {
        RosMessageSchema::parse(
            "demo/msg/Frame",
            &SchemaDefinition {
                encoding: "ros2msg".into(),
                data: bytes::Bytes::copy_from_slice(text.as_bytes()),
            },
        )
        .unwrap()
    }
    fn budget() -> FieldDecodeBudget {
        FieldDecodeBudget::new(FieldDecodeLimits {
            max_array_len: 128,
            max_values: 512,
            max_text_bytes: 32,
            max_total_text_bytes: 64,
        })
    }
    fn align(data: &mut Vec<u8>, n: usize) {
        while !(data.len() - 4).is_multiple_of(n) {
            data.push(0);
        }
    }
    fn text(data: &mut Vec<u8>, s: &str) {
        align(data, 4);
        data.extend_from_slice(&(s.len() as u32 + 1).to_le_bytes());
        data.extend_from_slice(s.as_bytes());
        data.push(0);
    }
    #[test]
    fn array_records_preserve_indices_and_consume_trailing_fields() {
        let schema = schema(
            "Item[] items\nfloat32[] values\n===\nMSG: demo/Item\nuint64 id\nstring name\nfloat64 score\n",
        );
        let mut data = vec![0, 1, 0, 0];
        data.extend_from_slice(&2u32.to_le_bytes());
        for (i, name) in ["first", "second"].iter().enumerate() {
            align(&mut data, 8);
            data.extend_from_slice(&(9_007_199_254_740_993u64 + i as u64).to_le_bytes());
            text(&mut data, name);
            align(&mut data, 8);
            data.extend_from_slice(&(0.9 - i as f64 * 0.1).to_le_bytes());
        }
        align(&mut data, 4);
        data.extend_from_slice(&3u32.to_le_bytes());
        for v in [1f32, 2., 3.] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let names = schema.compile_field("items[].name").unwrap();
        assert_eq!(
            names.extract(&data, &mut budget()).unwrap(),
            SelectedValue::Array(vec![
                SelectedValue::Scalar(MessageScalar::Text("first".into())),
                SelectedValue::Scalar(MessageScalar::Text("second".into()))
            ])
        );
        assert_eq!(
            schema
                .compile_field("items[].id")
                .unwrap()
                .extract(&data, &mut budget())
                .unwrap(),
            SelectedValue::Array(vec![
                SelectedValue::Scalar(MessageScalar::Unsigned(9_007_199_254_740_993)),
                SelectedValue::Scalar(MessageScalar::Unsigned(9_007_199_254_740_994))
            ])
        );
        assert_eq!(
            schema
                .compile_field("values[]")
                .unwrap()
                .extract(&data, &mut budget())
                .unwrap(),
            SelectedValue::Array(
                [1., 2., 3.]
                    .map(|v| SelectedValue::Scalar(MessageScalar::Float(v)))
                    .to_vec()
            )
        );
        for length in 0..data.len() {
            assert!(
                schema
                    .compile_field("values[]")
                    .unwrap()
                    .extract(&data[..length], &mut budget())
                    .is_err()
            );
        }
        let mut empty = vec![0, 1, 0, 0];
        empty.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            names.extract(&empty, &mut budget()).unwrap(),
            SelectedValue::Array(vec![])
        );
        let mut too_many = vec![0, 1, 0, 0];
        too_many.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(names.extract(&too_many, &mut budget()).is_err());
        let mut limited = FieldDecodeBudget::new(FieldDecodeLimits {
            max_array_len: 128,
            max_values: 512,
            max_text_bytes: 32,
            max_total_text_bytes: 5,
        });
        assert!(names.extract(&data, &mut limited).is_err());
        assert!(schema.compile_field("items.name").is_err());
        assert!(schema.compile_field("items[]").is_err());
        assert!(schema.compile_field("values[].x").is_err());
    }
    #[test]
    fn nested_and_fixed_arrays_keep_shape_and_share_budget() {
        let schema =
            schema("Group[] groups\n===\nMSG: demo/Group\nfloat64[2] values\nstring unused\n");
        let mut data = vec![0, 1, 0, 0];
        data.extend_from_slice(&2u32.to_le_bytes());
        for index in 0..2 {
            align(&mut data, 8);
            data.extend_from_slice(&(index as f64).to_le_bytes());
            data.extend_from_slice(&(index as f64 + 1.).to_le_bytes());
            text(&mut data, "skip me");
        }
        let plan = schema.compile_field("groups[].values[]").unwrap();
        let values = plan.extract(&data, &mut budget()).unwrap();
        let SelectedValue::Array(groups) = values else {
            panic!()
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[1],
            SelectedValue::Array(vec![
                SelectedValue::Scalar(MessageScalar::Float(1.)),
                SelectedValue::Scalar(MessageScalar::Float(2.))
            ])
        );
        let mut limited = FieldDecodeBudget::new(FieldDecodeLimits {
            max_array_len: 128,
            max_values: 7,
            max_text_bytes: 32,
            max_total_text_bytes: 64,
        });
        assert!(plan.extract(&data, &mut limited).is_ok());
        assert!(plan.extract(&data, &mut limited).is_err());
    }
}
