//! Incremental Chunk decoding. The caller supplies bounded input, selects records,
//! stages results, and decides when to yield or cancel between steps.
use super::chunk::McapChunkError;

const OUTPUT_BYTES: usize = 128 * 1024;
const MAX_RECORD_BYTES: u64 = 256 * 1024 * 1024;

enum Compression {
    None,
    #[cfg(feature = "mcap-zstd")]
    Zstd(zstd::stream::raw::Decoder<'static>),
}

/// A step consumes input and produces at most 128 KiB of uncompressed bytes.
/// Chunk lengths stay u64, including on WASM32. Individual records are bounded.
/// Unlike an owned Chunk backing, borrowed records must be copied if retained.
pub struct McapChunkStream {
    compression: Compression,
    compressed_size: u64,
    uncompressed_size: u64,
    consumed: u64,
    produced: u64,
    frame_complete: bool,
    output: Vec<u8>,
    pending: Vec<u8>,
    record_offset: u64,
}

impl McapChunkStream {
    pub fn new(
        compression: &str,
        compressed_size: u64,
        uncompressed_size: u64,
    ) -> Result<Self, McapChunkError> {
        let compression = match compression {
            "" => Compression::None,
            #[cfg(feature = "mcap-zstd")]
            "zstd" => {
                let mut decoder = zstd::stream::raw::Decoder::new().map_err(zstd_error)?;
                // Bound a malicious frame's decoder window independently of Chunk size.
                decoder
                    .set_parameter(zstd::stream::raw::DParameter::WindowLogMax(27))
                    .map_err(zstd_error)?;
                Compression::Zstd(decoder)
            }
            other => return Err(McapChunkError::UnsupportedCompression(other.into())),
        };
        Ok(Self {
            compression,
            compressed_size,
            uncompressed_size,
            consumed: 0,
            produced: 0,
            frame_complete: false,
            output: vec![0; OUTPUT_BYTES],
            pending: Vec::new(),
            record_offset: 0,
        })
    }

    /// Returns consumed input bytes. An empty input can drain decoder output.
    /// The visitor sees physical record offsets, including non-message records.
    /// Return false to pause after a record; buffered data is preserved for the next step.
    pub fn step<E: From<McapChunkError>>(
        &mut self,
        input: &[u8],
        mut visit: impl FnMut(u64, u8, &[u8]) -> Result<bool, E>,
    ) -> Result<usize, E> {
        if input.len() as u64 > self.compressed_size.saturating_sub(self.consumed) {
            return Err(McapChunkError::InvalidRecord("excess compressed Chunk bytes").into());
        }
        let (read, written) = match &mut self.compression {
            Compression::None => {
                let count = input.len().min(self.output.len());
                self.output[..count].copy_from_slice(&input[..count]);
                self.frame_complete = true;
                (count, count)
            }
            #[cfg(feature = "mcap-zstd")]
            Compression::Zstd(decoder) => {
                use zstd::stream::raw::Operation;
                let status = decoder
                    .run_on_buffers(input, &mut self.output)
                    .map_err(zstd_error)?;
                // An empty drain after a complete frame must not erase completion.
                if status.bytes_read != 0 || status.bytes_written != 0 {
                    self.frame_complete = status.remaining == 0;
                }
                (status.bytes_read, status.bytes_written)
            }
        };
        self.consumed += read as u64;
        if written as u64 > self.uncompressed_size.saturating_sub(self.produced) {
            return Err(McapChunkError::InvalidRecord("excess expanded Chunk bytes").into());
        }
        self.produced += written as u64;
        self.pending.extend_from_slice(&self.output[..written]);
        let mut offset = 0;
        while self.pending.len() - offset >= 9 {
            let header = &self.pending[offset..offset + 9];
            let length = u64::from_le_bytes(header[1..9].try_into().expect("eight-byte length"));
            if length > MAX_RECORD_BYTES {
                return Err(
                    McapChunkError::InvalidRecord("streamed record exceeds 256 MiB").into(),
                );
            }
            let end = offset + 9 + length as usize;
            if end > self.pending.len() {
                break;
            }
            let keep_reading = visit(
                self.record_offset,
                header[0],
                &self.pending[offset + 9..end],
            )?;
            self.record_offset += 9 + length;
            offset = end;
            if !keep_reading {
                break;
            }
        }
        if offset != 0 {
            self.pending.drain(..offset);
        }
        if read == 0 && written == 0 && !input.is_empty() {
            return Err(McapChunkError::InvalidRecord("Chunk decoder made no progress").into());
        }
        Ok(read)
    }

