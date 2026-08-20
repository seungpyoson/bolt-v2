//! Bounded, encounter-ordered JSONL record visitation across accepted archive
//! containers.

use std::io::{BufRead, BufReader, Cursor, Read};

use anyhow::{Context, Result, bail, ensure};

use crate::canonical_trades::{RawPayloadConfig, RawPayloadContainer};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Fail-closed bounds for one JSONL archive scan.
pub struct JsonlStreamLimits {
    /// Maximum cumulative decompressed bytes consumed from the container.
    pub max_decoded_bytes: u64,
    /// Maximum archive members, including members skipped by suffix filtering.
    pub max_members: u64,
    /// Maximum decompressed bytes in any one member.
    pub max_member_bytes: u64,
    /// Maximum bytes in one JSONL record, excluding its line ending.
    pub max_record_bytes: usize,
    /// Maximum nonempty JSONL records across the container.
    pub max_records: u64,
    /// Required member-name suffix for tar containers; unused otherwise.
    pub member_suffix: Option<String>,
}

/// Resolve the single TOML-owned JSONL limit group into runtime limits for the
/// shared visitor.
pub fn limits_from_raw_payload(raw: &RawPayloadConfig) -> Result<JsonlStreamLimits> {
    let stream = raw
        .jsonl_stream
        .as_ref()
        .context("converter.raw_payload.jsonl_stream is required")?;
    let (max_member_bytes, member_suffix) = match raw.container {
        RawPayloadContainer::TarGzipJsonl => (
            raw.max_member_bytes
                .context("tar_gzip_jsonl requires raw_payload.max_member_bytes")?,
            Some(
                raw.member_suffix
                    .clone()
                    .context("tar_gzip_jsonl requires raw_payload.member_suffix")?,
            ),
        ),
        RawPayloadContainer::JsonlText
        | RawPayloadContainer::JsonlGzip
        | RawPayloadContainer::SingleJsonlZip => (raw.max_decoded_bytes, None),
        other => bail!("bounded JSONL visitor does not support container {other:?}"),
    };
    let limits = JsonlStreamLimits {
        max_decoded_bytes: raw.max_decoded_bytes,
        max_members: stream.max_members,
        max_member_bytes,
        max_record_bytes: stream.max_record_bytes,
        max_records: stream.max_records,
        member_suffix,
    };
    validate_limits(&limits)?;
    Ok(limits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// Measured resource usage from a completed JSONL archive scan.
pub struct JsonlScanStats {
    /// Total decompressed bytes consumed, including skipped tar material.
    pub decoded_bytes: u64,
    /// Archive member count, including skipped tar members.
    pub members: u64,
    /// Nonempty records delivered to the visitor.
    pub records: u64,
    /// Largest record buffer observed, including a possible line ending.
    pub peak_record_buffer_bytes: usize,
}

/// Visit nonempty JSONL records in container encounter order under `limits`.
pub fn visit_jsonl_records(
    container: RawPayloadContainer,
    bytes: &[u8],
    limits: &JsonlStreamLimits,
    mut visit: impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<JsonlScanStats> {
    validate_limits(limits)?;
    match container {
        RawPayloadContainer::JsonlText => {
            visit_single_reader(Cursor::new(bytes), limits, &mut visit)
        }
        RawPayloadContainer::JsonlGzip => visit_single_reader(
            flate2::read::MultiGzDecoder::new(Cursor::new(bytes)),
            limits,
            &mut visit,
        ),
        RawPayloadContainer::SingleJsonlZip => {
            {
                let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
                    .context("inspect single-member JSONL ZIP")?;
                ensure!(
                    archive.len() == 1,
                    "single_jsonl_zip requires exactly one member, found {}",
                    archive.len()
                );
                let member = archive.by_index(0).context("inspect JSONL ZIP member")?;
                ensure!(!member.is_dir(), "single_jsonl_zip member is a directory");
            }
            let mut member = crate::zip_reader::zip_member_reader(bytes)
                .context("open single-member JSONL ZIP")?;
            ensure!(
                member.declared_len() as u64 <= limits.max_member_bytes,
                "ZIP JSONL member declares {} bytes, exceeding max_member_bytes {}",
                member.declared_len(),
                limits.max_member_bytes
            );
            let stats = visit_single_reader(&mut member, limits, &mut visit)?;
            member.verify().context("verify JSONL ZIP member")?;
            Ok(stats)
        }
        RawPayloadContainer::TarGzipJsonl => {
            let member_suffix = limits
                .member_suffix
                .as_deref()
                .context("tar_gzip_jsonl requires member_suffix")?;
            ensure!(!member_suffix.is_empty(), "member_suffix must not be empty");
            let mut stats = JsonlScanStats::default();
            let archive = crate::tar_reader::visit_gzip_tar_members(
                Cursor::new(bytes),
                member_suffix,
                limits.max_decoded_bytes,
                limits.max_members,
                limits.max_member_bytes,
                |_, _, reader| visit_reader(reader, limits, &mut stats, &mut visit),
            )?;
            stats.decoded_bytes = archive.decoded_bytes;
            stats.members = archive.members;
            Ok(stats)
        }
        other => bail!("JSONL record streaming does not support container {other:?} yet"),
    }
}

fn visit_single_reader<R: Read>(
    reader: R,
    limits: &JsonlStreamLimits,
    visit: &mut impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<JsonlScanStats> {
    let mut stats = JsonlScanStats {
        members: 1,
        ..JsonlScanStats::default()
    };
    visit_reader(reader, limits, &mut stats, visit)?;
    Ok(stats)
}

fn validate_limits(limits: &JsonlStreamLimits) -> Result<()> {
    ensure!(
        limits.max_decoded_bytes > 0,
        "max_decoded_bytes must be positive"
    );
    ensure!(limits.max_members > 0, "max_members must be positive");
    ensure!(
        limits.max_member_bytes > 0,
        "max_member_bytes must be positive"
    );
    ensure!(
        limits.max_record_bytes > 0,
        "max_record_bytes must be positive"
    );
    ensure!(limits.max_records > 0, "max_records must be positive");
    Ok(())
}

fn visit_reader<R: Read>(
    reader: R,
    limits: &JsonlStreamLimits,
    stats: &mut JsonlScanStats,
    visit: &mut impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<()> {
    let mut reader = BufReader::new(reader);
    let read_cap = limits
        .max_record_bytes
        .checked_add(2)
        .context("max_record_bytes is too large")?;
    let mut buffer = Vec::new();
    let mut reader_bytes = 0u64;
    loop {
        buffer.clear();
        let read = {
            let mut limited = (&mut reader).take(read_cap as u64);
            limited
                .read_until(b'\n', &mut buffer)
                .context("read JSONL record")?
        };
        if read == 0 {
            return Ok(());
        }
        stats.peak_record_buffer_bytes = stats.peak_record_buffer_bytes.max(buffer.len());
        stats.decoded_bytes = stats
            .decoded_bytes
            .checked_add(read as u64)
            .context("decoded JSONL byte count overflow")?;
        reader_bytes = reader_bytes
            .checked_add(read as u64)
            .context("JSONL member byte count overflow")?;
        ensure!(
            reader_bytes <= limits.max_member_bytes,
            "decoded JSONL member bytes {reader_bytes} exceed max_member_bytes {}",
            limits.max_member_bytes
        );
        ensure!(
            stats.decoded_bytes <= limits.max_decoded_bytes,
            "decoded JSONL bytes {} exceed max_decoded_bytes {}",
            stats.decoded_bytes,
            limits.max_decoded_bytes
        );

        let terminated = buffer.last() == Some(&b'\n');
        ensure!(
            terminated || reader.fill_buf()?.is_empty(),
            "JSONL record exceeds max_record_bytes {}",
            limits.max_record_bytes
        );
        if terminated {
            buffer.pop();
        }
        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
        ensure!(
            buffer.len() <= limits.max_record_bytes,
            "JSONL record exceeds max_record_bytes {}",
            limits.max_record_bytes
        );
        std::str::from_utf8(&buffer).context("JSONL record is not valid UTF-8")?;
        if buffer.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        ensure!(
            stats.records < limits.max_records,
            "JSONL record count exceeds max_records {}",
            limits.max_records
        );
        let ordinal = stats.records;
        visit(ordinal, &buffer)?;
        stats.records += 1;
    }
}
