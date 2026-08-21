//! Streaming reader over a gzip-compressed POSIX `ustar` tar of JSONL members.
//!
//! L2 archive objects in the tar-bundled family are a gzip of a POSIX tar that
//! carries hundreds of regular-file members, each one JSONL text. Two defects in
//! the superseded converter lane motivated this primitive and it is built to be
//! their exact opposite:
//!
//! 1. The superseded path gunzipped the *whole* tar into a single `Vec<u8>`
//!    bounded at a fixed archive cap. Real archives are multiple gibibytes
//!    uncompressed, so that approach either exhausts memory or rejects valid
//!    data. Here decompression is streaming: [`flate2::read::MultiGzDecoder`]
//!    wraps the source reader and tar entries are read sequentially out of the
//!    decompressed stream — the whole archive is never resident.
//! 2. The superseded path returned only the *first* matching member and dropped
//!    the rest. Here *every* member whose name ends with `member_suffix` is
//!    visited, in archive order, and non-matching members are skipped by
//!    consuming (never retaining) their bytes.
//!
//! Decompression uses [`flate2::read::MultiGzDecoder`], not the single-stream
//! `GzDecoder`. A gzip file is legitimately a concatenation of independent gzip
//! members (the format is defined to be stream-concatenable, and producers such
//! as parallel compressors and `cat a.gz b.gz` emit multi-member streams).
//! `GzDecoder` stops at the end of the *first* gzip member and silently drops
//! every subsequent member's bytes, which would truncate the tar mid-archive and
//! lose every tar member carried after the first gzip stream. `MultiGzDecoder`
//! transparently concatenates all gzip members into one decompressed byte
//! stream, so the tar walk sees the whole archive regardless of how many gzip
//! streams it was written as.
//!
//! Each visited member's content read is bounded by `max_member_bytes` and fails
//! loud naming the offending member when exceeded, so one pathological member
//! cannot blow the bound for the rest of the archive. A truncated archive (a
//! header or data block that runs past the end of the stream) also fails loud.
//!
//! This module owns only the container concern (decompress + walk tar members);
//! it does not parse JSONL or know anything about the order-book-delta wire
//! shape. The normalize path in
//! [`super::jsonl_record_stream`] consumes the visitor.

use std::io::{self, Read};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::MultiGzDecoder;

/// POSIX tar block size: headers and data are laid out in 512-byte blocks.
const TAR_BLOCK: usize = 512;

/// Byte offset of the `name` field within a tar header block.
const NAME_OFFSET: usize = 0;

/// Length of the `name` field within a tar header block.
const NAME_LEN: usize = 100;

/// Byte offset and length of the optional POSIX `prefix` field.
const PREFIX_OFFSET: usize = 345;
const PREFIX_LEN: usize = 155;

/// Byte offset of the octal `size` field within a tar header block.
const SIZE_OFFSET: usize = 124;

/// Length of the octal `size` field within a tar header block.
const SIZE_LEN: usize = 12;

/// Byte offset of the octal header-checksum field.
const CHECKSUM_OFFSET: usize = 148;

/// Length of the header-checksum field.
const CHECKSUM_LEN: usize = 8;

/// Byte offset of the `typeflag` field within a tar header block.
const TYPEFLAG_OFFSET: usize = 156;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GzipTarVisitStats {
    pub decoded_bytes: u64,
    pub members: u64,
}

