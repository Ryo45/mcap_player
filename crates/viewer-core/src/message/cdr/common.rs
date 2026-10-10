use std::fmt;

const MAX_FIELD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    InvalidEncapsulation,
    InvalidLength,
    InvalidUtf8,
    InvalidTimestamp,
    UnsupportedFormat(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated CDR payload"),
            Self::InvalidEncapsulation => write!(f, "unsupported CDR encapsulation"),
            Self::InvalidLength => write!(f, "invalid or excessive CDR field length"),
            Self::InvalidUtf8 => write!(f, "CDR string is not UTF-8"),
            Self::InvalidTimestamp => write!(f, "invalid ROS timestamp"),
            Self::UnsupportedFormat(value) => write!(f, "unsupported format: {value}"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    base: usize,
    endian: Endian,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 4 {
            return Err(DecodeError::Truncated);
        }
        let endian = match bytes[0..2] {
            [0, 0] | [0, 2] => Endian::Big,
            [0, 1] | [0, 3] => Endian::Little,
            _ => return Err(DecodeError::InvalidEncapsulation),
        };
        Ok(Self {
            bytes,
            position: 4,
            base: 4,
            endian,
        })
    }

    pub(crate) fn align(&mut self, alignment: usize) -> Result<(), DecodeError> {
        let relative = self.position - self.base;
        let padding = (alignment - relative % alignment) % alignment;
        self.position = self
            .position
            .checked_add(padding)
            .ok_or(DecodeError::InvalidLength)?;
        if self.position > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        Ok(())
    }

    pub(crate) fn scalar_bits(&mut self, size: usize) -> Result<u64, DecodeError> {
        self.align(size)?;
        let raw = self.take(size)?;
        let mut bytes = [0; 8];
        Ok(match self.endian {
            Endian::Little => {
                bytes[..size].copy_from_slice(raw);
                u64::from_le_bytes(bytes)
            }
            Endian::Big => {
                bytes[8 - size..].copy_from_slice(raw);
                u64::from_be_bytes(bytes)
            }
        })
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(DecodeError::InvalidLength)?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or(DecodeError::Truncated)?;
        self.position = end;
        Ok(result)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        self.align(4)?;
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(raw),
            Endian::Big => u32::from_be_bytes(raw),
        })
    }

    pub(crate) fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        self.align(8)?;
        let raw: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes(raw),
            Endian::Big => u64::from_be_bytes(raw),
        })
    }

    pub(crate) fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub(crate) fn length(&mut self) -> Result<usize, DecodeError> {
        let length = usize::try_from(self.u32()?).map_err(|_| DecodeError::InvalidLength)?;
        if length > MAX_FIELD_BYTES {
            return Err(DecodeError::InvalidLength);
        }
        Ok(length)
    }

    pub(crate) fn sequence_length(
        &mut self,
        minimum_element_bytes: usize,
    ) -> Result<usize, DecodeError> {
        let length = self.length()?;
        let remaining = self.bytes.len().saturating_sub(self.position);
        if length > remaining / minimum_element_bytes {
            return Err(DecodeError::InvalidLength);
        }
        Ok(length)
    }

    pub(crate) fn string(&mut self) -> Result<String, DecodeError> {
        let length = self.length()?;
        if length == 0 {
            return Err(DecodeError::InvalidLength);
        }
        let bytes = self.take(length)?;
        if bytes.last() != Some(&0) {
            return Err(DecodeError::InvalidLength);
        }
        std::str::from_utf8(&bytes[..length - 1])
            .map(str::to_owned)
            .map_err(|_| DecodeError::InvalidUtf8)
    }
}

pub(crate) fn align_output(output: &mut Vec<u8>, alignment: usize) {
    let relative = output.len() - 4;
    output.resize(
        output.len() + (alignment - relative % alignment) % alignment,
        0,
    );
}

pub(crate) fn push_u32(output: &mut Vec<u8>, value: u32) {
    align_output(output, 4);
    output.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_string(output: &mut Vec<u8>, value: &str) -> Result<(), DecodeError> {
    let length = value
        .len()
        .checked_add(1)
        .ok_or(DecodeError::InvalidLength)?;
    push_u32(
        output,
        u32::try_from(length).map_err(|_| DecodeError::InvalidLength)?,
    );
    output.extend_from_slice(value.as_bytes());
    output.push(0);
    Ok(())
}

pub(crate) fn push_f64(output: &mut Vec<u8>, value: f64) {
    align_output(output, 8);
    output.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn push_f32(output: &mut Vec<u8>, value: f32) {
    push_u32(output, value.to_bits());
}
