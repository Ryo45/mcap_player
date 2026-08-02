use std::{fs::File, io, path::Path};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReadMetrics {
    pub(crate) calls: u64,
    pub(crate) bytes: u64,
}

#[derive(Debug)]
pub(crate) struct FileRangeReader {
    file: File,
    file_size: u64,
}

impl FileRangeReader {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "recording path is not a regular file",
            ));
        }
        Ok(Self {
            file,
            file_size: metadata.len(),
        })
    }

    pub(crate) fn file_size(&self) -> u64 {
        self.file_size
    }

    pub(crate) fn read_exact_at(
        &self,
        offset: u64,
        length: usize,
        maximum: usize,
        metrics: &mut ReadMetrics,
    ) -> io::Result<Vec<u8>> {
        if length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zero-length ranges are not supported",
            ));
        }
        if length > maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "range exceeds configured maximum",
            ));
        }
        let length_u64 = u64::try_from(length)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "range length exceeds u64"))?;
        let end = offset
            .checked_add(length_u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "range end overflow"))?;
        if end > self.file_size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "range exceeds file size",
            ));
        }
        let mut buffer = vec![0; length];
        read_all_at(&self.file, offset, &mut buffer)?;
        metrics.calls = metrics.calls.saturating_add(1);
        metrics.bytes = metrics.bytes.saturating_add(length_u64);
        Ok(buffer)
    }
}

#[cfg(unix)]
fn read_all_at(file: &File, mut offset: u64, mut output: &mut [u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !output.is_empty() {
        let count = file.read_at(output, offset)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short positional read",
            ));
        }
        offset = offset
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "offset overflow"))?;
        output = &mut output[count..];
    }
    Ok(())
}

#[cfg(not(unix))]
fn read_all_at(file: &File, offset: u64, output: &mut [u8]) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut isolated = file.try_clone()?;
    isolated.seek(SeekFrom::Start(offset))?;
    isolated.read_exact(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::Arc, thread};

    struct FixtureFile(std::path::PathBuf);

    impl Drop for FixtureFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn fixture() -> (FixtureFile, Arc<FileRangeReader>) {
        let path = std::env::temp_dir().join(format!(
            "recording-server-range-{}-{:?}.bin",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut file = File::create(&path).unwrap();
        file.write_all(b"0123456789").unwrap();
        let reader = Arc::new(FileRangeReader::open(&path).unwrap());
        (FixtureFile(path), reader)
    }

    #[test]
    fn exact_range_and_metrics() {
        let (_directory, reader) = fixture();
        let mut metrics = ReadMetrics::default();
        assert_eq!(
            reader.read_exact_at(2, 4, 10, &mut metrics).unwrap(),
            b"2345"
        );
        assert_eq!(metrics, ReadMetrics { calls: 1, bytes: 4 });
        assert!(reader.read_exact_at(0, 0, 10, &mut metrics).is_err());
        assert!(reader.read_exact_at(9, 2, 10, &mut metrics).is_err());
        assert!(reader.read_exact_at(u64::MAX, 2, 10, &mut metrics).is_err());
    }

    #[test]
    fn positional_reads_do_not_share_a_cursor() {
        let (_directory, reader) = fixture();
        let first = Arc::clone(&reader);
        let second = Arc::clone(&reader);
        let a = thread::spawn(move || {
            first
                .read_exact_at(0, 5, 10, &mut ReadMetrics::default())
                .unwrap()
        });
        let b = thread::spawn(move || {
            second
                .read_exact_at(5, 5, 10, &mut ReadMetrics::default())
                .unwrap()
        });
        assert_eq!(a.join().unwrap(), b"01234");
        assert_eq!(b.join().unwrap(), b"56789");
    }

    #[test]
    fn detects_a_short_read_if_an_immutable_file_is_truncated() {
        let (fixture, reader) = fixture();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.0)
            .unwrap()
            .set_len(2)
            .unwrap();
        let error = reader
            .read_exact_at(0, 5, 10, &mut ReadMetrics::default())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
