use std::{fmt, ops::Range};

use alloy_primitives::{Address, B256, Keccak256, U256, keccak256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    bolt_v3_risk_closure_workspace::{RiskClosureWorkspaceError, RiskClosureWorkspaceLease},
    bolt_v3_secrets::ResolvedEvmSigningKey,
};

pub struct RedemptionPreparationConfig {
    schema_version: u32,
    production_activation_enabled: bool,
    chain_id: u64,
    wallet_type: &'static str,
    safe_address: Address,
    collateral_asset: Address,
    standard_adapter_target: Address,
    negative_risk_adapter_target: Address,
    parent_collection_id: B256,
    dummy_index_sets: [U256; 2],
    maximum_safe_nonce_decimal_digits: usize,
}

struct RedemptionProtocolFacts {
    function_selector: [u8; 4],
    operation: u8,
    value: U256,
    safe_tx_gas: U256,
    base_gas: U256,
    gas_price: U256,
    gas_token: Address,
    refund_receiver: Address,
    metadata: &'static str,
}

mod generated;
use generated::{POLYMARKET_REDEMPTION_PREPARATION_CONFIG, POLYMARKET_REDEMPTION_PROTOCOL};

pub fn polymarket_redemption_preparation_config() -> &'static RedemptionPreparationConfig {
    &POLYMARKET_REDEMPTION_PREPARATION_CONFIG
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionMarketKind {
    Standard,
    NegativeRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptKind {
    Original,
    Fence,
}

pub struct RedemptionPreparationPermit {
    private: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedemptionRequestInput {
    pub market_kind: RedemptionMarketKind,
    pub condition_id: B256,
    pub safe_nonce: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedRequestIdentity(B256);

pub struct PreparedRequest<'request> {
    bytes: &'request [u8],
    calldata_hex: &'request [u8],
    identity: PreparedRequestIdentity,
    safe_nonce: U256,
    target: Address,
    value: U256,
}

impl<'request> PreparedRequest<'request> {
    pub fn as_bytes(&self) -> &'request [u8] {
        self.bytes
    }

    pub fn calldata_hex(&self) -> &'request [u8] {
        self.calldata_hex
    }

    pub fn identity(&self) -> PreparedRequestIdentity {
        self.identity
    }

    pub fn safe_nonce(&self) -> U256 {
        self.safe_nonce
    }

    pub fn target(&self) -> Address {
        self.target
    }

    pub fn value(&self) -> U256 {
        self.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedemptionPreparationError {
    ProductionActivationForbidden,
    InvalidConfiguration { field: &'static str },
    InvalidRequestInput { field: &'static str },
    InvalidSigningKey,
    SigningFailed,
    WorkspaceTooSmall { required: usize, available: usize },
    Workspace(RiskClosureWorkspaceError),
    EncodingOverflow,
    EncodingInvariant,
}

impl fmt::Display for RedemptionPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProductionActivationForbidden => {
                formatter.write_str("redemption request preparation is disabled")
            }
            Self::InvalidConfiguration { field } => {
                write!(formatter, "invalid redemption configuration field: {field}")
            }
            Self::InvalidRequestInput { field } => {
                write!(formatter, "invalid redemption request input: {field}")
            }
            Self::InvalidSigningKey => formatter.write_str("invalid redemption signing key"),
            Self::SigningFailed => formatter.write_str("redemption request signing failed"),
            Self::WorkspaceTooSmall {
                required,
                available,
            } => write!(
                formatter,
                "risk-closure workspace is too small: required {required} bytes, available {available} bytes"
            ),
            Self::Workspace(error) => write!(formatter, "risk-closure workspace failed: {error:?}"),
            Self::EncodingOverflow => formatter.write_str("redemption request length overflowed"),
            Self::EncodingInvariant => {
                formatter.write_str("redemption request encoding invariant failed")
            }
        }
    }
}

impl std::error::Error for RedemptionPreparationError {}

impl From<RiskClosureWorkspaceError> for RedemptionPreparationError {
    fn from(error: RiskClosureWorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

pub fn prepare_redemption_request(
    permit: RedemptionPreparationPermit,
    lease: &mut RiskClosureWorkspaceLease,
    config: &RedemptionPreparationConfig,
    signing_key: &ResolvedEvmSigningKey,
    input: RedemptionRequestInput,
    attempt: AttemptKind,
    use_prepared: impl for<'request> FnOnce(PreparedRequest<'request>),
) -> Result<(), RedemptionPreparationError> {
    let RedemptionPreparationPermit { private: () } = permit;
    validate_config(config)?;
    validate_nonce(input.safe_nonce, config.maximum_safe_nonce_decimal_digits)?;
    let signer_private_key = Zeroizing::new(*signing_key.as_bytes());
    let signer = PrivateKeySigner::from_slice(signer_private_key.as_ref())
        .map_err(|_| RedemptionPreparationError::InvalidSigningKey)?;
    let target = match attempt {
        AttemptKind::Original => match input.market_kind {
            RedemptionMarketKind::Standard => config.standard_adapter_target,
            RedemptionMarketKind::NegativeRisk => config.negative_risk_adapter_target,
        },
        AttemptKind::Fence => config.safe_address,
    };

    lease
        .with_workspace_mut(|workspace| {
            prepare_in_workspace(
                workspace,
                config,
                &signer,
                input,
                attempt,
                target,
                use_prepared,
            )
        })
        .map_err(RedemptionPreparationError::Workspace)?
}

fn prepare_in_workspace(
    workspace: &mut [u8],
    config: &RedemptionPreparationConfig,
    signer: &PrivateKeySigner,
    input: RedemptionRequestInput,
    attempt: AttemptKind,
    target: Address,
    use_prepared: impl for<'request> FnOnce(PreparedRequest<'request>),
) -> Result<(), RedemptionPreparationError> {
    let available = workspace.len();
    let mut workspace = WorkspaceGuard::new(workspace);
    let mut counter = LengthCounter::default();
    let placeholder_signature = [u8::default(); 65];
    let _ = write_request(
        &mut counter,
        config,
        signer.address(),
        input,
        attempt,
        target,
        &placeholder_signature,
    )?;
    let required = counter.len();
    if required > available {
        return Err(RedemptionPreparationError::WorkspaceTooSmall {
            required,
            available,
        });
    }

    let calldata_hash = match attempt {
        AttemptKind::Original => {
            let encoded = encode_redemption_calldata(workspace.as_mut(), config, input)?;
            keccak256(&workspace.as_ref()[..encoded])
        }
        AttemptKind::Fence => keccak256([]),
    };
    let safe_digest = safe_typed_digest(config, target, input.safe_nonce, calldata_hash);
    let signature = signer
        .sign_message_sync(safe_digest.as_slice())
        .map_err(|_| RedemptionPreparationError::SigningFailed)?;
    let mut packed_signature = signature.as_bytes();
    packed_signature[64] = packed_signature[64]
        .checked_add(4)
        .ok_or(RedemptionPreparationError::SigningFailed)?;

    workspace.as_mut().zeroize();
    let mut writer = SliceWriter::new(workspace.as_mut());
    let calldata_range = write_request(
        &mut writer,
        config,
        signer.address(),
        input,
        attempt,
        target,
        &packed_signature,
    )?;
    packed_signature.zeroize();
    if writer.len() != required {
        return Err(RedemptionPreparationError::EncodingInvariant);
    }
    let written = writer.len();
    let identity = request_identity(
        config.chain_id,
        config.wallet_type,
        config.safe_address,
        input.safe_nonce,
        target,
        calldata_hash,
    );
    let request_bytes = &workspace.as_ref()[..written];
    let calldata_hex = &request_bytes[calldata_range];
    use_prepared(PreparedRequest {
        bytes: request_bytes,
        calldata_hex,
        identity,
        safe_nonce: input.safe_nonce,
        target,
        value: POLYMARKET_REDEMPTION_PROTOCOL.value,
    });
    Ok(())
}

fn validate_config(config: &RedemptionPreparationConfig) -> Result<(), RedemptionPreparationError> {
    if config.production_activation_enabled {
        return Err(RedemptionPreparationError::ProductionActivationForbidden);
    }
    for (valid, field) in [
        (config.schema_version == 1, "schema_version"),
        (config.chain_id != 0, "chain_id"),
        (config.wallet_type == "SAFE", "wallet_type"),
        (config.safe_address != Address::ZERO, "safe_address"),
        (config.collateral_asset != Address::ZERO, "collateral_asset"),
        (
            config.standard_adapter_target != Address::ZERO,
            "standard_adapter_target",
        ),
        (
            config.negative_risk_adapter_target != Address::ZERO,
            "negative_risk_adapter_target",
        ),
        (
            config.maximum_safe_nonce_decimal_digits > 0
                && config.maximum_safe_nonce_decimal_digits <= 78,
            "maximum_safe_nonce_decimal_digits",
        ),
        (
            POLYMARKET_REDEMPTION_PROTOCOL.metadata.is_empty(),
            "metadata",
        ),
    ] {
        if !valid {
            return Err(RedemptionPreparationError::InvalidConfiguration { field });
        }
    }
    Ok(())
}

fn validate_nonce(nonce: U256, maximum_digits: usize) -> Result<(), RedemptionPreparationError> {
    let mut buffer = [u8::default(); 78];
    let digits = decimal_bytes(nonce, &mut buffer);
    if digits.len() > maximum_digits {
        return Err(RedemptionPreparationError::InvalidRequestInput {
            field: "safe_nonce",
        });
    }
    buffer.zeroize();
    Ok(())
}

struct WorkspaceGuard<'workspace> {
    bytes: &'workspace mut [u8],
}