/// Visit matching regular tar members through a size-limited reader without
/// retaining member bodies. Every archive member consumes `max_members` and
/// `max_member_bytes`; every decompressed tar byte, including skipped members
/// and end padding, consumes `max_decoded_bytes`.
pub fn visit_gzip_tar_members<R: Read>(
    reader: R,
    member_suffix: &str,
    max_decoded_bytes: u64,
    max_members: u64,
    max_member_bytes: u64,
    mut visit: impl FnMut(&str, u64, &mut dyn Read) -> Result<()>,
) -> Result<GzipTarVisitStats> {
    ensure!(
        !member_suffix.is_empty(),
        "tar member suffix must not be empty"
    );
    ensure!(max_decoded_bytes > 0, "max_decoded_bytes must be positive");
    ensure!(max_members > 0, "max_members must be positive");
    ensure!(max_member_bytes > 0, "max_member_bytes must be positive");

    let decoder = MultiGzDecoder::new(reader);
    let mut decoder = DecodedLimitReader::new(decoder, max_decoded_bytes);
    let mut members = 0u64;
    loop {
        let mut header = [0u8; TAR_BLOCK];
        match read_full_block(&mut decoder, &mut header)? {
            BlockRead::Eof => bail!("tar archive ended before its end-of-archive marker"),
            BlockRead::Truncated(read) => {
                bail!("tar header truncated: read {read} of {TAR_BLOCK} bytes")
            }
            BlockRead::Full => {}
        }
        if header.iter().all(|&byte| byte == 0) {
            let mut second_end_block = [0u8; TAR_BLOCK];
            match read_full_block(&mut decoder, &mut second_end_block)? {
                BlockRead::Full => ensure!(
                    second_end_block.iter().all(|&byte| byte == 0),
                    "tar archive second end-of-archive block is not zero"
                ),
                BlockRead::Eof => {
                    bail!("tar archive ended after only one end-of-archive zero block")
                }
                BlockRead::Truncated(read) => bail!(
                    "tar second end-of-archive block truncated: read {read} of {TAR_BLOCK} bytes"
                ),
            }
            drain_zero_tar_tail(&mut decoder)?;
            break;
        }

        validate_header_checksum(&header)?;

        members = members
            .checked_add(1)
            .context("tar member count overflow")?;
        ensure!(
            members <= max_members,
            "tar member count {members} exceeds max_members {max_members}"
        );
        let typeflag = header[TYPEFLAG_OFFSET];
        if let Some(extension) = match typeflag {
            b'L' => Some("GNU longname"),
            b'K' => Some("GNU longlink"),
            b'x' => Some("PAX extended header"),
            b'g' => Some("PAX global extended header"),
            _ => None,
        } {
            bail!("tar member {members} uses unsupported {extension} record");
        }
        let name = parse_name(&header).with_context(|| format!("tar member {members} name"))?;
        let size = parse_octal_size(&header)?;
        ensure!(
            size <= max_member_bytes,
            "tar member {name:?} declares {size} bytes, exceeding max_member_bytes {max_member_bytes}"
        );
        let is_regular_file = typeflag == b'0' || typeflag == 0;
        let matches = is_regular_file && name.ends_with(member_suffix);

        if matches {
            {
                let mut member = (&mut decoder).take(size);
                visit(&name, size, &mut member)?;
                let remaining = member.limit();
                let consumed = io::copy(&mut member, &mut io::sink())
                    .with_context(|| format!("drain tar member {name:?}"))?;
                ensure!(
                    consumed == remaining,
                    "tar member {name:?} truncated: drained {consumed} of {remaining} remaining bytes"
                );
            }
            consume_padding(&mut decoder, size, &name)?;
        } else {
            skip_padded_member(&mut decoder, size, &name)?;
        }
    }

    Ok(GzipTarVisitStats {
        decoded_bytes: decoder.bytes_read(),
        members,
    })
}

struct DecodedLimitReader<R> {
    inner: R,
    max_bytes: u64,
    bytes_read: u64,
}

impl<R> DecodedLimitReader<R> {
    fn new(inner: R, max_bytes: u64) -> Self {
        Self {
            inner,
            max_bytes,
            bytes_read: 0,
        }
    }

    fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
}

impl<R: Read> Read for DecodedLimitReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let remaining = self.max_bytes.saturating_sub(self.bytes_read);
        if remaining == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!(
                    "decoded tar bytes exceed max_decoded_bytes {}",
                    self.max_bytes
                ))),
            };
        }
        let allowed = output
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut output[..allowed])?;
        self.bytes_read += read as u64;
        Ok(read)
    }
}

