//! Generic ZIP streaming: single-member deflate inflation with CRC-32 and
//! length verification at end-of-stream.
//!
//! Invariant: every byte read from a ZIP member passes through
//! [`ZipMemberReader`], which accumulates the CRC-32 and inflated byte count
//! in a single streaming pass. [`ZipMemberReader::verify`] MUST be called once
//! the stream is fully drained — it fails loud on truncation or corruption
//! rather than silently accepting a partial decode. Multi-GiB members are
//! supported via ZIP64 extended-information parsing (extra-field id `0x0001`).

use std::io::Read;

use anyhow::{Context, Result, bail, ensure};
use flate2::{Crc, read::DeflateDecoder};

use crate::io_safety::{STAGED_DECODED_BYTES, ensure_within_limit, read_to_vec_with_limit};

// ---------------------------------------------------------------------------
// ZIP format constants
// ---------------------------------------------------------------------------

/// ZIP local file header signature (`PK\x03\x04`).
const ZIP_LOCAL_HEADER_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
/// ZIP central directory header signature (`PK\x01\x02`).
const ZIP_CENTRAL_HEADER_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
/// ZIP end-of-central-directory record signature (`PK\x05\x06`).
const ZIP_EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
/// ZIP64 end-of-central-directory locator signature (`PK\x06\x07`).
const ZIP64_EOCD_LOCATOR_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
/// ZIP64 end-of-central-directory record signature (`PK\x06\x06`).
const ZIP64_EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
/// Fixed length of the ZIP end-of-central-directory record (without comment).
const ZIP_EOCD_LEN: usize = 22;
/// Fixed length of the ZIP64 EOCD locator record.
const ZIP64_EOCD_LOCATOR_LEN: usize = 20;
/// Maximum ZIP archive comment length (u16 field, so 65,535 bytes max).
const ZIP_MAX_COMMENT_LEN: usize = 65_535;
/// Fixed length of a ZIP local file header before the variable-length name.
const ZIP_LOCAL_HEADER_LEN: usize = 30;
/// Compression method code for raw DEFLATE.
const ZIP_METHOD_DEFLATE: u16 = 8;
/// Compression method code for STORED (no compression).
const ZIP_METHOD_STORED: u16 = 0;
/// General-purpose flag bit 3: sizes/CRC are zero in the local header and a
/// data descriptor follows the compressed data instead.
const ZIP_FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
/// ZIP64 extended-information extra-field header id.
const ZIP64_EXTRA_ID: u16 = 0x0001;
/// Sentinel a 32-bit ZIP size field carries when the real 64-bit value lives in
/// the ZIP64 extended-information extra field (member exceeds the 4 GiB u32 cap).
const ZIP32_SIZE_SENTINEL: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Little-endian field helpers
// ---------------------------------------------------------------------------

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

// ---------------------------------------------------------------------------
// ZIP64 extended-information parsing
// ---------------------------------------------------------------------------

