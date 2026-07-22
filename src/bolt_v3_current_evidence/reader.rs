use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;

use super::{
    codec::decode_startup_recovery_fact,
    facts::StartupRecoveryFacts,
    generated_contract::{
        KnownSink, descriptor_for_identity, purpose_for_identity, resolve_identity,
        sink_for_purpose,
    },
};

#[derive(Deserialize)]
struct HeaderOnlyLine {
    kind: String,
    schema_version: u32,
    gate_id: String,
    gate_version: String,
    recorded_at_utc_ns: i64,
}

pub(super) fn validate_machine_stream(
    machine: &mut File,
    max_bytes: Option<u64>,
) -> Result<StartupRecoveryFacts> {
    let len = machine.metadata()?.len();
    if let Some(max_bytes) = max_bytes {
        ensure!(
            len <= max_bytes,
            "machine evidence stream exceeds configured byte cap: {len} > {max_bytes}"
        );
    }
    machine.seek(SeekFrom::Start(0))?;
    let mut bytes = String::new();
    machine.read_to_string(&mut bytes)?;
    let mut facts = StartupRecoveryFacts::default();
    for (index, line) in bytes.lines().enumerate() {
        ensure!(
            !line.trim().is_empty(),
            "blank machine evidence line {}",
            index + 1
        );
        let header: HeaderOnlyLine = serde_json::from_str(line)
            .with_context(|| format!("malformed machine evidence line {}", index + 1))?;
        ensure!(
            header.recorded_at_utc_ns > 0,
            "recorded_at_utc_ns must be positive at machine evidence line {}",
            index + 1
        );
        ensure!(
            !header.gate_version.trim().is_empty(),
            "gate_version must be non-empty at machine evidence line {}",
            index + 1
        );
        let identity = resolve_identity(&header.kind, header.schema_version).ok_or_else(|| {
            anyhow!(
                "unsupported exact identity at machine evidence line {}: ({}, {})",
                index + 1,
                header.kind,
                header.schema_version
            )
        })?;
        let descriptor = descriptor_for_identity(identity);
        ensure!(
            header.gate_id == descriptor.gate_id,
            "wrong gate_id at machine evidence line {}",
            index + 1
        );
        let purpose = purpose_for_identity(identity);
        ensure!(
            sink_for_purpose(purpose) == KnownSink::Machine,
            "observation identity in machine stream at line {}",
            index + 1
        );
        if let Some(fact) = decode_startup_recovery_fact(identity, line, index + 1)? {
            facts.apply(fact)?;
        }
    }
    facts.validate()?;
    Ok(facts)
}