fn skip_padded_member(reader: &mut impl Read, size: u64, name: &str) -> Result<()> {
    let total = padded_len(size, name)?;
    let consumed = io::copy(&mut reader.take(total), &mut io::sink())
        .map_err(|error| anyhow::anyhow!("skip tar member {name:?}: {error}"))?;
    ensure!(
        consumed == total,
        "tar member {name:?} truncated: skipped {consumed} of {total} bytes"
    );
    Ok(())
}

fn drain_zero_tar_tail(reader: &mut impl Read) -> Result<()> {
    let mut buffer = [0u8; TAR_BLOCK];
    loop {
        let read = reader.read(&mut buffer).context("drain tar end padding")?;
        if read == 0 {
            return Ok(());
        }
        ensure!(
            buffer[..read].iter().all(|byte| *byte == 0),
            "tar archive contains non-zero bytes after its end marker"
        );
    }
}

/// Outcome of attempting to read one full 512-byte tar block.
enum BlockRead {
    /// A full block was read.
    Full,
    /// Clean end-of-stream before any byte of this block was read.
    Eof,
    /// The stream ended partway through this block (truncated archive).
    Truncated(usize),
}

/// Read exactly one 512-byte block from a streaming reader.
///
/// Distinguishes a clean end-of-archive (zero bytes available) from a truncated
/// archive (some-but-not-all bytes available), so the caller can fail loud on
/// the latter.
fn read_full_block<R: Read>(reader: &mut R, block: &mut [u8; TAR_BLOCK]) -> Result<BlockRead> {
    let mut filled = 0usize;
    while filled < TAR_BLOCK {
        let read = reader
            .read(&mut block[filled..])
            .context("read tar header block")?;
        if read == 0 {
            if filled == 0 {
                return Ok(BlockRead::Eof);
            }
            return Ok(BlockRead::Truncated(filled));
        }
        filled += read;
    }
    Ok(BlockRead::Full)
}

/// Consume the block padding that follows a member's data, failing loud if the
/// stream ends before the padding is fully read.
fn consume_padding<R: Read>(reader: &mut R, size: u64, name: &str) -> Result<()> {
    let padding = padded_len(size, name)?
        .checked_sub(size)
        .context("tar member padding underflow")?;
    if padding == 0 {
        return Ok(());
    }
    let consumed = std::io::copy(&mut reader.take(padding), &mut std::io::sink())
        .with_context(|| format!("consume tar member {name:?} padding"))?;
    ensure!(
        consumed == padding,
        "tar member {name:?} truncated padding: consumed {consumed} of {padding} bytes"
    );
    Ok(())
}

/// Round a member's data size up to the next 512-byte block boundary.
fn padded_len(size: u64, name: &str) -> Result<u64> {
    let blocks = size
        .checked_add(TAR_BLOCK as u64 - 1)
        .with_context(|| format!("tar member {name:?} size overflow"))?
        / TAR_BLOCK as u64;
    blocks
        .checked_mul(TAR_BLOCK as u64)
        .with_context(|| format!("tar member {name:?} padded size overflow"))
}

/// Parse the strict UTF-8 POSIX `prefix/name` fields of a tar header.
fn parse_name(header: &[u8; TAR_BLOCK]) -> Result<String> {
    fn parse_field<'a>(raw: &'a [u8], label: &str) -> Result<&'a str> {
        let end = raw
            .iter()
            .position(|&byte| byte == 0)
            .with_context(|| format!("tar {label} field has no NUL terminator"))?;
        std::str::from_utf8(&raw[..end]).with_context(|| format!("tar {label} field is not UTF-8"))
    }

    let name = parse_field(&header[NAME_OFFSET..NAME_OFFSET + NAME_LEN], "name")?;
    ensure!(!name.is_empty(), "tar name field is empty");
    let prefix = parse_field(&header[PREFIX_OFFSET..PREFIX_OFFSET + PREFIX_LEN], "prefix")?;
    if prefix.is_empty() {
        Ok(name.to_string())
    } else {
        Ok(format!("{prefix}/{name}"))
    }
}