/// Resolve the real 64-bit `(uncompressed, compressed)` sizes from a local
/// header's ZIP64 extended-information extra field (id `0x0001`).
///
/// Per APPNOTE the block stores the original (uncompressed) size first, then
/// the compressed size, each present only when the corresponding 32-bit field
/// held the `0xFFFFFFFF` sentinel, in that fixed order. Returns `None` for a
/// size whose 32-bit field was not a sentinel — archives whose uncompressed
/// members exceed 4 GiB use this path.
pub fn zip64_sizes(
    extra: &[u8],
    uncompressed_is_sentinel: bool,
    compressed_is_sentinel: bool,
) -> Result<(Option<usize>, Option<usize>)> {
    let mut offset = 0usize;
    while offset + 4 <= extra.len() {
        let id = read_u16_le(extra, offset);
        let block_len = read_u16_le(extra, offset + 2) as usize;
        let body = offset + 4;
        let body_end = body
            .checked_add(block_len)
            .context("ZIP64 extra block length overflow")?;
        ensure!(
            body_end <= extra.len(),
            "ZIP64 extra block extends past the extra field"
        );
        if id == ZIP64_EXTRA_ID {
            let mut cursor = body;
            let mut uncompressed = None;
            if uncompressed_is_sentinel {
                ensure!(
                    cursor + 8 <= body_end,
                    "ZIP64 extra block too short for the uncompressed size"
                );
                let value = u64::from_le_bytes(
                    extra[cursor..cursor + 8]
                        .try_into()
                        .context("ZIP64 uncompressed size: 8-byte slice")?,
                );
                uncompressed =
                    Some(usize::try_from(value).context("ZIP64 uncompressed size exceeds usize")?);
                cursor += 8;
            }
            let mut compressed = None;
            if compressed_is_sentinel {
                ensure!(
                    cursor + 8 <= body_end,
                    "ZIP64 extra block too short for the compressed size"
                );
                let value = u64::from_le_bytes(
                    extra[cursor..cursor + 8]
                        .try_into()
                        .context("ZIP64 compressed size: 8-byte slice")?,
                );
                compressed =
                    Some(usize::try_from(value).context("ZIP64 compressed size exceeds usize")?);
            }
            return Ok((uncompressed, compressed));
        }
        offset = body_end;
    }
    bail!("ZIP64 extra field (id 0x0001) not found despite a 0xFFFFFFFF size sentinel")
}

// ---------------------------------------------------------------------------
// Central-directory fallback (streamed entries)
// ---------------------------------------------------------------------------

/// Locate the EOCD record by scanning backward from the archive tail.
///
/// The EOCD sits within the last `ZIP_EOCD_LEN + ZIP_MAX_COMMENT_LEN` bytes.
/// We scan rightward to find the rightmost `PK\x05\x06` whose embedded comment
/// length is consistent with the record position (uniquely identifying it).
///
/// Returns the byte offset of the EOCD record within `zip`.
fn locate_eocd(zip: &[u8]) -> Result<usize> {
    if zip.len() < ZIP_EOCD_LEN {
        bail!("ZIP archive too short to contain an EOCD record");
    }
    // The EOCD can start as early as zip.len() - ZIP_EOCD_LEN - ZIP_MAX_COMMENT_LEN.
    let search_start = zip.len().saturating_sub(ZIP_EOCD_LEN + ZIP_MAX_COMMENT_LEN);
    // Scan from right-to-left: prefer the RIGHTMOST candidate whose comment
    // length is self-consistent, to handle (rare) EOCD sigs in archive comments.
    let mut candidate = None;
    let search_end = zip.len() - ZIP_EOCD_LEN + 1;
    for offset in (search_start..search_end).rev() {
        if zip[offset..offset + 4] == ZIP_EOCD_SIG {
            let comment_len = read_u16_le(zip, offset + 20) as usize;
            if offset + ZIP_EOCD_LEN + comment_len == zip.len() {
                candidate = Some(offset);
                break;
            }
        }
    }
    candidate.with_context(|| "ZIP end-of-central-directory record not found")
}

