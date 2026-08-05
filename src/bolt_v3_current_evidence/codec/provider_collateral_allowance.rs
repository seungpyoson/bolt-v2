//! Provider allowance capture-failure wire codec.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        ProviderCollateralAllowanceCaptureEndpoint, ProviderCollateralAllowanceCaptureErrorClass,
        ProviderCollateralAllowanceCaptureFailureFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_capture_failure(
    fact: ProviderCollateralAllowanceCaptureFailureFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_capture_failure(&fact)?;
    let purpose = KnownPurpose::ProviderCollateralAllowanceCaptureFailure;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &CaptureFailureLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            capture_failure: CaptureFailureV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_capture_failure(
    line: &str,
    line_number: usize,
) -> Result<ProviderCollateralAllowanceCaptureFailureFact> {
    let decoded: CaptureFailureLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ProviderCollateralAllowanceCaptureFailureV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.capture_failure.into_fact();
    validate_capture_failure(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_capture_failure(
    fact: &ProviderCollateralAllowanceCaptureFailureFact,
) -> Result<(), RecordFailure> {
    if fact.source.trim().is_empty() || fact.observed_at_ns == 0 || fact.captures_missed == 0 {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "provider-allowance capture failure contains an empty or invalid field"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureFailureLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    capture_failure: CaptureFailureV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureFailureV1 {
    source: String,
    observed_at_ns: u64,
    endpoint: CaptureEndpointV1,
    error_class: CaptureErrorClassV1,
    captures_missed: u64,
}

impl CaptureFailureV1 {
    fn from_fact(fact: ProviderCollateralAllowanceCaptureFailureFact) -> Self {
        Self {
            source: fact.source,
            observed_at_ns: fact.observed_at_ns,
            endpoint: CaptureEndpointV1::from_fact(fact.endpoint),
            error_class: CaptureErrorClassV1::from_fact(fact.error_class),
            captures_missed: fact.captures_missed,
        }
    }

    fn into_fact(self) -> ProviderCollateralAllowanceCaptureFailureFact {
        ProviderCollateralAllowanceCaptureFailureFact {
            source: self.source,
            observed_at_ns: self.observed_at_ns,
            endpoint: self.endpoint.into_fact(),
            error_class: self.error_class.into_fact(),
            captures_missed: self.captures_missed,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CaptureEndpointV1 {
    ProviderCollateralAllowanceSnapshot,
    ClobBalanceAllowance,
}

impl CaptureEndpointV1 {
    fn from_fact(value: ProviderCollateralAllowanceCaptureEndpoint) -> Self {
        match value {
            ProviderCollateralAllowanceCaptureEndpoint::ProviderCollateralAllowanceSnapshot => {
                Self::ProviderCollateralAllowanceSnapshot
            }
            ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance => {
                Self::ClobBalanceAllowance
            }
        }
    }

    fn into_fact(self) -> ProviderCollateralAllowanceCaptureEndpoint {
        match self {
            Self::ProviderCollateralAllowanceSnapshot => {
                ProviderCollateralAllowanceCaptureEndpoint::ProviderCollateralAllowanceSnapshot
            }
            Self::ClobBalanceAllowance => {
                ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CaptureErrorClassV1 {
    Unknown,
    TransportOrDecode,
}

impl CaptureErrorClassV1 {
    fn from_fact(value: ProviderCollateralAllowanceCaptureErrorClass) -> Self {
        match value {
            ProviderCollateralAllowanceCaptureErrorClass::Unknown => Self::Unknown,
            ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode => {
                Self::TransportOrDecode
            }
        }
    }

    fn into_fact(self) -> ProviderCollateralAllowanceCaptureErrorClass {
        match self {
            Self::Unknown => ProviderCollateralAllowanceCaptureErrorClass::Unknown,
            Self::TransportOrDecode => {
                ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode
            }
        }
    }
}
