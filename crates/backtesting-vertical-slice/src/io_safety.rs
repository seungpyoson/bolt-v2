use anyhow::{Context, Result, ensure};
use std::{fmt::Display, fs, io::Read, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteLimit {
    max_bytes: u64,
}

impl ByteLimit {
    pub fn new(max_bytes: u64) -> Result<Self> {
        ensure!(max_bytes > 0, "byte limit must be positive");
        Ok(Self { max_bytes })
    }

    pub const fn trusted_nonzero(max_bytes: u64) -> Self {
        Self { max_bytes }
    }

    pub const fn max_bytes(self) -> u64 {
        self.max_bytes
    }
}

pub const STAGED_OBJECT_BYTES: ByteLimit = ByteLimit::trusted_nonzero(1024 * 1024 * 1024);
pub const STAGED_DECODED_BYTES: ByteLimit = ByteLimit::trusted_nonzero(4 * 1024 * 1024 * 1024);

pub fn ensure_within_limit(label: impl Display, size: u64, limit: ByteLimit) -> Result<()> {
    ensure!(
        size <= limit.max_bytes(),
        "{label} is {size} bytes, exceeds configured byte limit {}",
        limit.max_bytes()
    );
    Ok(())
}

pub fn read_file_with_limit(path: &Path, limit: ByteLimit) -> Result<Vec<u8>> {
    let metadata =
        fs::metadata(path).with_context(|| format!("get metadata for {}", path.display()))?;
    ensure_within_limit(path.display(), metadata.len(), limit)?;
    fs::read(path).with_context(|| format!("read {}", path.display()))
}

pub fn read_to_vec_with_limit<R: Read>(
    reader: R,
    limit: ByteLimit,
    context: impl Display,
) -> Result<Vec<u8>> {
    let mut limited = reader.take(limit.max_bytes().saturating_add(1));
    let mut bytes = Vec::new();
    let read = limited
        .read_to_end(&mut bytes)
        .with_context(|| context.to_string())?;
    let read = u64::try_from(read).context("read byte count exceeds u64")?;
    ensure_within_limit(context, read, limit)?;
    Ok(bytes)
}

pub fn read_to_string_with_limit<R: Read>(
    reader: R,
    limit: ByteLimit,
    context: impl Display,
) -> Result<String> {
    let context = context.to_string();
    let bytes = read_to_vec_with_limit(reader, limit, context.as_str())?;
    String::from_utf8(bytes).with_context(|| format!("{context} is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{ByteLimit, read_file_with_limit, read_to_string_with_limit};
    use std::io::Cursor;

    #[test]
    fn read_file_with_limit_rejects_metadata_larger_than_limit() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("object.bin");
        std::fs::write(&path, b"abcdef").expect("write object");

        let err = read_file_with_limit(&path, ByteLimit::new(5).expect("limit"))
            .expect_err("oversize file must be rejected before reading");

        assert!(
            format!("{err:#}").contains("exceeds configured byte limit"),
            "{err:#}"
        );
    }

    #[test]
    fn read_to_string_with_limit_rejects_stream_larger_than_limit() {
        let err = read_to_string_with_limit(
            Cursor::new(b"abcdef"),
            ByteLimit::new(5).expect("limit"),
            "decode fixture",
        )
        .expect_err("oversize decoded stream must be rejected");

        assert!(
            format!("{err:#}").contains("exceeds configured byte limit"),
            "{err:#}"
        );
    }
}
