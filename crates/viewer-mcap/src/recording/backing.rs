//! Owned immutable bytes or bounded positional reads. File mutation cannot invalidate Rust slices.
use crate::McapSourceError;
use bytes::Bytes;
use mcap::Summary;
#[cfg(not(target_arch = "wasm32"))]
use mcap::sans_io::{SummaryReadEvent, SummaryReader, SummaryReaderOptions};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, SeekFrom};

// Metadata and individual records retain the smaller bound. Chunk payloads have
// a separate ceiling so large camera chunks need not fit the metadata budget.
pub const MAX_SOURCE_RECORD_BYTES: usize = 256 * 1024 * 1024;
pub(super) const MAX_CHUNK_BYTES: usize = if isize::BITS == 32 {
    // Vec allocations cannot exceed isize::MAX even when usize holds 2 GiB.
    isize::MAX as usize
} else {
    2 * 1024 * 1024 * 1024
};

#[derive(Clone)]
pub(super) enum Backing {
    Bytes(Bytes),
    #[cfg(not(target_arch = "wasm32"))]
    File(std::sync::Arc<FileRanges>),
    #[cfg(not(target_arch = "wasm32"))]
    Segments(std::sync::Arc<super::segments::SegmentedBacking>),
}
impl Backing {
    pub fn channel_id(&self, chunk: usize, local: u16) -> Result<u16, McapSourceError> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Self::Segments(parts) = self {
            return parts
                .chunk_channels
                .get(chunk)
                .and_then(|map| map.get(&local))
                .copied()
                .ok_or(mcap::McapError::BadIndex.into());
        }
        let _ = chunk;
        Ok(local)
    }
    pub fn file_read_totals(&self) -> (u64, u64) {
        match self {
            Self::Bytes(_) => (0, 0),
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => (
                file.read_calls.load(std::sync::atomic::Ordering::Relaxed),
                file.read_bytes.load(std::sync::atomic::Ordering::Relaxed),
            ),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Segments(parts) => parts.read_totals(),
        }
    }
    pub fn validate(&self) -> Result<(), McapSourceError> {
        match self {
            Self::Bytes(_) => Ok(()),
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => file.check(),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Segments(parts) => parts.validate(),
        }
    }
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => file.metadata.len(),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Segments(parts) => parts.length,
        }
    }
    pub fn read(&self, offset: u64, length: usize) -> Result<Bytes, McapSourceError> {
        self.read_with_limit(offset, length, MAX_SOURCE_RECORD_BYTES)
    }
    pub fn read_chunk(&self, offset: u64, length: usize) -> Result<Bytes, McapSourceError> {
        self.read_with_limit(offset, length, MAX_CHUNK_BYTES)
    }
    pub(super) fn read_with_limit(
        &self,
        offset: u64,
        length: usize,
        maximum: usize,
    ) -> Result<Bytes, McapSourceError> {
        if length > maximum {
            return Err(McapSourceError::ReadLimit {
                requested: length,
                maximum,
            });
        }
        let end = offset
            .checked_add(length as u64)
            .ok_or(McapSourceError::ReadOverflow)?;
        if end > self.len() {
            return Err(McapSourceError::ReadOutOfBounds {
                offset,
                length,
                file_size: self.len(),
            });
        }
        match self {
            Self::Bytes(bytes) => Ok(bytes.slice(offset as usize..end as usize)),
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => file.read(offset, length),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Segments(parts) => parts.read(offset, length, maximum),
        }
    }
    pub fn summary(&self) -> Result<std::sync::Arc<Summary>, McapSourceError> {
        match self {
            Self::Bytes(bytes) => Summary::read(bytes)?
                .ok_or(McapSourceError::SummaryRequired)
                .map(std::sync::Arc::new),
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(_) => self.summary_from_ranges().map(std::sync::Arc::new),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Segments(parts) => Ok(parts.summary.clone()),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn summary_from_ranges(&self) -> Result<Summary, McapSourceError> {
        let mut reader = SummaryReader::new_with_options(
            SummaryReaderOptions::default()
                .with_file_size(self.len())
                .with_record_length_limit(MAX_SOURCE_RECORD_BYTES),
        );
        let mut position = 0u64;
        while let Some(event) = reader.next_event() {
            match event? {
                SummaryReadEvent::SeekRequest(seek) => {
                    position = match seek {
                        SeekFrom::Start(value) => Some(value),
                        SeekFrom::Current(delta) => position.checked_add_signed(delta),
                        SeekFrom::End(delta) => self.len().checked_add_signed(delta),
                    }
                    .ok_or(McapSourceError::ReadOverflow)?;
                    if position > self.len() {
                        return Err(McapSourceError::ReadOutOfBounds {
                            offset: position,
                            length: 0,
                            file_size: self.len(),
                        });
                    }
                    reader.notify_seeked(position);
                }
                SummaryReadEvent::ReadRequest(length) => {
                    let data = self.read(position, length)?;
                    reader.insert(length).copy_from_slice(&data);
                    reader.notify_read(length);
                    position = position
                        .checked_add(length as u64)
                        .ok_or(McapSourceError::ReadOverflow)?;
                }
            }
        }
        reader.finish().ok_or(McapSourceError::SummaryRequired)
    }
    pub fn summary_crc(&self) -> Result<u32, McapSourceError> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Self::Segments(parts) = self {
            parts.validate()?;
            return Ok(parts.summary_crc);
        }
        let offset = self
            .len()
            .checked_sub(37)
            .ok_or(McapSourceError::ReadOverflow)?;
        let tail = self.read(offset, 37)?;
        let mut bytes = mcap::MAGIC.to_vec();
        bytes.extend_from_slice(&tail);
        Ok(mcap::read::footer(&bytes)?.summary_crc)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct FileRanges {
    file: std::fs::File,
    metadata: std::fs::Metadata,
    read_calls: std::sync::atomic::AtomicU64,
    read_bytes: std::sync::atomic::AtomicU64,
    #[cfg(not(any(unix, windows)))]
    cursor: std::sync::Mutex<()>,
}
#[cfg(not(target_arch = "wasm32"))]
impl FileRanges {
    pub fn open(path: &std::path::Path) -> Result<Self, McapSourceError> {
        let file = std::fs::File::open(path).map_err(McapSourceError::Io)?;
        let metadata = file.metadata().map_err(McapSourceError::Io)?;
        if !metadata.is_file() {
            return Err(McapSourceError::NotRegularFile);
        }
        Ok(Self {
            file,
            metadata,
            read_calls: Default::default(),
            read_bytes: Default::default(),
            #[cfg(not(any(unix, windows)))]
            cursor: std::sync::Mutex::new(()),
        })
    }
    fn check(&self) -> Result<(), McapSourceError> {
        let actual = self.file.metadata().map_err(McapSourceError::Io)?;
        if actual.len() != self.metadata.len()
            || actual.modified().ok() != self.metadata.modified().ok()
        {
            return Err(McapSourceError::SourceChanged);
        }
        Ok(())
    }
    fn read(&self, offset: u64, length: usize) -> Result<Bytes, McapSourceError> {
        self.check()?;
        let mut bytes = vec![0; length];
        #[cfg(not(any(unix, windows)))]
        let _guard = self
            .cursor
            .lock()
            .map_err(|_| McapSourceError::ReadLockPoisoned)?;
        let mut done = 0;
        while done < length {
            let position = offset
                .checked_add(done as u64)
                .ok_or(McapSourceError::ReadOverflow)?;
            #[cfg(unix)]
            let result = {
                use std::os::unix::fs::FileExt;
                self.file.read_at(&mut bytes[done..], position)
            };
            #[cfg(windows)]
            let result = {
                use std::os::windows::fs::FileExt;
                self.file.seek_read(&mut bytes[done..], position)
            };
            #[cfg(not(any(unix, windows)))]
            let result = {
                use std::io::{Read, Seek};
                let mut file = &self.file;
                file.seek(SeekFrom::Start(position))
                    .and_then(|_| file.read(&mut bytes[done..]))
            };
            match result {
                Ok(0) => {
                    return Err(McapSourceError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "short positional read",
                    )));
                }
                Ok(count) => {
                    self.read_calls
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.read_bytes
                        .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
                    done += count;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(McapSourceError::Io(error)),
            }
        }
        self.check()?;
        Ok(Bytes::from(bytes))
    }
}