impl<'workspace> WorkspaceGuard<'workspace> {
    fn new(bytes: &'workspace mut [u8]) -> Self {
        bytes.zeroize();
        Self { bytes }
    }

    fn as_ref(&self) -> &[u8] {
        self.bytes
    }

    fn as_mut(&mut self) -> &mut [u8] {
        self.bytes
    }
}

impl Drop for WorkspaceGuard<'_> {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

trait ByteSink {
    fn push(&mut self, byte: u8) -> Result<(), RedemptionPreparationError>;
    fn len(&self) -> usize;

    fn write(&mut self, bytes: &[u8]) -> Result<(), RedemptionPreparationError> {
        for byte in bytes {
            self.push(*byte)?;
        }
        Ok(())
    }

    fn write_hex(&mut self, bytes: &[u8]) -> Result<(), RedemptionPreparationError> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            self.push(HEX[usize::from(byte >> 4)])?;
            self.push(HEX[usize::from(byte & 0x0f)])?;
        }
        Ok(())
    }

    fn write_address(&mut self, address: Address) -> Result<(), RedemptionPreparationError> {
        self.write_hex(address.as_slice())
    }

    fn write_u256(&mut self, value: U256) -> Result<(), RedemptionPreparationError> {
        let mut buffer = [u8::default(); 78];
        let digits = decimal_bytes(value, &mut buffer);
        self.write(digits)?;
        buffer.zeroize();
        Ok(())
    }
}