/// Parse the octal `size` field (bytes 124..136) of a tar header.
///
/// The POSIX/`ustar` size field is octal ASCII digits that may be surrounded by
/// spaces and terminated by a space or NUL; different tar implementations
/// left-pad, right-pad, or both. The field is therefore cut at its first NUL
/// only (never at an interior space, which would drop a leading-space-padded
/// value), then ASCII-trimmed before the octal parse. An all-NUL or all-space
/// field trims to empty and fails loud.
fn parse_octal_size(header: &[u8; TAR_BLOCK]) -> Result<u64> {
    let field = &header[SIZE_OFFSET..SIZE_OFFSET + SIZE_LEN];
    let end = field
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(field.len());
    let text = std::str::from_utf8(&field[..end]).context("tar size field is not ASCII")?;
    let text = text.trim();
    ensure!(!text.is_empty(), "tar size field is empty");
    u64::from_str_radix(text, 8).context("tar size field is not octal")
}

/// Validate the POSIX tar header checksum before trusting any header field.
fn validate_header_checksum(header: &[u8; TAR_BLOCK]) -> Result<()> {
    let field = &header[CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_LEN];
    let text = std::str::from_utf8(field).context("tar checksum field is not ASCII")?;
    let text = text.trim_matches(['\0', ' ']);
    ensure!(!text.is_empty(), "tar checksum field is empty");
    let expected = u64::from_str_radix(text, 8).context("tar checksum field is not octal")?;
    let actual = tar_header_checksum(header);
    ensure!(
        expected == actual,
        "tar header checksum mismatch: declared {expected:o}, computed {actual:o}"
    );
    Ok(())
}

