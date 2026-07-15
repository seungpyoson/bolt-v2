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
//!    yielded, in archive order, and non-matching members are skipped by
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
//! Each yielded member's content read is bounded by `max_member_bytes` and fails
//! loud naming the offending member when exceeded, so one pathological member
//! cannot blow the bound for the rest of the archive. A truncated archive (a
//! header or data block that runs past the end of the stream) also fails loud.
//!
//! This module owns only the container concern (decompress + walk tar members);
//! it does not parse JSONL or know anything about the order-book-delta wire
//! shape. The normalize path in
//! [`super::canonical_order_book_deltas`] consumes the member iterator.

use std::io::Read;

use anyhow::{Context, Result, bail, ensure};
use flate2::read::MultiGzDecoder;

/// POSIX tar block size: headers and data are laid out in 512-byte blocks.
const TAR_BLOCK: usize = 512;

/// Byte offset of the `name` field within a tar header block.
const NAME_OFFSET: usize = 0;

/// Length of the `name` field within a tar header block.
const NAME_LEN: usize = 100;

/// Byte offset of the octal `size` field within a tar header block.
const SIZE_OFFSET: usize = 124;

/// Length of the octal `size` field within a tar header block.
const SIZE_LEN: usize = 12;

/// Byte offset of the `typeflag` field within a tar header block.
const TYPEFLAG_OFFSET: usize = 156;

/// One extracted tar member: its name and decoded UTF-8 text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarMember {
    pub name: String,
    pub text: String,
}

/// Stream the members of a gzip-compressed POSIX tar whose names end with
/// `member_suffix`.
///
/// Decompression and tar walking are both streaming: `reader` is wrapped in a
/// [`MultiGzDecoder`] and tar entries are read sequentially from the decompressed
/// stream, so the whole archive is never held in memory. A multi-member gzip
/// stream is transparently concatenated, so no tar member is lost when the gzip
/// was written as several streams. Each *matching*
/// member's text is read under a `max_member_bytes` bound (per member, not
/// cumulative); a non-matching member's bytes are consumed and discarded.
///
/// Members are yielded in archive order. The returned iterator yields `Err` for
/// the first malformed/oversize/truncated member and then stops; callers that
/// must process the whole archive should treat any `Err` as fatal.
pub fn gzip_tar_members<R: Read>(
    reader: R,
    member_suffix: &str,
    max_member_bytes: u64,
) -> TarMembers<MultiGzDecoder<R>> {
    tar_members(MultiGzDecoder::new(reader), member_suffix, max_member_bytes)
}

/// Walk an already-decoded tar stream through the same bounded member parser.
///
/// This seam lets callers wrap the decompressed byte stream with cooperative
/// read checks while preserving one tar parsing implementation.
pub fn tar_members<R: Read>(
    reader: R,
    member_suffix: &str,
    max_member_bytes: u64,
) -> TarMembers<R> {
    TarMembers {
        reader,
        member_suffix: member_suffix.to_string(),
        max_member_bytes,
        done: false,
    }
}

/// Streaming iterator over the matching members of a gzip-compressed POSIX tar.
///
/// Created by [`gzip_tar_members`]. Holds the streaming [`MultiGzDecoder`] and the
/// member filter; it never buffers more than one member's bytes at a time.
pub struct TarMembers<R: Read> {
    reader: R,
    member_suffix: String,
    max_member_bytes: u64,
    done: bool,
}

impl<R: Read> TarMembers<R> {
    /// Advance to and return the next matching member, or `Ok(None)` at
    /// end-of-archive.
    ///
    /// Skips non-matching members by consuming their padded data blocks without
    /// retention. Fails loud on a truncated header/data block, a malformed
    /// octal size, or a matching member whose text exceeds `max_member_bytes`.
    fn next_member(&mut self) -> Result<Option<TarMember>> {
        loop {
            let mut header = [0u8; TAR_BLOCK];
            match read_full_block(&mut self.reader, &mut header)? {
                BlockRead::Eof => return Ok(None),
                BlockRead::Truncated(read) => {
                    bail!("tar header truncated: read {read} of {TAR_BLOCK} bytes")
                }
                BlockRead::Full => {}
            }

            // An all-zero block marks the end-of-archive padding; stop the walk.
            if header.iter().all(|&byte| byte == 0) {
                return Ok(None);
            }

            let name = parse_name(&header);
            let size = parse_octal_size(&header)?;
            let typeflag = header[TYPEFLAG_OFFSET];
            // typeflag '0' or NUL = regular file; only regular files carry the
            // JSONL member payload. Directory/link/extended entries are walked
            // past by their (block-padded) size like any other member.
            let is_regular_file = typeflag == b'0' || typeflag == 0;
            let matches = is_regular_file && name.ends_with(self.member_suffix.as_str());

            if matches {
                let text = self.read_member_text(&name, size)?;
                return Ok(Some(TarMember { name, text }));
            }

            // Non-matching member: consume its data (block-padded) without
            // retaining it, then continue to the next header.
            self.skip_member_data(&name, size)?;
        }
    }

