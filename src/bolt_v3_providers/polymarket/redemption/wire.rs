use std::fmt;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::config::ValidatedRedemptionProfile;

#[derive(Clone, PartialEq, Eq)]
pub struct BoundedWireResponse(Vec<u8>);

impl fmt::Debug for BoundedWireResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedWireResponse")
            .field("byte_len", &self.0.len())
            .field("sha256", &hex::encode(Sha256::digest(&self.0)))
            .finish()
    }
}

impl BoundedWireResponse {
    pub fn from_relayer(
        profile: &ValidatedRedemptionProfile,
        bytes: Vec<u8>,
    ) -> Result<Self, WireParseError> {
        Self::with_limit(bytes, profile.config.relayer.max_response_bytes)
    }

    pub fn from_rpc(
        profile: &ValidatedRedemptionProfile,
        bytes: Vec<u8>,
    ) -> Result<Self, WireParseError> {
        Self::with_limit(bytes, profile.config.rpc.max_response_bytes)
    }

    fn with_limit(bytes: Vec<u8>, max_bytes: usize) -> Result<Self, WireParseError> {
        if bytes.len() > max_bytes {
            return Err(WireParseError::diagnostic(
                WireFailureClass::Oversize,
                None,
                &bytes,
            ));
        }
        Ok(Self(bytes))
    }

    pub fn parse_submit(
        &self,
        profile: &ValidatedRedemptionProfile,
    ) -> Result<SubmitResponse, WireParseError> {
        let parsed: SubmitWire = serde_json::from_slice(&self.0)
            .map_err(|_| WireParseError::diagnostic(WireFailureClass::Malformed, None, &self.0))?;
        validate_id(&parsed.transaction_id, profile)?;
        Ok(SubmitResponse {
            transaction_id: parsed.transaction_id,
            state: parse_state(&parsed.state, &self.0)?,
            transaction_hash: parse_optional_hash(&parsed.transaction_hash, &self.0)?,
        })
    }

    pub fn parse_exact_transaction(
        &self,
        profile: &ValidatedRedemptionProfile,
        expected_id: &str,
    ) -> Result<RelayerTransaction, WireParseError> {
        validate_id(expected_id, profile)?;
        let parsed: Vec<TransactionWire> = serde_json::from_slice(&self.0)
            .map_err(|_| WireParseError::diagnostic(WireFailureClass::Malformed, None, &self.0))?;
        if parsed.len() != profile.config.relayer.max_transaction_items {
            return Err(WireParseError::diagnostic(
                WireFailureClass::WrongItemCount,
                None,
                &self.0,
            ));
        }
        let transaction = parsed.into_iter().next().ok_or_else(|| {
            WireParseError::diagnostic(WireFailureClass::WrongItemCount, None, &self.0)
        })?;
        if transaction.transaction_id != expected_id {
            return Err(WireParseError::diagnostic(
                WireFailureClass::IdentityMismatch,
                None,
                &self.0,
            ));
        }
        for value in [
            &transaction.from,
            &transaction.to,
            &transaction.proxy_address,
            &transaction.data,
            &transaction.nonce,
            &transaction.value,
            &transaction.transaction_type,
            &transaction.created_at,
            &transaction.updated_at,
        ] {
            if value.len() > profile.config.relayer.max_response_bytes {
                return Err(WireParseError::diagnostic(
                    WireFailureClass::FieldTooLarge,
                    None,
                    &self.0,
                ));
            }
        }
        if transaction.metadata.len() > profile.config.relayer.max_metadata_bytes {
            return Err(WireParseError::diagnostic(
                WireFailureClass::FieldTooLarge,
                None,
                &self.0,
            ));
        }
        Ok(RelayerTransaction {
            transaction_id: transaction.transaction_id,
            transaction_hash: parse_optional_hash(&transaction.transaction_hash, &self.0)?,
            state: parse_state(&transaction.state, &self.0)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayerState {
    New,
    Executed,
    Mined,
    Invalid,
    Confirmed,
    Failed,
}

impl RelayerState {
    pub fn is_terminal_proof(self) -> bool {
        let _ = self;
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitResponse {
    pub transaction_id: String,
    pub state: RelayerState,
    pub transaction_hash: Option<[u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayerTransaction {
    pub transaction_id: String,
    pub state: RelayerState,
    pub transaction_hash: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFailureClass {
    Transport,
    Http,
    Oversize,
    Malformed,
    WrongItemCount,
    IdentityMismatch,
    FieldTooLarge,
    UnknownState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireDiagnostic {
    pub class: WireFailureClass,
    pub http_status: Option<u16>,
    pub body_len: usize,
    pub body_sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireParseError {
    pub diagnostic: WireDiagnostic,
}

impl WireParseError {
    pub fn diagnostic(class: WireFailureClass, http_status: Option<u16>, bytes: &[u8]) -> Self {
        Self {
            diagnostic: WireDiagnostic {
                class,
                http_status,
                body_len: bytes.len(),
                body_sha256: Sha256::digest(bytes).into(),
            },
        }
    }
}

impl fmt::Display for WireParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "redacted relayer wire failure: {:?}",
            self.diagnostic
        )
    }
}

impl std::error::Error for WireParseError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitWire {
    #[serde(rename = "transactionID")]
    transaction_id: String,
    state: String,
    #[serde(rename = "transactionHash", default)]
    transaction_hash: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionWire {
    #[serde(rename = "transactionID")]
    transaction_id: String,
    #[serde(rename = "transactionHash")]
    transaction_hash: String,
    from: String,
    to: String,
    #[serde(rename = "proxyAddress")]
    proxy_address: String,
    data: String,
    nonce: String,
    value: String,
    state: String,
    #[serde(rename = "type")]
    transaction_type: String,
    metadata: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

fn validate_id(value: &str, profile: &ValidatedRedemptionProfile) -> Result<(), WireParseError> {
    if value.is_empty() || value.len() > profile.config.relayer.max_transaction_id_bytes {
        return Err(WireParseError::diagnostic(
            WireFailureClass::FieldTooLarge,
            None,
            value.as_bytes(),
        ));
    }
    Ok(())
}

fn parse_state(value: &str, bytes: &[u8]) -> Result<RelayerState, WireParseError> {
    match value {
        "STATE_NEW" => Ok(RelayerState::New),
        "STATE_EXECUTED" => Ok(RelayerState::Executed),
        "STATE_MINED" => Ok(RelayerState::Mined),
        "STATE_INVALID" => Ok(RelayerState::Invalid),
        "STATE_CONFIRMED" => Ok(RelayerState::Confirmed),
        "STATE_FAILED" => Ok(RelayerState::Failed),
        _ => Err(WireParseError::diagnostic(
            WireFailureClass::UnknownState,
            None,
            bytes,
        )),
    }
}

fn parse_optional_hash(value: &str, bytes: &[u8]) -> Result<Option<[u8; 32]>, WireParseError> {
    if value.is_empty() {
        return Ok(None);
    }
    let encoded = value.strip_prefix("0x").unwrap_or(value);
    let decoded = hex::decode(encoded)
        .map_err(|_| WireParseError::diagnostic(WireFailureClass::Malformed, None, bytes))?;
    decoded
        .try_into()
        .map(Some)
        .map_err(|_| WireParseError::diagnostic(WireFailureClass::Malformed, None, bytes))
}