#[derive(Default)]
struct LengthCounter {
    written: usize,
}

impl ByteSink for LengthCounter {
    fn push(&mut self, _: u8) -> Result<(), RedemptionPreparationError> {
        self.written = self
            .written
            .checked_add(1)
            .ok_or(RedemptionPreparationError::EncodingOverflow)?;
        Ok(())
    }

    fn len(&self) -> usize {
        self.written
    }
}

struct SliceWriter<'buffer> {
    buffer: &'buffer mut [u8],
    written: usize,
}

impl<'buffer> SliceWriter<'buffer> {
    fn new(buffer: &'buffer mut [u8]) -> Self {
        Self { buffer, written: 0 }
    }
}

impl ByteSink for SliceWriter<'_> {
    fn push(&mut self, byte: u8) -> Result<(), RedemptionPreparationError> {
        let slot = self
            .buffer
            .get_mut(self.written)
            .ok_or(RedemptionPreparationError::EncodingInvariant)?;
        *slot = byte;
        self.written = self
            .written
            .checked_add(1)
            .ok_or(RedemptionPreparationError::EncodingOverflow)?;
        Ok(())
    }

    fn len(&self) -> usize {
        self.written
    }
}

fn decimal_bytes(value: U256, buffer: &mut [u8; 78]) -> &[u8] {
    let mut remaining = value;
    let mut start = buffer.len();
    if remaining == U256::ZERO {
        start -= 1;
        buffer[start] = b'0';
    } else {
        let radix = U256::from(10_u8);
        while remaining != U256::ZERO {
            let (quotient, remainder) = remaining.div_rem(radix);
            start -= 1;
            buffer[start] = b'0' + remainder.to::<u8>();
            remaining = quotient;
        }
    }
    &buffer[start..]
}