fn tar_header_checksum(header: &[u8; TAR_BLOCK]) -> u64 {
    header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_LEN).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    /// Build a minimal POSIX `ustar` header block for one regular-file member.
    fn ustar_header(name: &str, size: u64) -> [u8; TAR_BLOCK] {
        let mut header = [0u8; TAR_BLOCK];
        let name_bytes = name.as_bytes();
        assert!(name_bytes.len() <= NAME_LEN, "test member name too long");
        header[NAME_OFFSET..NAME_OFFSET + name_bytes.len()].copy_from_slice(name_bytes);
        // mode / uid / gid: fixed octal values are irrelevant to the reader.
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        // size: 11 octal digits + space terminator (POSIX layout).
        let size_field = format!("{size:011o}");
        header[SIZE_OFFSET..SIZE_OFFSET + 11].copy_from_slice(size_field.as_bytes());
        header[SIZE_OFFSET + 11] = b' ';
        // mtime: zero is fine for the reader.
        header[136..147].copy_from_slice(b"00000000000");
        // typeflag '0' = regular file.
        header[TYPEFLAG_OFFSET] = b'0';
        // ustar magic + version.
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        write_test_checksum(&mut header);
        header
    }

    fn write_test_checksum(header: &mut [u8; TAR_BLOCK]) {
        header[CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_LEN].fill(b' ');
        let checksum = tar_header_checksum(header);
        let checksum_field = format!("{checksum:06o}");
        header[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 6].copy_from_slice(checksum_field.as_bytes());
        header[CHECKSUM_OFFSET + 6] = 0;
        header[CHECKSUM_OFFSET + 7] = b' ';
    }

    /// Append one member (header + data + block padding) to a raw tar buffer.
    fn push_member(tar: &mut Vec<u8>, name: &str, data: &[u8]) {
        tar.extend_from_slice(&ustar_header(name, data.len() as u64));
        tar.extend_from_slice(data);
        let padding = (TAR_BLOCK - data.len() % TAR_BLOCK) % TAR_BLOCK;
        tar.extend(std::iter::repeat_n(0u8, padding));
    }

    /// Gzip-compress a raw tar buffer.
    fn gzip(tar: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(tar).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    /// Build a gzip tar with the given (name, data) members plus end-of-archive
    /// zero padding.
    fn gzip_tar(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, data) in members {
            push_member(&mut tar, name, data);
        }
        // End-of-archive marker: two zero blocks.
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));
        gzip(&tar)
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CollectedMember {
        name: String,
        text: String,
    }

    fn collect(bytes: Vec<u8>, suffix: &str, limit: u64) -> Result<Vec<CollectedMember>> {
        let mut members = Vec::new();
        visit_gzip_tar_members(
            Cursor::new(bytes),
            suffix,
            1 << 20,
            128,
            limit,
            |name, _, reader| {
                let mut text = String::new();
                reader
                    .read_to_string(&mut text)
                    .with_context(|| format!("read test member {name:?}"))?;
                members.push(CollectedMember {
                    name: name.to_string(),
                    text,
                });
                Ok(())
            },
        )?;
        Ok(members)
    }

    #[test]
    fn yields_all_matching_members_in_archive_order() {
        // Three members; the first and third match `.data`, the middle does not.
        let archive = gzip_tar(&[
            ("first.data", b"alpha"),
            ("ignore.meta", b"skip-me"),
            ("third.data", b"gamma"),
        ]);
        let members = collect(archive, ".data", 1024).expect("stream all members");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].name, "first.data");
        assert_eq!(members[0].text, "alpha");
        assert_eq!(members[1].name, "third.data");
        assert_eq!(members[1].text, "gamma");
    }

    #[test]
    fn skips_non_matching_members_without_yielding_them() {
        let archive = gzip_tar(&[
            ("a.meta", b"meta-only-1"),
            ("b.meta", b"meta-only-2"),
            ("c.data", b"payload"),
        ]);
        let members = collect(archive, ".data", 1024).expect("stream matching members only");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "c.data");
        assert_eq!(members[0].text, "payload");
    }

    #[test]
    fn handles_member_data_that_is_an_exact_block_multiple() {
        // A 512-byte member exercises the zero-padding branch (no trailing pad).
        let exact = vec![b'x'; TAR_BLOCK];
        let archive = gzip_tar(&[("exact.data", exact.as_slice())]);
        let members = collect(archive, ".data", 4096).expect("stream block-exact member");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].text.len(), TAR_BLOCK);
    }

    #[test]
    fn rejects_member_exceeding_per_member_limit_naming_the_member() {
        let archive = gzip_tar(&[("small.data", b"ok"), ("huge.data", &[b'y'; 200])]);
        // Bound below the second member so it is rejected; the first must have
        // streamed fine, proving the bound is per-member, not cumulative.
        let err = collect(archive, ".data", 100).expect_err("oversize member must be rejected");
        let message = err.to_string();
        assert!(message.contains("huge.data"), "{message}");
        assert!(message.contains("max_member_bytes"), "{message}");
    }

    #[test]
    fn fails_loud_on_truncated_archive() {
        // Build a valid gzip tar, then chop the compressed bytes mid-stream so
        // the decompressed header/data block runs past the end.
        let archive = gzip_tar(&[("only.data", &[b'z'; 1024])]);
        let truncated = archive[..archive.len() / 2].to_vec();
        let err = collect(truncated, ".data", 4096).expect_err("truncated archive must fail loud");
        // The failure surfaces from the streaming gzip/tar read, not silently.
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn rejects_archive_without_end_of_archive_marker() {
        let mut tar = Vec::new();
        push_member(&mut tar, "only.data", b"payload");
        let err = collect(gzip(&tar), ".data", 4096)
            .expect_err("tar EOF before the zero end marker must fail loud");
        assert!(err.to_string().contains("end-of-archive marker"), "{err}");
    }

    #[test]
    fn rejects_archive_with_only_one_end_of_archive_zero_block() {
        let mut tar = Vec::new();
        push_member(&mut tar, "only.data", b"payload");
        tar.extend_from_slice(&[0; TAR_BLOCK]);
        let err = collect(gzip(&tar), ".data", 4096)
            .expect_err("one zero block is a truncated tar end marker");
        assert!(err.to_string().contains("only one"), "{err}");
    }

    #[test]
    fn rejects_invalid_header_checksum() {
        let mut tar = Vec::new();
        let mut header = ustar_header("only.data", 7);
        header[0] = b'X';
        tar.extend_from_slice(&header);
        tar.extend_from_slice(b"payload");
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK - 7));
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));

        let err = collect(gzip(&tar), ".data", 4096)
            .expect_err("a tar header with a stale checksum must fail closed");
        assert!(err.to_string().contains("checksum"), "{err}");
    }

    #[test]
    fn rejects_gnu_longname_extension_instead_of_silently_skipping_payload() {
        let long_name = format!("{}.data", "nested/segment".repeat(10));
        let mut tar = Vec::new();
        let mut longname_header = ustar_header("././@LongLink", long_name.len() as u64 + 1);
        longname_header[TYPEFLAG_OFFSET] = b'L';
        write_test_checksum(&mut longname_header);
        tar.extend_from_slice(&longname_header);
        tar.extend_from_slice(long_name.as_bytes());
        tar.push(0);
        let longname_padding = (TAR_BLOCK - (long_name.len() + 1) % TAR_BLOCK) % TAR_BLOCK;
        tar.extend(std::iter::repeat_n(0u8, longname_padding));
        push_member(&mut tar, &long_name[..NAME_LEN], b"payload");
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));

        let err = collect(gzip(&tar), ".data", 4096)
            .expect_err("unsupported GNU longname records must fail loud");
        let message = err.to_string();
        assert!(message.contains("member 1"), "{message}");
        assert!(message.contains("longname"), "{message}");
    }

    #[test]
    fn reads_real_bsdtar_gzip_with_posix_prefix_name() {
        // Produced outside this parser with:
        // bsdtar 3.5.3/libarchive 3.7.4
        // `COPYFILE_DISABLE=1 tar --format=ustar -cf long.tar <120-char-dir>/sample.jsonl`
        // followed by `gzip -n -k long.tar`. The path exceeds the legacy
        // 100-byte name field and exercises the POSIX ustar prefix field.
        let encoded =
            include_str!("../tests/fixtures/tar_reader/bsdtar-3.5.3-posix-prefix.tar.gz.hex")
                .split_whitespace()
                .collect::<String>();
        let archive = hex::decode(encoded).expect("committed external tar fixture hex");

        let members = collect(archive, ".jsonl", 4096).expect("read externally produced tar");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, format!("{}/sample.jsonl", "a".repeat(120)));
        assert_eq!(
            members[0].text.as_bytes(),
            b"{\"instrument\":\"BASEQUOTE\",\"action\":\"snapshot\"}\n"
        );
    }

    #[test]
    fn rejects_non_utf8_member_name_instead_of_lossy_matching() {
        let mut tar = Vec::new();
        let mut header = ustar_header("valid.data", 7);
        header[NAME_OFFSET] = 0xff;
        write_test_checksum(&mut header);
        tar.extend_from_slice(&header);
        tar.extend_from_slice(b"payload");
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK - 7));
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));

        let err = collect(gzip(&tar), ".data", 4096)
            .expect_err("non-UTF-8 tar member names must fail loud");
        let message = format!("{err:#}");
        assert!(message.contains("member 1"), "{message}");
        assert!(message.contains("UTF-8"), "{message}");
    }

    #[test]
    fn rejects_unterminated_full_width_member_name() {
        let mut tar = Vec::new();
        let name = format!("{}.data", "x".repeat(NAME_LEN - ".data".len()));
        assert_eq!(name.len(), NAME_LEN);
        let header = ustar_header(&name, 7);
        tar.extend_from_slice(&header);
        tar.extend_from_slice(b"payload");
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK - 7));
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));

        let err = collect(gzip(&tar), ".data", 4096)
            .expect_err("unterminated full-width tar member names must fail loud");
        let message = format!("{err:#}");
        assert!(message.contains("member 1"), "{message}");
        assert!(message.contains("NUL terminator"), "{message}");
    }

    #[test]
    fn rejects_malformed_octal_size_field() {
        let mut tar = Vec::new();
        let mut header = ustar_header("bad.data", 4);
        // Corrupt the size field with a non-octal byte.
        header[SIZE_OFFSET] = b'9';
        header[SIZE_OFFSET + 1] = b'Z';
        write_test_checksum(&mut header);
        tar.extend_from_slice(&header);
        tar.extend_from_slice(b"data");
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK - 4));
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));
        let err =
            collect(gzip(&tar), ".data", 4096).expect_err("malformed octal size must be rejected");
        assert!(err.to_string().contains("octal"), "{err}");
    }

    #[test]
    fn parses_space_padded_octal_size_field() {
        // POSIX/star tars may surround the octal size with spaces (leading,
        // trailing, or both). The field must be cut at NUL only and ASCII-trimmed,
        // not stopped at the first interior space — a leading-space-padded value
        // must still parse to its octal magnitude.
        let mut header = [0u8; TAR_BLOCK];
        header[SIZE_OFFSET..SIZE_OFFSET + 9].copy_from_slice(b"   1750  ");
        let size = parse_octal_size(&header).expect("space-padded octal size parses");
        assert_eq!(size, 0o1750);
    }

    #[test]
    fn streams_member_with_space_padded_size_field() {
        // End-to-end: a real member whose header size field is leading-AND-
        // trailing space padded must be read with the correct length, not
        // rejected as empty. The data length is chosen to match the octal value.
        let data = vec![b'q'; 0o12]; // 0o12 == 10 bytes.
        let mut header = ustar_header("padded.data", data.len() as u64);
        // Overwrite the size field with " 12 " surrounded by spaces, NUL-filled.
        for byte in &mut header[SIZE_OFFSET..SIZE_OFFSET + SIZE_LEN] {
            *byte = 0;
        }
        header[SIZE_OFFSET..SIZE_OFFSET + 4].copy_from_slice(b"  12");
        header[SIZE_OFFSET + 4] = b' ';
        write_test_checksum(&mut header);

        let mut tar = Vec::new();
        tar.extend_from_slice(&header);
        tar.extend_from_slice(&data);
        let padding = (TAR_BLOCK - data.len() % TAR_BLOCK) % TAR_BLOCK;
        tar.extend(std::iter::repeat_n(0u8, padding));
        tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));

        let members = collect(gzip(&tar), ".data", 1024)
            .expect("space-padded size field member streams cleanly");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "padded.data");
        assert_eq!(members[0].text, "q".repeat(10));
    }

    #[test]
    fn rejects_all_space_size_field() {
        // An all-space (or all-NUL) size field trims to empty and must fail loud
        // rather than parsing as zero.
        let mut header = [0u8; TAR_BLOCK];
        for byte in &mut header[SIZE_OFFSET..SIZE_OFFSET + SIZE_LEN] {
            *byte = b' ';
        }
        let err = parse_octal_size(&header).expect_err("all-space size field must fail loud");
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn empty_archive_yields_no_members() {
        let archive = gzip_tar(&[]);
        let members = collect(archive, ".data", 1024).expect("empty archive streams cleanly");
        assert!(members.is_empty());
    }

    #[test]
    fn yields_all_members_across_concatenated_gzip_streams() {
        // A gzip file is legitimately a concatenation of independent gzip
        // members. Build the tar as two raw halves and gzip each half on its
        // own, then concatenate the two gzip streams into one byte vector. The
        // single-stream `GzDecoder` stops at the end of the first gzip member
        // and silently drops the second member's tar bytes; `MultiGzDecoder`
        // must concatenate both gzip streams so the whole tar (both members)
        // is walked.
        let mut first_half = Vec::new();
        push_member(&mut first_half, "first.data", b"alpha");

        let mut second_half = Vec::new();
        push_member(&mut second_half, "second.data", b"omega");
        // End-of-archive marker lives at the very end of the decompressed tar.
        second_half.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));

        let mut concatenated = gzip(&first_half);
        concatenated.extend_from_slice(&gzip(&second_half));

        let members = collect(concatenated, ".data", 1024)
            .expect("multi-member gzip stream walks the whole tar");
        assert_eq!(
            members.len(),
            2,
            "both gzip streams' tar members must be yielded"
        );
        assert_eq!(members[0].name, "first.data");
        assert_eq!(members[0].text, "alpha");
        assert_eq!(members[1].name, "second.data");
        assert_eq!(members[1].text, "omega");
    }
}