/// Locate the single member's compressed size by reading the ZIP central
/// directory via the EOCD record, used when the local header advertised
/// streaming sizes (general-purpose flag bit 3 set).
///
/// Locates the EOCD from the archive tail (never scanning forward through
/// compressed payload bytes), then follows the EOCD's central-directory offset
/// to the first central-directory record. Handles the ZIP64 EOCD locator path
/// for archives whose central directory offset itself exceeds 4 GiB.
///
/// Returns `(compressed_size, uncompressed_size, crc32)`.
fn central_directory_member(zip: &[u8]) -> Result<(usize, usize, u32)> {
    let eocd_offset = locate_eocd(zip)?;

    // Check whether a ZIP64 EOCD locator precedes the EOCD.  The locator is
    // ZIP64_EOCD_LOCATOR_LEN bytes and immediately precedes the EOCD when present.
    let cd_offset: usize = if eocd_offset >= ZIP64_EOCD_LOCATOR_LEN
        && zip[eocd_offset - ZIP64_EOCD_LOCATOR_LEN..eocd_offset - ZIP64_EOCD_LOCATOR_LEN + 4]
            == ZIP64_EOCD_LOCATOR_SIG
    {
        // ZIP64 path: read the ZIP64 EOCD offset from the locator (field at +8,
        // 8 bytes), then read the central-directory offset from the ZIP64 EOCD
        // record (field at +48, 8 bytes).
        let locator_offset = eocd_offset - ZIP64_EOCD_LOCATOR_LEN;
        let zip64_eocd_offset_raw = u64::from_le_bytes(
            zip[locator_offset + 8..locator_offset + 16]
                .try_into()
                .context("ZIP64 EOCD locator: 8-byte offset slice")?,
        );
        let zip64_eocd_offset =
            usize::try_from(zip64_eocd_offset_raw).context("ZIP64 EOCD offset exceeds usize")?;
        ensure!(
            zip64_eocd_offset + 56 <= zip.len(),
            "ZIP64 EOCD record extends past archive end"
        );
        ensure!(
            zip[zip64_eocd_offset..zip64_eocd_offset + 4] == ZIP64_EOCD_SIG,
            "ZIP64 EOCD signature mismatch"
        );
        let cd_offset_raw = u64::from_le_bytes(
            zip[zip64_eocd_offset + 48..zip64_eocd_offset + 56]
                .try_into()
                .context("ZIP64 EOCD: 8-byte cd-offset slice")?,
        );
        usize::try_from(cd_offset_raw).context("ZIP64 central-directory offset exceeds usize")?
    } else {
        // Standard path: EOCD field at +16 is the 4-byte central-directory offset.
        read_u32_le(zip, eocd_offset + 16) as usize
    };

    // The central directory starts at cd_offset; the first record describes the
    // single member.  Central directory record layout (APPNOTE 4.3.12):
    //   +0  4 bytes  signature
    //   +16 4 bytes  crc-32
    //   +20 4 bytes  compressed size
    //   +24 4 bytes  uncompressed size
    ensure!(
        cd_offset + 46 <= zip.len(),
        "ZIP central directory record extends past archive end"
    );
    ensure!(
        zip[cd_offset..cd_offset + 4] == ZIP_CENTRAL_HEADER_SIG,
        "ZIP central directory signature mismatch at resolved offset"
    );
    let crc = read_u32_le(zip, cd_offset + 16);
    let csize = read_u32_le(zip, cd_offset + 20) as usize;
    let usize_field = read_u32_le(zip, cd_offset + 24) as usize;
    Ok((csize, usize_field, crc))
}

// ---------------------------------------------------------------------------
// ZipMemberReader
// ---------------------------------------------------------------------------

/// A streaming reader over the single member of a ZIP archive.
///
/// Inflates DEFLATE (or passes through STORED) on the fly while accumulating
/// the CRC-32 and inflated byte count, so a multi-GiB member is consumed in
/// bounded chunks and a corrupt or truncated member fails loud at
/// end-of-stream — without ever holding the whole inflated body in memory.
/// [`extract_csv_from_zip`] reads it whole for small objects; large members
/// can be streamed through the reader directly.
///
/// [`ZipMemberReader::verify`] MUST be called once the stream is fully
/// drained; it checks the inflated length and CRC-32 against the archive's
/// declared values.
pub struct ZipMemberReader<'a> {
    source: ZipMemberSource<'a>,
    hasher: Crc,
    inflated_len: u64,
    declared_uncompressed_len: u64,
    declared_crc: u32,
}