fn write_request(
    sink: &mut impl ByteSink,
    config: &RedemptionPreparationConfig,
    signer_address: Address,
    input: RedemptionRequestInput,
    attempt: AttemptKind,
    target: Address,
    signature: &[u8; 65],
) -> Result<Range<usize>, RedemptionPreparationError> {
    let protocol = &POLYMARKET_REDEMPTION_PROTOCOL;
    sink.write(br#"{"from":"0x"#)?;
    sink.write_address(signer_address)?;
    sink.write(br#"","to":"0x"#)?;
    sink.write_address(target)?;
    sink.write(br#"","proxyWallet":"0x"#)?;
    sink.write_address(config.safe_address)?;
    sink.write(br#"","data":""#)?;
    let calldata_start = sink.len();
    sink.write(b"0x")?;
    if attempt == AttemptKind::Original {
        write_redemption_calldata_hex(sink, config, input)?;
    }
    let calldata_end = sink.len();
    sink.write(br#"","nonce":""#)?;
    sink.write_u256(input.safe_nonce)?;
    sink.write(br#"","signature":"0x"#)?;
    sink.write_hex(signature)?;
    sink.write(br#"","signatureParams":{"gasPrice":""#)?;
    sink.write_u256(protocol.gas_price)?;
    sink.write(br#"","operation":""#)?;
    sink.write_u256(U256::from(protocol.operation))?;
    sink.write(br#"","safeTxnGas":""#)?;
    sink.write_u256(protocol.safe_tx_gas)?;
    sink.write(br#"","baseGas":""#)?;
    sink.write_u256(protocol.base_gas)?;
    sink.write(br#"","gasToken":"0x"#)?;
    sink.write_address(protocol.gas_token)?;
    sink.write(br#"","refundReceiver":"0x"#)?;
    sink.write_address(protocol.refund_receiver)?;
    sink.write(br#""},"type":""#)?;
    sink.write(config.wallet_type.as_bytes())?;
    sink.write(br#"","metadata":""#)?;
    sink.write(protocol.metadata.as_bytes())?;
    sink.write(br#""}"#)?;
    Ok(calldata_start..calldata_end)
}

fn encode_redemption_calldata(
    workspace: &mut [u8],
    config: &RedemptionPreparationConfig,
    input: RedemptionRequestInput,
) -> Result<usize, RedemptionPreparationError> {
    let mut writer = SliceWriter::new(workspace);
    write_redemption_calldata_binary(&mut writer, config, input)?;
    Ok(writer.len())
}

fn write_redemption_calldata_binary(
    sink: &mut impl ByteSink,
    config: &RedemptionPreparationConfig,
    input: RedemptionRequestInput,
) -> Result<(), RedemptionPreparationError> {
    sink.write(&POLYMARKET_REDEMPTION_PROTOCOL.function_selector)?;
    write_address_word(sink, config.collateral_asset)?;
    sink.write(config.parent_collection_id.as_slice())?;
    sink.write(input.condition_id.as_slice())?;
    write_u256_word(sink, U256::from(128_u16))?;
    write_u256_word(sink, U256::from(config.dummy_index_sets.len()))?;
    for index_set in config.dummy_index_sets {
        write_u256_word(sink, index_set)?;
    }
    Ok(())
}

fn write_redemption_calldata_hex(
    sink: &mut impl ByteSink,
    config: &RedemptionPreparationConfig,
    input: RedemptionRequestInput,
) -> Result<(), RedemptionPreparationError> {
    sink.write_hex(&POLYMARKET_REDEMPTION_PROTOCOL.function_selector)?;
    write_address_word_hex(sink, config.collateral_asset)?;
    sink.write_hex(config.parent_collection_id.as_slice())?;
    sink.write_hex(input.condition_id.as_slice())?;
    write_u256_word_hex(sink, U256::from(128_u16))?;
    write_u256_word_hex(sink, U256::from(config.dummy_index_sets.len()))?;
    for index_set in config.dummy_index_sets {
        write_u256_word_hex(sink, index_set)?;
    }
    Ok(())
}

fn write_address_word(
    sink: &mut impl ByteSink,
    address: Address,
) -> Result<(), RedemptionPreparationError> {
    let mut word = [u8::default(); 32];
    word[12..].copy_from_slice(address.as_slice());
    sink.write(&word)
}

fn write_address_word_hex(
    sink: &mut impl ByteSink,
    address: Address,
) -> Result<(), RedemptionPreparationError> {
    let mut word = [u8::default(); 32];
    word[12..].copy_from_slice(address.as_slice());
    sink.write_hex(&word)
}

fn write_u256_word(
    sink: &mut impl ByteSink,
    value: U256,
) -> Result<(), RedemptionPreparationError> {
    sink.write(&value.to_be_bytes::<32>())
}

fn write_u256_word_hex(
    sink: &mut impl ByteSink,
    value: U256,
) -> Result<(), RedemptionPreparationError> {
    sink.write_hex(&value.to_be_bytes::<32>())
}

fn safe_typed_digest(
    config: &RedemptionPreparationConfig,
    target: Address,
    nonce: U256,
    calldata_hash: B256,
) -> B256 {
    let domain_type_hash = keccak256(b"EIP712Domain(uint256 chainId,address verifyingContract)");
    let safe_tx_type_hash = keccak256(
        b"SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)",
    );
    let mut domain = [u8::default(); 32 * 3];
    put_b256_word(&mut domain, 0, domain_type_hash);
    put_u256_word(&mut domain, 1, U256::from(config.chain_id));
    put_address_word(&mut domain, 2, config.safe_address);
    let domain_separator = keccak256(domain);

    let protocol = &POLYMARKET_REDEMPTION_PROTOCOL;
    let mut safe_tx = [u8::default(); 32 * 11];
    put_b256_word(&mut safe_tx, 0, safe_tx_type_hash);
    put_address_word(&mut safe_tx, 1, target);
    put_u256_word(&mut safe_tx, 2, protocol.value);
    put_b256_word(&mut safe_tx, 3, calldata_hash);
    put_u256_word(&mut safe_tx, 4, U256::from(protocol.operation));
    put_u256_word(&mut safe_tx, 5, protocol.safe_tx_gas);
    put_u256_word(&mut safe_tx, 6, protocol.base_gas);
    put_u256_word(&mut safe_tx, 7, protocol.gas_price);
    put_address_word(&mut safe_tx, 8, protocol.gas_token);
    put_address_word(&mut safe_tx, 9, protocol.refund_receiver);
    put_u256_word(&mut safe_tx, 10, nonce);
    let safe_tx_hash = keccak256(safe_tx);

    let mut typed_data = [u8::default(); 2 + 32 + 32];
    typed_data[..2].copy_from_slice(&[0x19, 0x01]);
    typed_data[2..34].copy_from_slice(domain_separator.as_slice());
    typed_data[34..].copy_from_slice(safe_tx_hash.as_slice());
    keccak256(typed_data)
}

fn put_b256_word(destination: &mut [u8], index: usize, value: B256) {
    let start = index * 32;
    destination[start..start + 32].copy_from_slice(value.as_slice());
}

fn put_u256_word(destination: &mut [u8], index: usize, value: U256) {
    let start = index * 32;
    destination[start..start + 32].copy_from_slice(&value.to_be_bytes::<32>());
}

fn put_address_word(destination: &mut [u8], index: usize, value: Address) {
    let start = index * 32;
    destination[start + 12..start + 32].copy_from_slice(value.as_slice());
}

pub fn request_identity(
    chain_id: u64,
    wallet_type: &str,
    safe_address: Address,
    safe_nonce: U256,
    target: Address,
    calldata_hash: B256,
) -> PreparedRequestIdentity {
    let mut hasher = Keccak256::new();
    hasher.update(b"bolt-v2/polymarket-safe-redemption-request/v1");
    hasher.update(U256::from(chain_id).to_be_bytes::<32>());
    hasher.update(U256::from(wallet_type.len()).to_be_bytes::<32>());
    hasher.update(wallet_type.as_bytes());
    hasher.update(safe_address.as_slice());
    hasher.update(safe_nonce.to_be_bytes::<32>());
    hasher.update(target.as_slice());
    hasher.update(calldata_hash.as_slice());
    PreparedRequestIdentity(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, str};

    use alloy_primitives::{Address, B256, U256, keccak256};
    use zeroize::{ZeroizeOnDrop, Zeroizing};

    use super::*;
    use crate::bolt_v3_risk_closure_workspace::test_recovery_lease as recovery_lease;

    const FIXTURE_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const GOLDEN_CALLDATA: &str = concat!(
        "0x01b7037c",
        "0000000000000000000000004444444444444444444444444444444444444444",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "0000000000000000000000000000000000000000000000000000000000000001",
        "0000000000000000000000000000000000000000000000000000000000000002",
    );
    const GOLDEN_STANDARD_REQUEST: &str = concat!(
        r#"{"from":"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266","to":"0x2222222222222222222222222222222222222222","proxyWallet":"0x1111111111111111111111111111111111111111","data":""#,
        "0x01b7037c",
        "0000000000000000000000004444444444444444444444444444444444444444",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "0000000000000000000000000000000000000000000000000000000000000001",
        "0000000000000000000000000000000000000000000000000000000000000002",
        r#"","nonce":"7","signature":"0x00dacdf8520e79b2c158b14eaeb737524a398e5e612caf96b987986fa6c0a4eb16421a0796fcffc30ccbbde64b5614daeee5cd71f78da1ded226657f7f3a9a6320","signatureParams":{"gasPrice":"0","operation":"0","safeTxnGas":"0","baseGas":"0","gasToken":"0x0000000000000000000000000000000000000000","refundReceiver":"0x0000000000000000000000000000000000000000"},"type":"SAFE","metadata":""}"#,
    );
    const GOLDEN_NEGATIVE_RISK_REQUEST: &str = concat!(
        r#"{"from":"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266","to":"0x3333333333333333333333333333333333333333","proxyWallet":"0x1111111111111111111111111111111111111111","data":""#,
        "0x01b7037c",
        "0000000000000000000000004444444444444444444444444444444444444444",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0000000000000000000000000000000000000000000000000000000000000002",
        "0000000000000000000000000000000000000000000000000000000000000001",
        "0000000000000000000000000000000000000000000000000000000000000002",
        r#"","nonce":"7","signature":"0xeabd59c401862e04f478bbbad4f3922d1f92efeac68aa2210b0219987fd139c177cb6cd00d5af13bc96ba1ebf355be0b73ef6b93476d987c0683eed1227db0ef1f","signatureParams":{"gasPrice":"0","operation":"0","safeTxnGas":"0","baseGas":"0","gasToken":"0x0000000000000000000000000000000000000000","refundReceiver":"0x0000000000000000000000000000000000000000"},"type":"SAFE","metadata":""}"#,
    );
    const GOLDEN_FENCE_REQUEST: &str = r#"{"from":"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266","to":"0x1111111111111111111111111111111111111111","proxyWallet":"0x1111111111111111111111111111111111111111","data":"0x","nonce":"7","signature":"0x504f131bb0edd6fec228387a34093cb4853878e9af2f19a96b3efc856fd2bf2929011d7d2b13aa193e7d5d2d5c64e81bf6bec2329c3c6de996b8cca574a9873e1f","signatureParams":{"gasPrice":"0","operation":"0","safeTxnGas":"0","baseGas":"0","gasToken":"0x0000000000000000000000000000000000000000","refundReceiver":"0x0000000000000000000000000000000000000000"},"type":"SAFE","metadata":""}"#;

    fn address(byte: u8) -> Address {
        Address::from([byte; 20])
    }

    fn test_config() -> RedemptionPreparationConfig {
        RedemptionPreparationConfig {
            schema_version: 1,
            production_activation_enabled: false,
            chain_id: 137,
            wallet_type: "SAFE",
            safe_address: address(0x11),
            collateral_asset: address(0x44),
            standard_adapter_target: address(0x22),
            negative_risk_adapter_target: address(0x33),
            parent_collection_id: B256::ZERO,
            dummy_index_sets: [U256::from(1_u8), U256::from(2_u8)],
            maximum_safe_nonce_decimal_digits: 78,
        }
    }

    fn test_credentials() -> ResolvedEvmSigningKey {
        crate::bolt_v3_providers::polymarket::decode_private_key(FIXTURE_PRIVATE_KEY)
            .expect("fixture private key must decode through the provider boundary")
    }

    fn test_provider_secrets()
    -> crate::bolt_v3_providers::polymarket::ResolvedBoltV3PolymarketSecrets {
        crate::bolt_v3_providers::polymarket::ResolvedBoltV3PolymarketSecrets {
            private_key: Zeroizing::new(FIXTURE_PRIVATE_KEY.to_string()),
            redemption_signing_key: crate::bolt_v3_providers::polymarket::decode_private_key(
                FIXTURE_PRIVATE_KEY,
            )
            .expect("fixture private key must decode through the provider boundary"),
            api_key: Zeroizing::new("fixture-builder-key".to_string()),
            api_secret: Zeroizing::new("fixture-builder-secret".to_string()),
            passphrase: Zeroizing::new("fixture-builder-passphrase".to_string()),
        }
    }

    fn original_input(market_kind: RedemptionMarketKind) -> RedemptionRequestInput {
        RedemptionRequestInput {
            market_kind,
            condition_id: B256::from([0xaa; 32]),
            safe_nonce: U256::from(7_u8),
        }
    }

    fn test_preparation_permit() -> RedemptionPreparationPermit {
        RedemptionPreparationPermit { private: () }
    }

    #[test]
    fn standard_v2_calldata_and_request_match_independent_golden() {
        let config = test_config();
        let credentials = test_credentials();
        let mut lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "golden-standard");

        prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            &credentials,
            original_input(RedemptionMarketKind::Standard),
            AttemptKind::Original,
            |prepared| {
                assert_eq!(prepared.calldata_hex(), GOLDEN_CALLDATA.as_bytes());
                assert_eq!(prepared.as_bytes(), GOLDEN_STANDARD_REQUEST.as_bytes());
                assert_eq!(prepared.value(), U256::ZERO);
            },
        )
        .expect("standard request must fit exactly");
    }

    #[test]
    fn negative_risk_v2_calldata_and_request_match_independent_golden() {
        let config = test_config();
        let credentials = test_credentials();
        let mut lease = recovery_lease(GOLDEN_NEGATIVE_RISK_REQUEST.len(), "golden-negative-risk");

        prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            &credentials,
            original_input(RedemptionMarketKind::NegativeRisk),
            AttemptKind::Original,
            |prepared| {
                assert_eq!(prepared.calldata_hex(), GOLDEN_CALLDATA.as_bytes());
                assert_eq!(prepared.as_bytes(), GOLDEN_NEGATIVE_RISK_REQUEST.as_bytes());
            },
        )
        .expect("negative-risk request must fit exactly");
    }

    #[test]
    fn fence_is_zero_value_empty_calldata_and_reuses_original_nonce() {
        let config = test_config();
        let credentials = test_credentials();
        let input = original_input(RedemptionMarketKind::Standard);
        let mut original_lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "nonce-original");
        let mut fence_lease = recovery_lease(GOLDEN_FENCE_REQUEST.len(), "nonce-fence");

        prepare_redemption_request(
            test_preparation_permit(),
            &mut original_lease,
            &config,
            &credentials,
            input,
            AttemptKind::Original,
            |original| {
                prepare_redemption_request(
                    test_preparation_permit(),
                    &mut fence_lease,
                    &config,
                    &credentials,
                    input,
                    AttemptKind::Fence,
                    |fence| {
                        assert_eq!(fence.as_bytes(), GOLDEN_FENCE_REQUEST.as_bytes());
                        assert_eq!(fence.calldata_hex(), b"0x");
                        assert_eq!(fence.value(), U256::ZERO);
                        assert_eq!(fence.safe_nonce(), original.safe_nonce());
                        assert_eq!(fence.target(), config.safe_address);
                    },
                )
                .expect("fence request must fit exactly");
            },
        )
        .expect("original request must fit exactly");
    }

    #[test]
    fn retries_from_identical_inputs_are_byte_identical() {
        let config = test_config();
        let credentials = test_credentials();
        let input = original_input(RedemptionMarketKind::Standard);
        let mut first_lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "retry-first");
        let mut second_lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "retry-second");

        prepare_redemption_request(
            test_preparation_permit(),
            &mut first_lease,
            &config,
            &credentials,
            input,
            AttemptKind::Original,
            |first| {
                prepare_redemption_request(
                    test_preparation_permit(),
                    &mut second_lease,
                    &config,
                    &credentials,
                    input,
                    AttemptKind::Original,
                    |second| {
                        assert_eq!(first.as_bytes(), second.as_bytes());
                        assert_eq!(first.identity(), second.identity());
                    },
                )
                .expect("second retry must prepare");
            },
        )
        .expect("first retry must prepare");
    }

    #[test]
    fn numeric_zero_one_scaled_dust_and_maximum_vectors_prepare() {
        let vectors = [
            (
                U256::ZERO,
                U256::ZERO,
                "0",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            (
                U256::from(1_u8),
                U256::from(1_u8),
                "1",
                "0000000000000000000000000000000000000000000000000000000000000001",
            ),
            (
                U256::from(1_000_000_u64),
                U256::from(1_000_000_u64),
                "1000000",
                "00000000000000000000000000000000000000000000000000000000000f4240",
            ),
            (
                U256::from(999_999_u64),
                U256::from(999_999_u64),
                "999999",
                "00000000000000000000000000000000000000000000000000000000000f423f",
            ),
            (
                U256::MAX,
                U256::MAX,
                "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            ),
        ];
        let config = test_config();
        let credentials = test_credentials();

        for (index, (condition_value, nonce, expected_nonce, expected_condition_id)) in
            vectors.into_iter().enumerate()
        {
            let mut lease = recovery_lease(
                GOLDEN_STANDARD_REQUEST.len() + 128,
                &format!("numeric-vector-{index}"),
            );
            let input = RedemptionRequestInput {
                market_kind: RedemptionMarketKind::Standard,
                condition_id: B256::from(condition_value.to_be_bytes::<32>()),
                safe_nonce: nonce,
            };
            prepare_redemption_request(
                test_preparation_permit(),
                &mut lease,
                &config,
                &credentials,
                input,
                AttemptKind::Original,
                |prepared| {
                    let request =
                        str::from_utf8(prepared.as_bytes()).expect("request is UTF-8 JSON");
                    assert!(request.contains(&format!(r#""nonce":"{expected_nonce}""#)));
                    let calldata =
                        str::from_utf8(prepared.calldata_hex()).expect("calldata is lowercase hex");
                    let condition_start = b"0x01b7037c".len() + (64 * 2);
                    assert_eq!(
                        &calldata[condition_start..condition_start + 64],
                        expected_condition_id,
                    );
                    assert_eq!(prepared.safe_nonce(), nonce);
                },
            )
            .expect("numeric boundary vector must prepare");
        }
    }

    #[test]
    fn one_byte_short_fails_before_callback_and_clears_workspace() {
        let config = test_config();
        let credentials = test_credentials();
        let mut lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len() - 1, "one-byte-short");
        lease
            .with_workspace_mut(|workspace| workspace.fill(0xa5))
            .expect("test must prefill workspace");
        let called = Cell::new(false);

        let error = prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            &credentials,
            original_input(RedemptionMarketKind::Standard),
            AttemptKind::Original,
            |_| called.set(true),
        )
        .expect_err("one-byte-short workspace must fail");

        assert_eq!(
            error,
            RedemptionPreparationError::WorkspaceTooSmall {
                required: GOLDEN_STANDARD_REQUEST.len(),
                available: GOLDEN_STANDARD_REQUEST.len() - 1,
            }
        );
        assert!(!called.get(), "failure must not expose a partial request");
        lease
            .with_workspace_mut(|workspace| assert!(workspace.iter().all(|byte| *byte == 0)))
            .expect("failed preparation must leave a cleared workspace");
    }

    #[test]
    fn successful_callback_is_followed_by_workspace_zeroization() {
        let config = test_config();
        let credentials = test_credentials();
        let mut lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "success-clears");

        prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            &credentials,
            original_input(RedemptionMarketKind::Standard),
            AttemptKind::Original,
            |prepared| assert_eq!(prepared.as_bytes(), GOLDEN_STANDARD_REQUEST.as_bytes()),
        )
        .expect("request must prepare");

        lease
            .with_workspace_mut(|workspace| assert!(workspace.iter().all(|byte| *byte == 0)))
            .expect("successful preparation must clear bytes after callback");
    }

    #[test]
    fn every_identity_field_changes_identity() {
        let chain_id = 137;
        let wallet_type = "SAFE";
        let safe_address = address(0x11);
        let safe_nonce = U256::from(7_u8);
        let target = address(0x22);
        let calldata_hash = keccak256(b"calldata");
        let baseline = request_identity(
            chain_id,
            wallet_type,
            safe_address,
            safe_nonce,
            target,
            calldata_hash,
        );
        let mutations = [
            request_identity(
                chain_id + 1,
                wallet_type,
                safe_address,
                safe_nonce,
                target,
                calldata_hash,
            ),
            request_identity(
                chain_id,
                "SAFE-CHANGED",
                safe_address,
                safe_nonce,
                target,
                calldata_hash,
            ),
            request_identity(
                chain_id,
                wallet_type,
                address(0x12),
                safe_nonce,
                target,
                calldata_hash,
            ),
            request_identity(
                chain_id,
                wallet_type,
                safe_address,
                safe_nonce + U256::from(1_u8),
                target,
                calldata_hash,
            ),
            request_identity(
                chain_id,
                wallet_type,
                safe_address,
                safe_nonce,
                address(0x23),
                calldata_hash,
            ),
            request_identity(
                chain_id,
                wallet_type,
                safe_address,
                safe_nonce,
                target,
                keccak256(b"changed-calldata"),
            ),
        ];

        assert!(mutations.into_iter().all(|identity| identity != baseline));
    }

    #[test]
    fn activation_true_fails_before_callback() {
        let mut config = test_config();
        config.production_activation_enabled = true;
        let credentials = test_credentials();
        let mut lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "activation-true");
        let called = Cell::new(false);

        let error = prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            &credentials,
            original_input(RedemptionMarketKind::Standard),
            AttemptKind::Original,
            |_| called.set(true),
        )
        .expect_err("activation true must fail closed");

        assert_eq!(
            error,
            RedemptionPreparationError::ProductionActivationForbidden
        );
        assert!(!called.get());
    }

    #[test]
    fn provider_snapshot_is_redacted_and_drives_request_preparation() {
        fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}

        let config = test_config();
        let provider_secrets = test_provider_secrets();
        let debug = format!("{provider_secrets:?}");
        let sentinels = [
            FIXTURE_PRIVATE_KEY,
            "fixture-builder-key",
            "fixture-builder-secret",
            "fixture-builder-passphrase",
        ];
        assert_zeroize_on_drop::<
            crate::bolt_v3_providers::polymarket::ResolvedBoltV3PolymarketSecrets,
        >();
        for sentinel in sentinels {
            assert!(!debug.contains(sentinel));
        }

        let mut lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "secret-evidence");
        prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            provider_secrets.redemption_signing_key(),
            original_input(RedemptionMarketKind::Standard),
            AttemptKind::Original,
            |prepared| {
                let identity = format!("{:?}", prepared.identity());
                for sentinel in sentinels {
                    let sentinel_bytes = sentinel.as_bytes();
                    assert!(
                        !prepared
                            .as_bytes()
                            .windows(sentinel_bytes.len())
                            .any(|window| window == sentinel_bytes)
                    );
                    assert!(
                        !prepared
                            .calldata_hex()
                            .windows(sentinel_bytes.len())
                            .any(|window| window == sentinel_bytes)
                    );
                    assert!(!identity.contains(sentinel));
                }
            },
        )
        .expect("provider snapshot must prepare redacted evidence");
    }

    #[test]
    fn nonce_above_configured_bound_is_a_request_input_error() {
        let mut config = test_config();
        config.maximum_safe_nonce_decimal_digits = 1;
        let credentials = test_credentials();
        let mut lease = recovery_lease(GOLDEN_STANDARD_REQUEST.len(), "nonce-bound");

        let mut input = original_input(RedemptionMarketKind::Standard);
        input.safe_nonce = U256::from(10_u8);
        let error = prepare_redemption_request(
            test_preparation_permit(),
            &mut lease,
            &config,
            &credentials,
            input,
            AttemptKind::Original,
            |_| panic!("invalid nonce must fail before the callback"),
        )
        .expect_err("over-bound nonce must fail closed");

        assert_eq!(
            error,
            RedemptionPreparationError::InvalidRequestInput {
                field: "safe_nonce"
            }
        );
    }
}
