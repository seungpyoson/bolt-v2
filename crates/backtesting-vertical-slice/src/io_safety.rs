//! Filesystem-type and byte-bounding primitives for trusted operator inputs.
//!
//! Invariant: every I/O path that reads from a staged object or inflates a
//! compressed member goes through one of these helpers, so an oversized input
//! cannot silently exhaust memory — it fails loud with a clear byte-limit error
//! before the excess bytes are ever loaded.

use anyhow::{Context, Result, ensure};
use std::{
    fmt::Display,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

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

/// Open a regular file without first blocking on a special-file open.
///
/// The path-level check rejects static FIFOs and other special files before
/// [`File::open`]. The descriptor-level check preserves the invariant after
/// opening for replacements whose open completes. A concurrent replacement
/// with a writer-less FIFO can still block in `open`; that adversarial race is
/// outside the trusted-operator threat model.
pub fn open_regular_file(path: &Path, label: impl Display) -> Result<File> {
    let label = label.to_string();
    let path_metadata =
        fs::metadata(path).with_context(|| format!("open {label} {}", path.display()))?;
    ensure!(
        path_metadata.is_file(),
        "{label} {} is not a regular file",
        path.display()
    );
    let file = File::open(path).with_context(|| format!("open {label} {}", path.display()))?;
    let opened_metadata = file
        .metadata()
        .with_context(|| format!("inspect opened {label} {}", path.display()))?;
    ensure!(
        opened_metadata.is_file(),
        "opened {label} {} is not a regular file",
        path.display()
    );
    Ok(file)
}

/// Recursively enumerate only regular files beneath `root` without following
/// symlinks or accepting other special filesystem entries.
pub fn collect_regular_files(root: &Path, label: impl Display) -> Result<Vec<PathBuf>> {
    let label = label.to_string();
    let root_metadata = fs::symlink_metadata(root)
        .with_context(|| format!("inspect {label} root {}", root.display()))?;
    ensure!(
        root_metadata.file_type().is_dir(),
        "{label} root {} is not a real directory",
        root.display()
    );
    let mut files = Vec::new();
    collect_regular_files_under(root, root, &label, &mut files)?;
    Ok(files)
}

fn collect_regular_files_under(
    root: &Path,
    dir: &Path,
    label: &str,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read directory {}", dir.display()))? {
        let entry =
            entry.with_context(|| format!("read directory entry under {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", path.display()))?;
        if file_type.is_dir() {
            collect_regular_files_under(root, &path, label, files)?;
        } else if file_type.is_file() {
            files.push(path);
        } else {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            anyhow::bail!("{label} contains non-regular file {}", relative.display());
        }
    }
    Ok(())
}

pub fn read_file_with_limit(path: &Path, limit: ByteLimit) -> Result<Vec<u8>> {
    let file = open_regular_file(path, "bounded input")?;
    let metadata = file
        .metadata()
        .with_context(|| format!("get metadata for {}", path.display()))?;
    ensure_within_limit(path.display(), metadata.len(), limit)?;
    read_to_vec_with_limit(file, limit, format!("read {}", path.display()))
}

pub fn read_to_vec_with_limit<R: Read>(
    mut reader: R,
    limit: ByteLimit,
    context: impl Display,
) -> Result<Vec<u8>> {
    let mut limited = reader.by_ref().take(limit.max_bytes());
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .with_context(|| context.to_string())?;
    let mut excess = [0_u8; 1];
    let excess_bytes = loop {
        match reader.read(&mut excess) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => break result.with_context(|| context.to_string())?,
        }
    };
    ensure!(
        excess_bytes == 0,
        "{context} exceeds configured byte limit {}",
        limit.max_bytes()
    );
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
    use super::{
        ByteLimit, open_regular_file, read_file_with_limit, read_to_string_with_limit,
        read_to_vec_with_limit,
    };
    use std::{
        cell::RefCell,
        io::{Cursor, Read},
        rc::Rc,
    };

    struct RecordingReader {
        bytes: Cursor<Vec<u8>>,
        requested_reads: Rc<RefCell<Vec<usize>>>,
    }

    impl Read for RecordingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.requested_reads.borrow_mut().push(buffer.len());
            self.bytes.read(buffer)
        }
    }

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

    #[cfg(unix)]
    #[test]
    fn open_regular_file_rejects_fifo_before_opening() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("control.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("create FIFO");
        assert!(status.success(), "mkfifo must create the test FIFO");

        let err = open_regular_file(&path, "pinned control")
            .expect_err("a FIFO must be rejected without opening it");

        assert!(
            format!("{err:#}").contains("is not a regular file"),
            "{err:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_regular_file_preserves_symlink_to_regular_file_contract() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let target = dir.path().join("control.toml");
        let link = dir.path().join("control-link.toml");
        std::fs::write(&target, b"regular bytes").expect("write regular target");
        symlink(&target, &link).expect("create symlink to regular target");

        let mut file = open_regular_file(&link, "pinned control")
            .expect("symlink to regular file remains accepted");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read opened target");

        assert_eq!(bytes, b"regular bytes");
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

    #[test]
    fn oversized_stream_is_probed_without_retaining_a_limit_plus_one_buffer() {
        let requested_reads = Rc::new(RefCell::new(Vec::new()));
        let reader = RecordingReader {
            bytes: Cursor::new(b"abcdef".to_vec()),
            requested_reads: Rc::clone(&requested_reads),
        };

        let err =
            read_to_vec_with_limit(reader, ByteLimit::new(5).expect("limit"), "bounded fixture")
                .expect_err("one byte over the limit must reject");

        assert!(
            format!("{err:#}").contains("exceeds configured byte limit"),
            "{err:#}"
        );
        assert!(
            requested_reads
                .borrow()
                .iter()
                .all(|requested| *requested <= 5),
            "the retained read must never request a limit-plus-one buffer: {:?}",
            requested_reads.borrow()
        );
    }
}