enum ZipMemberSource<'a> {
    Deflate(DeflateDecoder<&'a [u8]>),
    Stored(&'a [u8]),
}

impl ZipMemberReader<'_> {
    /// The member's declared uncompressed length, for sizing a whole-buffer
    /// read (`Vec::with_capacity`).
    pub fn declared_len(&self) -> usize {
        usize::try_from(self.declared_uncompressed_len).unwrap_or(usize::MAX)
    }

    /// Verify the fully drained member against its declared length and CRC-32.
    ///
    /// # Errors
    ///
    /// Returns an error if the inflated byte count or CRC-32 does not match
    /// the archive's declared values (a truncated or corrupt member).
    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.inflated_len == self.declared_uncompressed_len,
            "ZIP member inflated to {} bytes, header declared {}",
            self.inflated_len,
            self.declared_uncompressed_len
        );
        let computed = self.hasher.sum();
        ensure!(
            computed == self.declared_crc,
            "ZIP member CRC-32 mismatch (computed {computed:#010x}, declared {:#010x})",
            self.declared_crc
        );
        Ok(())
    }
}

impl Read for ZipMemberReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = match &mut self.source {
            ZipMemberSource::Deflate(decoder) => decoder.read(buf)?,
            ZipMemberSource::Stored(rest) => rest.read(buf)?,
        };
        self.hasher.update(&buf[..read]);
        self.inflated_len += read as u64;
        Ok(read)
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Open a streaming [`ZipMemberReader`] over the single member of a ZIP
/// archive.
///
/// The archive must carry exactly one regular file compressed with DEFLATE or
/// STORED. The local file header is parsed directly; when the header
/// advertises streaming sizes (general-purpose flag bit 3), the member's
/// sizes and CRC are recovered from the central directory; 64-bit sizes are
/// read from the ZIP64 extra field for archives whose uncompressed members
/// exceed 4 GiB. The returned reader is positioned at the member's compressed
/// data and verifies CRC-32 + length on [`ZipMemberReader::verify`].
///
/// # Errors
///
/// Returns an error if the signature is wrong, the member extends past the
/// archive, or the compression method is unsupported.
pub fn zip_member_reader(zip_bytes: &[u8]) -> Result<ZipMemberReader<'_>> {
    ensure!(
        zip_bytes.len() >= ZIP_LOCAL_HEADER_LEN,
        "ZIP archive is shorter than a local file header"
    );
    ensure!(
        zip_bytes[0..4] == ZIP_LOCAL_HEADER_SIG,
        "missing ZIP local file header signature"
    );

    let flags = read_u16_le(zip_bytes, 6);
    let method = read_u16_le(zip_bytes, 8);
    let mut crc = read_u32_le(zip_bytes, 14);
    let mut compressed_size = read_u32_le(zip_bytes, 18) as usize;
    let mut uncompressed_size = read_u32_le(zip_bytes, 22) as usize;
    let name_len = read_u16_le(zip_bytes, 26) as usize;
    let extra_len = read_u16_le(zip_bytes, 28) as usize;

    // Streamed entry: the real size/CRC live in the central directory.
    if flags & ZIP_FLAG_DATA_DESCRIPTOR != 0 {
        let (csize, usize_field, central_crc) = central_directory_member(zip_bytes)?;
        compressed_size = csize;
        uncompressed_size = usize_field;
        crc = central_crc;
    } else if compressed_size == ZIP32_SIZE_SENTINEL as usize
        || uncompressed_size == ZIP32_SIZE_SENTINEL as usize
    {
        // ZIP64 member: the 32-bit size fields hold the 0xFFFFFFFF sentinel
        // and the real 64-bit sizes live in the local header's ZIP64 extra
        // field (archives with members that exceed 4 GiB uncompressed).
        let extra_start = ZIP_LOCAL_HEADER_LEN
            .checked_add(name_len)
            .context("ZIP local header name length overflow")?;
        let extra_end = extra_start
            .checked_add(extra_len)
            .context("ZIP local header extra length overflow")?;
        ensure!(
            extra_end <= zip_bytes.len(),
            "ZIP local header extra field extends past archive end"
        );
        let (uncompressed64, compressed64) = zip64_sizes(
            &zip_bytes[extra_start..extra_end],
            uncompressed_size == ZIP32_SIZE_SENTINEL as usize,
            compressed_size == ZIP32_SIZE_SENTINEL as usize,
        )?;
        if let Some(value) = uncompressed64 {
            uncompressed_size = value;
        }
        if let Some(value) = compressed64 {
            compressed_size = value;
        }
    }

    let data_start = ZIP_LOCAL_HEADER_LEN
        .checked_add(name_len)
        .and_then(|value| value.checked_add(extra_len))
        .context("ZIP local header length overflow")?;
    let data_end = data_start
        .checked_add(compressed_size)
        .context("ZIP member size overflow")?;
    ensure!(
        data_end <= zip_bytes.len(),
        "ZIP member extends past archive end (need {data_end}, have {})",
        zip_bytes.len()
    );
    let compressed = &zip_bytes[data_start..data_end];

    let source = match method {
        ZIP_METHOD_DEFLATE => ZipMemberSource::Deflate(DeflateDecoder::new(compressed)),
        ZIP_METHOD_STORED => ZipMemberSource::Stored(compressed),
        other => bail!("unsupported ZIP compression method {other}"),
    };

    Ok(ZipMemberReader {
        source,
        hasher: Crc::new(),
        inflated_len: 0,
        declared_uncompressed_len: uncompressed_size as u64,
        declared_crc: crc,
    })
}