    /// Read one matching member's `size` data bytes under the per-member bound,
    /// then consume its block padding, returning the decoded UTF-8 text.
    fn read_member_text(&mut self, name: &str, size: u64) -> Result<String> {
        ensure!(
            size <= self.max_member_bytes,
            "tar member {name:?} declares {size} bytes, exceeding per-member limit {}",
            self.max_member_bytes
        );
        let mut bytes = vec![0u8; usize_from_size(size, name)?];
        read_exact_member(&mut self.reader, &mut bytes, name)?;
        consume_padding(&mut self.reader, size, name)?;
        String::from_utf8(bytes).with_context(|| format!("tar member {name:?} is not valid UTF-8"))
    }

    /// Consume a non-matching member's `size` data bytes plus block padding
    /// without retaining them, so memory stays bounded regardless of the
    /// member's declared size.
    fn skip_member_data(&mut self, name: &str, size: u64) -> Result<()> {
        let total = padded_len(size, name)?;
        let consumed = std::io::copy(&mut (&mut self.reader).take(total), &mut std::io::sink())
            .with_context(|| format!("skip tar member {name:?}"))?;
        ensure!(
            consumed == total,
            "tar member {name:?} truncated: skipped {consumed} of {total} bytes"
        );
        Ok(())
    }
}

impl<R: Read> Iterator for TarMembers<R> {
    type Item = Result<TarMember>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.next_member() {
            Ok(Some(member)) => Some(Ok(member)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
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

/// Read exactly `buffer.len()` bytes for a matching member, failing loud if the
/// stream ends early (truncated archive).
fn read_exact_member<R: Read>(reader: &mut R, buffer: &mut [u8], name: &str) -> Result<()> {
    let mut filled = 0usize;
    while filled < buffer.len() {
        let read = reader
            .read(&mut buffer[filled..])
            .with_context(|| format!("read tar member {name:?} data"))?;
        ensure!(
            read != 0,
            "tar member {name:?} truncated: read {filled} of {} bytes",
            buffer.len()
        );
        filled += read;
    }
    Ok(())
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

/// Narrow a member's declared `u64` size to `usize` for buffer allocation,
/// failing loud on platforms where it does not fit.
fn usize_from_size(size: u64, name: &str) -> Result<usize> {
    usize::try_from(size)
        .with_context(|| format!("tar member {name:?} size {size} exceeds addressable memory"))
}

/// Parse the NUL-terminated `name` field of a tar header.
fn parse_name(header: &[u8; TAR_BLOCK]) -> String {
    let raw = &header[NAME_OFFSET..NAME_OFFSET + NAME_LEN];
    let end = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
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
        // Checksum: tar checksum is the sum of all header bytes with the
        // checksum field treated as spaces. The streaming reader does not
        // verify it, but a real tar layout carries it, so fill it faithfully.
        header[148..156].copy_from_slice(b"        ");
        let checksum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
        let checksum_field = format!("{checksum:06o}");
        header[148..154].copy_from_slice(checksum_field.as_bytes());
        header[154] = 0;
        header[155] = b' ';
        header
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

    fn collect(bytes: Vec<u8>, suffix: &str, limit: u64) -> Result<Vec<TarMember>> {
        gzip_tar_members(Cursor::new(bytes), suffix, limit).collect()
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
        assert!(message.contains("per-member limit"), "{message}");
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
    fn rejects_malformed_octal_size_field() {
        let mut tar = Vec::new();
        let mut header = ustar_header("bad.data", 4);
        // Corrupt the size field with a non-octal byte.
        header[SIZE_OFFSET] = b'9';
        header[SIZE_OFFSET + 1] = b'Z';
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