    /// Call after draining with empty input. No staged messages should be committed
    /// before this succeeds. CRC policy remains the caller's responsibility.
    pub fn finish(self) -> Result<(), McapChunkError> {
        if self.consumed != self.compressed_size
            || self.produced != self.uncompressed_size
            || !self.pending.is_empty()
            || !self.frame_complete
        {
            return Err(McapChunkError::InvalidRecord("incomplete streamed Chunk"));
        }
        Ok(())
    }

    /// Physical offset of the next record, also when the visitor paused parsing.
    pub fn next_record_offset(&self) -> u64 {
        self.record_offset
    }

    pub fn expanded_bytes(&self) -> u64 {
        self.produced
    }
}

#[cfg(feature = "mcap-zstd")]
fn zstd_error(source: std::io::Error) -> McapChunkError {
    McapChunkError::Decompression {
        format: "zstd",
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<u8> {
        let mut bytes = Vec::new();
        for (opcode, size) in [(3, 0), (5, 300_000), (5, 1)] {
            bytes.push(opcode);
            bytes.extend_from_slice(&(size as u64).to_le_bytes());
            bytes.extend(std::iter::repeat_n(opcode, size));
        }
        bytes
    }

    fn decode(
        encoded: &[u8],
        codec: &str,
        size: u64,
    ) -> Result<Vec<(u64, u8, Vec<u8>)>, McapChunkError> {
        let mut decoder = McapChunkStream::new(codec, encoded.len() as u64, size)?;
        let mut records = Vec::new();
        for input in encoded.chunks(17) {
            let mut consumed = 0;
            loop {
                let before = decoder.expanded_bytes();
                consumed += decoder.step(&input[consumed..], |offset, opcode, body| {
                    records.push((offset, opcode, body.to_vec()));
                    Ok::<_, McapChunkError>(true)
                })?;
                if consumed == input.len() && decoder.expanded_bytes() == before {
                    break;
                }
            }
        }
        decoder.finish()?;
        Ok(records)
    }

    #[test]
    fn fragmented_headers_payloads_and_physical_offsets() {
        let raw = records();
        let actual = decode(&raw, "", raw.len() as u64).unwrap();
        assert_eq!(
            actual,
            vec![
                (0, 3, vec![]),
                (9, 5, vec![5; 300_000]),
                (300_018, 5, vec![5])
            ]
        );
    }

    #[test]
    #[cfg(feature = "mcap-zstd")]
    fn zstd_fragments_match_uncompressed_records_and_reject_truncation() {
        let raw = records();
        let encoded = zstd::bulk::compress(&raw, 3).unwrap();
        assert_eq!(
            decode(&encoded, "zstd", raw.len() as u64).unwrap(),
            decode(&raw, "", raw.len() as u64).unwrap()
        );
        for length in [0, 1, encoded.len() - 1] {
            assert!(decode(&encoded[..length], "zstd", raw.len() as u64).is_err());
        }
        assert!(decode(&encoded, "zstd", raw.len() as u64 - 1).is_err());
        assert!(decode(&encoded, "zstd", raw.len() as u64 + 1).is_err());
    }

    #[test]
    fn malformed_lengths_are_errors_without_reserving_the_declared_size() {
        let mut raw = vec![5];
        raw.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode(&raw, "", raw.len() as u64).is_err());
        assert!(decode(&[5], "", 1).is_err());
        assert!(decode(&records(), "", 1).is_err());
        // Creating a decoder for a 20 GiB Chunk does not allocate a Chunk-sized buffer.
        let decoder = McapChunkStream::new("", 20 << 30, 20 << 30).unwrap();
        assert_eq!(decoder.output.len(), OUTPUT_BYTES);
        assert!(decoder.pending.is_empty());
    }

    #[test]
    #[cfg(feature = "mcap-zstd")]
    fn paused_record_visit_resumes_without_losing_prefetched_bytes() {
        let raw = records();
        let encoded = zstd::bulk::compress(&raw, 3).unwrap();
        let mut decoder =
            McapChunkStream::new("zstd", encoded.len() as u64, raw.len() as u64).unwrap();
        let mut consumed = decoder
            .step(&encoded, |offset, opcode, body| {
                assert_eq!((offset, opcode, body.len()), (0, 3, 0));
                Ok::<_, McapChunkError>(false)
            })
            .unwrap();
        assert_eq!(decoder.next_record_offset(), 9);
        let mut seen = Vec::new();
        loop {
            let before = decoder.expanded_bytes();
            consumed += decoder
                .step(&encoded[consumed..], |offset, opcode, body| {
                    seen.push((offset, opcode, body.len()));
                    Ok::<_, McapChunkError>(true)
                })
                .unwrap();
            if consumed == encoded.len() && before == decoder.expanded_bytes() {
                break;
            }
        }
        decoder.finish().unwrap();
        assert_eq!(seen, vec![(9, 5, 300_000), (300_018, 5, 1)]);
    }
}