/// Extract the single CSV member of a ZIP archive and return its decompressed
/// UTF-8 text.
///
/// Reads the whole member through [`zip_member_reader`], verifying CRC-32 and
/// length at end-of-stream — the small-object path. Large members should
/// stream through [`zip_member_reader`] directly rather than materialising the
/// whole text.
///
/// # Errors
///
/// Returns an error if the archive is malformed, the member extends past the
/// archive, inflation fails, the CRC or length mismatches, or the bytes are
/// not valid UTF-8.
pub fn extract_csv_from_zip(zip_bytes: &[u8]) -> Result<String> {
    let mut reader = zip_member_reader(zip_bytes)?;
    ensure_within_limit(
        "ZIP member declared size",
        u64::try_from(reader.declared_len()).context("ZIP member declared size exceeds u64")?,
        STAGED_DECODED_BYTES,
    )?;
    let inflated = read_to_vec_with_limit(&mut reader, STAGED_DECODED_BYTES, "inflate ZIP member")?;
    reader.verify()?;
    String::from_utf8(inflated).context("ZIP CSV member is not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::{
        ZIP_CENTRAL_HEADER_SIG, ZIP_EOCD_SIG, ZIP_FLAG_DATA_DESCRIPTOR, ZIP_LOCAL_HEADER_SIG,
        ZIP_METHOD_DEFLATE, ZIP_METHOD_STORED, ZIP32_SIZE_SENTINEL, ZIP64_EXTRA_ID,
        extract_csv_from_zip, zip_member_reader, zip64_sizes,
    };
    use flate2::Crc;

    #[test]
    fn zip_extractor_rejects_bad_signature() {
        let err = extract_csv_from_zip(b"not a zip archive at all............").unwrap_err();
        assert!(err.to_string().contains("signature"), "{err}");
    }

    #[test]
    fn zip64_extended_info_parsed_for_large_members() {
        // A STORED member whose local-header 32-bit size fields hold the
        // 0xFFFFFFFF sentinel, with the real sizes in a ZIP64 extra field —
        // the shape archives with members exceeding 4 GiB uncompressed take.
        // A tiny payload exercises the ZIP64 path without a 4 GiB fixture.
        let content = b"a,b,c\n1,2,3\n";
        let mut hasher = Crc::new();
        hasher.update(content);
        let crc = hasher.sum();
        let name = b"x";

        let mut zip = Vec::new();
        zip.extend_from_slice(&ZIP_LOCAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&ZIP32_SIZE_SENTINEL.to_le_bytes()); // compressed sentinel
        zip.extend_from_slice(&ZIP32_SIZE_SENTINEL.to_le_bytes()); // uncompressed sentinel
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&20u16.to_le_bytes()); // extra_len: 4 header + 16 data
        zip.extend_from_slice(name);
        // ZIP64 extra: id, block length, then uncompressed size, compressed size.
        zip.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
        zip.extend_from_slice(&16u16.to_le_bytes());
        zip.extend_from_slice(&(content.len() as u64).to_le_bytes());
        zip.extend_from_slice(&(content.len() as u64).to_le_bytes());
        zip.extend_from_slice(content);

        let csv = extract_csv_from_zip(&zip).expect("ZIP64 member extracts");
        assert_eq!(csv, "a,b,c\n1,2,3\n");
    }

    #[test]
    fn zip64_sizes_returns_none_for_non_sentinel_fields() {
        // Build a ZIP64 extra block with both uncompressed and compressed sizes.
        let mut extra = Vec::new();
        extra.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
        extra.extend_from_slice(&16u16.to_le_bytes()); // block_len: 2 × u64
        extra.extend_from_slice(&42u64.to_le_bytes()); // uncompressed
        extra.extend_from_slice(&10u64.to_le_bytes()); // compressed

        // When neither field is a sentinel, neither is returned.
        let (u, c) = zip64_sizes(&extra, false, false).unwrap();
        assert_eq!(u, None);
        assert_eq!(c, None);

        // When only the uncompressed field is a sentinel, only it is read.
        let (u, c) = zip64_sizes(&extra, true, false).unwrap();
        assert_eq!(u, Some(42));
        assert_eq!(c, None);

        // When both are sentinels, both are read.
        let (u, c) = zip64_sizes(&extra, true, true).unwrap();
        assert_eq!(u, Some(42));
        assert_eq!(c, Some(10));
    }

    #[test]
    fn zip_extractor_rejects_truncated_member() {
        // Build a valid STORED archive then truncate the payload — verify()
        // must fail loud on length mismatch.
        let content = b"col1,col2\nval1,val2\n";
        let mut hasher = Crc::new();
        hasher.update(content);
        let crc = hasher.sum();
        let name = b"data.csv";

        let mut zip = Vec::new();
        zip.extend_from_slice(&ZIP_LOCAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes());
        // Declare the full compressed and uncompressed sizes …
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra_len
        zip.extend_from_slice(name);
        // … but only append half the payload bytes.
        zip.extend_from_slice(&content[..content.len() / 2]);
        // Pad so the declared compressed_size is satisfied at the slice level,
        // but the content is still wrong (wrong CRC and wrong inflated length).
        zip.extend_from_slice(&vec![0u8; content.len() - content.len() / 2]);

        // extract_csv_from_zip materialises and then verifies — the verify
        // step catches the CRC mismatch on the zeroed padding.
        let err = extract_csv_from_zip(&zip).unwrap_err();
        assert!(
            err.to_string().contains("CRC-32") || err.to_string().contains("inflated"),
            "expected CRC or length error, got: {err}"
        );
    }

    /// Build a minimal valid STORED archive with a real EOCD, central directory,
    /// and a local member whose compressed data contains the 4-byte sequence
    /// `PK\x01\x02` (the central-directory signature) embedded in the payload.
    ///
    /// The old forward-scan implementation would false-match on that embedded
    /// signature and return garbage sizes/CRC.  The EOCD-anchored implementation
    /// must ignore the embedded signature and return the correct member content.
    fn build_zip_with_embedded_cd_sig_in_payload(content: &[u8]) -> Vec<u8> {
        // Embed ZIP_CENTRAL_HEADER_SIG bytes in the payload so a naive
        // forward scan would hit them before the real central directory.
        // We do this by prepending the 4-byte signature to the content.
        let mut payload = Vec::new();
        payload.extend_from_slice(&ZIP_CENTRAL_HEADER_SIG); // fake CD sig at byte 0
        payload.extend_from_slice(content);

        let mut hasher = Crc::new();
        hasher.update(&payload);
        let crc = hasher.sum();
        let name = b"data.csv";

        // -- Local file header --
        let mut zip = Vec::new();
        zip.extend_from_slice(&ZIP_LOCAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&ZIP_FLAG_DATA_DESCRIPTOR.to_le_bytes()); // bit 3 set → streamed
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&0u32.to_le_bytes()); // crc (zero in local header for streamed)
        zip.extend_from_slice(&0u32.to_le_bytes()); // compressed size (zero for streamed)
        zip.extend_from_slice(&0u32.to_le_bytes()); // uncompressed size (zero for streamed)
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra_len
        zip.extend_from_slice(name);
        // Payload contains the embedded fake signature followed by real content.
        zip.extend_from_slice(&payload);

        // -- Central directory record --
        let cd_offset = zip.len() as u32;
        zip.extend_from_slice(&ZIP_CENTRAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version made by
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&ZIP_FLAG_DATA_DESCRIPTOR.to_le_bytes()); // flags
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // compressed size
        zip.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra_len
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment_len
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        zip.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        zip.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        zip.extend_from_slice(&0u32.to_le_bytes()); // local header offset
        zip.extend_from_slice(name);

        // -- EOCD record --
        let cd_size = (zip.len() as u32) - cd_offset;
        zip.extend_from_slice(&ZIP_EOCD_SIG);
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk with cd
        zip.extend_from_slice(&1u16.to_le_bytes()); // entries on disk
        zip.extend_from_slice(&1u16.to_le_bytes()); // total entries
        zip.extend_from_slice(&cd_size.to_le_bytes());
        zip.extend_from_slice(&cd_offset.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment_len

        zip
    }

    #[test]
    fn eocd_anchored_scan_ignores_embedded_cd_signature_in_payload() {
        // Finding 1: a naive forward scan from offset 0 would false-match the
        // embedded PK\x01\x02 inside the payload bytes.  The EOCD-anchored scan
        // must locate the real central directory via the EOCD offset field and
        // return the correct content.
        let content = b"col1,col2\nval1,val2\n";
        let zip = build_zip_with_embedded_cd_sig_in_payload(content);

        // The extracted bytes are the payload = [PK\x01\x02] ++ content.
        // We verify that extraction succeeds and returns the full payload
        // (including the embedded sig bytes) — proving the real CD was found.
        let mut reader = zip_member_reader(&zip).expect("should parse with EOCD-anchored scan");
        reader.verify().expect("should verify correctly");

        let mut buf = Vec::new();
        use std::io::Read as _;
        // Re-open to drain (verify consumes nothing on its own).
        let mut reader2 = zip_member_reader(&zip).expect("second open");
        reader2.read_to_end(&mut buf).expect("read");
        reader2.verify().expect("verify after drain");

        let mut expected = Vec::new();
        expected.extend_from_slice(&ZIP_CENTRAL_HEADER_SIG);
        expected.extend_from_slice(content);
        assert_eq!(
            buf, expected,
            "payload including embedded sig must be returned intact"
        );
    }

    #[test]
    fn verify_rejects_length_mismatch_with_correct_crc() {
        // Finding 2: construct a ZipMemberReader input where the CRC of the
        // supplied bytes is CORRECT for the supplied bytes, but the declared
        // uncompressed length differs — so the length branch fires, not the CRC
        // branch.
        //
        // We do this by building a STORED archive where:
        //   - declared uncompressed_size = content.len() + 1  (one byte MORE)
        //   - compressed_size            = content.len()       (actual payload length)
        //   - crc                        = CRC of the actual payload
        //
        // After inflation inflated_len == content.len() but declared == content.len()+1,
        // so the length ensure! fires.  The CRC of the inflated bytes matches the
        // declared CRC, so the CRC branch would NOT fire if we reached it.
        let content = b"col1,col2\nval1,val2\n";
        let mut hasher = Crc::new();
        hasher.update(content);
        let crc = hasher.sum();
        let name = b"data.csv";

        let declared_uncompressed = content.len() as u32 + 1; // one byte longer than actual

        let mut zip = Vec::new();
        zip.extend_from_slice(&ZIP_LOCAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags (not streamed)
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC matches the ACTUAL payload
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes()); // compressed = actual len
        zip.extend_from_slice(&declared_uncompressed.to_le_bytes()); // uncompressed = actual+1
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra_len
        zip.extend_from_slice(name);
        zip.extend_from_slice(content); // the real payload

        // Pad archive so data_end <= zip.len() (otherwise the bounds check fires first).
        zip.push(0u8); // one extra byte so the declared slice fits

        let err = extract_csv_from_zip(&zip).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("inflated"),
            "expected length-mismatch ('inflated') error, got: {err}"
        );
        assert!(
            !msg.contains("CRC-32"),
            "CRC branch must not fire when length-mismatch fires first, got: {err}"
        );
    }

    /// Build a STORED archive with general-purpose flag bit 3 set (data-descriptor /
    /// streamed entry), with a valid central directory and EOCD so that the
    /// `central_directory_member` path is exercised.
    fn build_streamed_stored_zip(content: &[u8]) -> Vec<u8> {
        let mut hasher = Crc::new();
        hasher.update(content);
        let crc = hasher.sum();
        let name = b"data.csv";

        // -- Local file header (bit 3 set, sizes/crc zeroed) --
        let mut zip = Vec::new();
        zip.extend_from_slice(&ZIP_LOCAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&ZIP_FLAG_DATA_DESCRIPTOR.to_le_bytes()); // bit 3
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&0u32.to_le_bytes()); // crc (zeroed in local header)
        zip.extend_from_slice(&0u32.to_le_bytes()); // compressed size (zeroed)
        zip.extend_from_slice(&0u32.to_le_bytes()); // uncompressed size (zeroed)
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra_len
        zip.extend_from_slice(name);
        zip.extend_from_slice(content);

        // -- Central directory record (has real sizes/crc) --
        let cd_offset = zip.len() as u32;
        zip.extend_from_slice(&ZIP_CENTRAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version made by
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&ZIP_FLAG_DATA_DESCRIPTOR.to_le_bytes());
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes()); // compressed size
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra_len
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment_len
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        zip.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        zip.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        zip.extend_from_slice(&0u32.to_le_bytes()); // local header offset
        zip.extend_from_slice(name);

        // -- EOCD record --
        let cd_size = (zip.len() as u32) - cd_offset;
        zip.extend_from_slice(&ZIP_EOCD_SIG);
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk with cd
        zip.extend_from_slice(&1u16.to_le_bytes()); // entries on disk
        zip.extend_from_slice(&1u16.to_le_bytes()); // total entries
        zip.extend_from_slice(&cd_size.to_le_bytes());
        zip.extend_from_slice(&cd_offset.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment_len

        zip
    }

    #[test]
    fn data_descriptor_path_extracts_correct_content() {
        // Finding 3: general-purpose flag bit 3 (data-descriptor / streamed entry)
        // exercises the central_directory_member fallback path.  Verify that a
        // STORED member with bit 3 set is correctly extracted end-to-end.
        let content = b"ticker,price,size\nBTC,50000.0,1.5\n";
        let zip = build_streamed_stored_zip(content);

        let csv = extract_csv_from_zip(&zip).expect("streamed STORED member must extract");
        assert_eq!(csv.as_bytes(), content);
    }

    #[test]
    fn data_descriptor_path_rejects_corrupt_content() {
        // Additional negative case for the data-descriptor path: corrupt payload
        // must fail loud (not silently pass) when CRC mismatches.
        let content = b"ticker,price,size\nBTC,50000.0,1.5\n";
        let mut zip = build_streamed_stored_zip(content);

        // Flip a byte in the payload region (after the local header + name).
        // local header = 30 bytes, name = 8 bytes ("data.csv") → payload at byte 38.
        zip[38] ^= 0xFF;

        let err = extract_csv_from_zip(&zip).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("CRC-32") || msg.contains("inflated"),
            "corrupt streamed member must fail loud, got: {err}"
        );
    }
}
