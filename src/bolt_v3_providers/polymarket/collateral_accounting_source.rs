use std::collections::HashMap;

use alloy_primitives::keccak256;
use async_trait::async_trait;
use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_network::http::{HttpClient, USER_AGENT};
use nautilus_polymarket::{
    common::{
        consts::USDC_DECIMALS, credential::Secrets as PolymarketSecrets, enums::SignatureType,
    },
    http::{
        clob::PolymarketClobHttpClient,
        query::{AssetType, GetBalanceAllowanceParams},
    },
    signing::eip712::{CTF_EXCHANGE, NEG_RISK_CTF_EXCHANGE},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_operator_artifacts::{BoltV3OperatorArtifactError, json_artifact_sha256},
    bolt_v3_providers::{
        ClobV2CollateralAccountingSourceMaterialization,
        ClobV2CollateralAccountingSourceMaterializationRequest, ExternalSnapshotConfirmationPolicy,
        fetch_external_snapshot_with_retries,
    },
};

const CLOB_V2_COLLATERAL_BALANCE_ALLOWANCE_PATH: &str = "/balance-allowance";
const CLOB_V2_COLLATERAL_ASSET_TYPE: &str = "COLLATERAL";
const CLOB_V2_COLLATERAL_BALANCE_UNIT: &str = "p_usd";
const CLOB_V2_COLLATERAL_PUSD_MICRO_SCALE: u32 = 1_000_000;
const ON_CHAIN_COLLATERAL_JSON_RPC_VERSION: &str = "2.0";
const ON_CHAIN_COLLATERAL_JSON_RPC_ETH_CALL_METHOD: &str = "eth_call";
const ON_CHAIN_COLLATERAL_JSON_RPC_CHAIN_ID_METHOD: &str = "eth_chainId";
const ON_CHAIN_COLLATERAL_JSON_RPC_BLOCK_NUMBER_METHOD: &str = "eth_blockNumber";
const ON_CHAIN_COLLATERAL_JSON_RPC_GET_BLOCK_METHOD: &str = "eth_getBlockByNumber";
const ON_CHAIN_COLLATERAL_JSON_RPC_GET_CODE_METHOD: &str = "eth_getCode";
const ON_CHAIN_COLLATERAL_JSON_RPC_GET_STORAGE_METHOD: &str = "eth_getStorageAt";
const ON_CHAIN_COLLATERAL_JSON_RPC_LATEST_BLOCK: &str = "latest";
const ON_CHAIN_COLLATERAL_JSON_RPC_ID: u64 = 1;
const ON_CHAIN_COLLATERAL_CONTENT_TYPE_HEADER: &str = "Content-Type";
const ON_CHAIN_COLLATERAL_CONTENT_TYPE_JSON: &str = "application/json";
const ON_CHAIN_COLLATERAL_BALANCE_OF_SIGNATURE: &str = "balanceOf(address)";
const ON_CHAIN_COLLATERAL_ALLOWANCE_SIGNATURE: &str = "allowance(address,address)";
const ON_CHAIN_COLLATERAL_HEX_PREFIX: &str = "0x";
const ON_CHAIN_COLLATERAL_EVM_WORD_HEX_LEN: usize = 64;
const ON_CHAIN_COLLATERAL_EVM_ADDRESS_HEX_LEN: usize = 40;

pub async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_with_policy(
        request, true,
    )
    .await
}

async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_with_policy(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
    retry_initial_fetch: bool,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    let context = collateral_context(request)?;
    if let Some(on_chain) = context.cfg.on_chain_collateral.as_ref() {
        return materialize_clob_v2_collateral_accounting_source_from_on_chain_pusd_allowance(
            &context,
            on_chain,
            retry_initial_fetch,
        )
        .await;
    }
    materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_context(
        &context,
        request,
        retry_initial_fetch,
    )
    .await
}

struct ClobV2CollateralAccountingContext<'a> {
    schema_version: u32,
    on_chain_balance_allowance_record_kind: &'static str,
    execution_client_id: &'a str,
    cfg: super::PolymarketExecutionConfig,
}

fn collateral_context(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
) -> Result<ClobV2CollateralAccountingContext<'_>, BoltV3OperatorArtifactError> {
    let strategy = request
        .loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == request.strategy_instance_id)
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "strategy_instance_id",
        })?;
    let execution_client_id = strategy.config.execution_client_id.as_str();
    let client = request.loaded.root.clients.get(execution_client_id).ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "execution_client_id",
        },
    )?;
    if client.venue.as_str() != super::KEY {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "execution_client_provider",
        });
    }
    let execution = client
        .execution
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "execution" })?;
    let cfg: super::PolymarketExecutionConfig = execution.clone().try_into().map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field: "execution" }
    })?;
    Ok(ClobV2CollateralAccountingContext {
        schema_version: request.schema_version,
        on_chain_balance_allowance_record_kind: request.on_chain_balance_allowance_record_kind,
        execution_client_id,
        cfg,
    })
}

async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_context(
    context: &ClobV2CollateralAccountingContext<'_>,
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
    retry_initial_fetch: bool,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    let resolved =
        request
            .resolved
            .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "resolved_secrets",
            })?;
    let secrets = super::secrets_for(context.execution_client_id, resolved).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "resolved_secrets",
        }
    })?;
    let polymarket_secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.as_str().to_owned()),
        Some(secrets.api_secret.as_str().to_owned()),
        Some(secrets.passphrase.as_str().to_owned()),
        context.cfg.funder.clone(),
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "polymarket_credentials",
    })?;
    let signature_type = super::nt_signature_type(context.cfg.signature_type);
    let http_client = PolymarketClobHttpClient::new(
        polymarket_secrets.credential,
        polymarket_secrets.address,
        Some(context.cfg.base_url_http.clone()),
        context.cfg.http_timeout_secs,
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "execution.base_url_http",
    })?;
    let confirmation_policy = ExternalSnapshotConfirmationPolicy::from_retry_fields(
        context.cfg.max_retries,
        context.cfg.retry_delay_initial_ms,
        context.cfg.retry_delay_max_ms,
    );
    let balance_allowance = if retry_initial_fetch {
        fetch_external_snapshot_with_retries(confirmation_policy, || {
            http_client.get_balance_allowance(balance_allowance_params(signature_type))
        })
        .await
    } else {
        http_client
            .get_balance_allowance(balance_allowance_params(signature_type))
            .await
    }
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "balance_allowance_response",
    })?;
    let micro_scale = Decimal::from(CLOB_V2_COLLATERAL_PUSD_MICRO_SCALE);
    let p_usd_balance = balance_allowance.balance / micro_scale;
    let p_usd_allowance = balance_allowance.allowance.ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "p_usd_allowance",
        },
    )? / micro_scale;
    let p_usd_balance = p_usd_balance.to_string();
    let p_usd_allowance = p_usd_allowance.to_string();
    let base_url_http_sha256 = sha256_text(&context.cfg.base_url_http);
    let proof = ClobV2CollateralAccountingBalanceAllowanceProof {
        schema_version: request.schema_version,
        record_kind: request.balance_allowance_record_kind,
        execution_client_id: context.execution_client_id,
        base_url_http_sha256: &base_url_http_sha256,
        request_path: CLOB_V2_COLLATERAL_BALANCE_ALLOWANCE_PATH,
        asset_type: CLOB_V2_COLLATERAL_ASSET_TYPE,
        signature_type: signature_type_label(context.cfg.signature_type),
        balance_unit: CLOB_V2_COLLATERAL_BALANCE_UNIT,
        p_usd_balance: &p_usd_balance,
        p_usd_allowance: &p_usd_allowance,
    };
    let collateral_accounting_source_sha256 = json_artifact_sha256(&proof)?;

    Ok(ClobV2CollateralAccountingSourceMaterialization {
        p_usd_balance,
        p_usd_allowance,
        collateral_accounting_source_sha256,
    })
}

async fn materialize_clob_v2_collateral_accounting_source_from_on_chain_pusd_allowance(
    context: &ClobV2CollateralAccountingContext<'_>,
    on_chain: &super::PolymarketOnChainCollateralConfig,
    retry_initial_fetch: bool,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    let confirmation_policy = ExternalSnapshotConfirmationPolicy::from_retry_fields(
        context.cfg.max_retries,
        context.cfg.retry_delay_initial_ms,
        context.cfg.retry_delay_max_ms,
    );
    let funder = context.cfg.funder.as_deref().ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "execution.funder",
        },
    )?;
    let fetch = || collect_on_chain_pusd_allowance(context, on_chain, funder);
    let proof = if retry_initial_fetch {
        fetch_external_snapshot_with_retries(confirmation_policy, fetch).await
    } else {
        fetch().await
    }?;
    let collateral_accounting_source_sha256 = json_artifact_sha256(&proof)?;
    Ok(ClobV2CollateralAccountingSourceMaterialization {
        p_usd_balance: proof.p_usd_balance,
        p_usd_allowance: proof.effective_p_usd_allowance,
        collateral_accounting_source_sha256,
    })
}

async fn collect_on_chain_pusd_allowance(
    context: &ClobV2CollateralAccountingContext<'_>,
    on_chain: &super::PolymarketOnChainCollateralConfig,
    funder: &str,
) -> Result<ClobV2OnChainCollateralAccountingProof, BoltV3OperatorArtifactError> {
    let normalized_funder = normalized_evm_address(funder)?;
    let collateral_token_address = normalized_evm_address(&on_chain.collateral_token_address)?;
    let ctf_exchange_spender = normalized_evm_address(&format!("{CTF_EXCHANGE:#x}"))?;
    let neg_risk_ctf_exchange_spender =
        normalized_evm_address(&format!("{NEG_RISK_CTF_EXCHANGE:#x}"))?;
    let client = OnChainCollateralRpcClient::try_new(on_chain, context.cfg.http_timeout_secs)?;
    let balance_raw = client
        .eth_call_u256_word(
            &collateral_token_address,
            &balance_of_calldata(&normalized_funder),
        )
        .await?;
    let ctf_allowance_raw = client
        .eth_call_u256_word(
            &collateral_token_address,
            &allowance_calldata(&normalized_funder, &ctf_exchange_spender),
        )
        .await?;
    let neg_risk_allowance_raw = client
        .eth_call_u256_word(
            &collateral_token_address,
            &allowance_calldata(&normalized_funder, &neg_risk_ctf_exchange_spender),
        )
        .await?;
    let effective_allowance_raw = if ctf_allowance_raw <= neg_risk_allowance_raw {
        ctf_allowance_raw
    } else {
        neg_risk_allowance_raw
    };
    Ok(ClobV2OnChainCollateralAccountingProof {
        schema_version: context.schema_version,
        record_kind: context.on_chain_balance_allowance_record_kind,
        execution_client_id: context.execution_client_id.to_string(),
        chain_id: on_chain.chain_id,
        rpc_url_sha256: sha256_text(&on_chain.rpc_url),
        collateral_token_address_sha256: sha256_text(&collateral_token_address),
        funder_sha256: sha256_text(&normalized_funder),
        ctf_exchange_spender_sha256: sha256_text(&ctf_exchange_spender),
        neg_risk_ctf_exchange_spender_sha256: sha256_text(&neg_risk_ctf_exchange_spender),
        block_tag: ON_CHAIN_COLLATERAL_JSON_RPC_LATEST_BLOCK.to_string(),
        balance_unit: CLOB_V2_COLLATERAL_BALANCE_UNIT.to_string(),
        p_usd_balance: u256_word_to_decimal_string(&balance_raw, USDC_DECIMALS),
        ctf_exchange_p_usd_allowance: u256_word_to_decimal_string(
            &ctf_allowance_raw,
            USDC_DECIMALS,
        ),
        neg_risk_ctf_exchange_p_usd_allowance: u256_word_to_decimal_string(
            &neg_risk_allowance_raw,
            USDC_DECIMALS,
        ),
        effective_p_usd_allowance: u256_word_to_decimal_string(
            &effective_allowance_raw,
            USDC_DECIMALS,
        ),
    })
}

pub(super) struct OnChainCollateralRpcClient<'a> {
    client: HttpClient,
    on_chain: &'a super::PolymarketOnChainCollateralConfig,
    timeout_secs: u64,
}

impl<'a> OnChainCollateralRpcClient<'a> {
    pub(super) fn try_new(
        on_chain: &'a super::PolymarketOnChainCollateralConfig,
        timeout_secs: u64,
    ) -> Result<Self, BoltV3OperatorArtifactError> {
        let client = HttpClient::new(
            HashMap::from([(USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string())]),
            Vec::new(),
            Vec::new(),
            None,
            Some(timeout_secs),
            None,
        )
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_client",
        })?;
        Ok(Self {
            client,
            on_chain,
            timeout_secs,
        })
    }

    pub(super) async fn chain_id(&self) -> Result<u64, BoltV3OperatorArtifactError> {
        self.quantity(ON_CHAIN_COLLATERAL_JSON_RPC_CHAIN_ID_METHOD)
            .await
    }

    pub(super) async fn block_number(&self) -> Result<u64, BoltV3OperatorArtifactError> {
        self.quantity(ON_CHAIN_COLLATERAL_JSON_RPC_BLOCK_NUMBER_METHOD)
            .await
    }

    pub(super) async fn eth_call_u256_word(
        &self,
        contract_address: &str,
        calldata: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        self.eth_call_u256_word_at(
            contract_address,
            calldata,
            ON_CHAIN_COLLATERAL_JSON_RPC_LATEST_BLOCK,
        )
        .await
    }

    pub(super) async fn eth_call_u256_word_at(
        &self,
        contract_address: &str,
        calldata: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        let result = self
            .request(
                ON_CHAIN_COLLATERAL_JSON_RPC_ETH_CALL_METHOD,
                serde_json::json!([
                    {
                        "to": format!("{ON_CHAIN_COLLATERAL_HEX_PREFIX}{contract_address}"),
                        "data": calldata,
                    },
                    block_tag,
                ]),
            )
            .await?;
        let result =
            result
                .as_str()
                .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                    field: "on_chain_collateral.rpc_result",
                })?;
        parse_u256_word_hex(result)
    }

    pub(super) async fn code_at(
        &self,
        contract_address: &str,
        block_tag: &str,
    ) -> Result<Vec<u8>, BoltV3OperatorArtifactError> {
        let result = self
            .request(
                ON_CHAIN_COLLATERAL_JSON_RPC_GET_CODE_METHOD,
                serde_json::json!([
                    format!("{ON_CHAIN_COLLATERAL_HEX_PREFIX}{contract_address}"),
                    block_tag,
                ]),
            )
            .await?;
        parse_hex_bytes(&result)
    }

    pub(super) async fn storage_word_at(
        &self,
        contract_address: &str,
        slot: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        let result = self
            .request(
                ON_CHAIN_COLLATERAL_JSON_RPC_GET_STORAGE_METHOD,
                serde_json::json!([
                    format!("{ON_CHAIN_COLLATERAL_HEX_PREFIX}{contract_address}"),
                    slot,
                    block_tag,
                ]),
            )
            .await?;
        let encoded =
            result
                .as_str()
                .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                    field: "on_chain_collateral.rpc_result",
                })?;
        parse_u256_word_hex(encoded)
    }

    pub(super) async fn block_header(
        &self,
        block_number: u64,
    ) -> Result<OnChainBlockHeader, BoltV3OperatorArtifactError> {
        let block_tag = format!("{ON_CHAIN_COLLATERAL_HEX_PREFIX}{block_number:x}");
        let result = self
            .request(
                ON_CHAIN_COLLATERAL_JSON_RPC_GET_BLOCK_METHOD,
                serde_json::json!([block_tag, false]),
            )
            .await?;
        let wire: OnChainBlockHeaderWire = serde_json::from_value(result).map_err(|_| {
            BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "on_chain_collateral.rpc_result",
            }
        })?;
        Ok(OnChainBlockHeader {
            number: quantity_from_hex(&wire.number)?,
            hash: wire.hash,
            timestamp_secs: quantity_from_hex(&wire.timestamp)?,
        })
    }

    async fn quantity(&self, method: &'static str) -> Result<u64, BoltV3OperatorArtifactError> {
        let result = self.request(method, serde_json::json!([])).await?;
        let encoded =
            result
                .as_str()
                .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                    field: "on_chain_collateral.rpc_result",
                })?;
        u64::from_str_radix(
            encoded.strip_prefix(ON_CHAIN_COLLATERAL_HEX_PREFIX).ok_or(
                BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                    field: "on_chain_collateral.rpc_result",
                },
            )?,
            16,
        )
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        })
    }

    async fn request(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, BoltV3OperatorArtifactError> {
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": ON_CHAIN_COLLATERAL_JSON_RPC_VERSION,
            "id": ON_CHAIN_COLLATERAL_JSON_RPC_ID,
            "method": method,
            "params": params,
        }))
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_request",
        })?;
        let response = self
            .client
            .post(
                self.on_chain.rpc_url.clone(),
                None,
                Some(HashMap::from([(
                    ON_CHAIN_COLLATERAL_CONTENT_TYPE_HEADER.to_string(),
                    ON_CHAIN_COLLATERAL_CONTENT_TYPE_JSON.to_string(),
                )])),
                Some(body),
                Some(self.timeout_secs),
                None,
            )
            .await
            .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "on_chain_collateral.rpc_response",
            })?;
        if !response.status.is_success() {
            return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "on_chain_collateral.rpc_status",
            });
        }
        decode_json_rpc_result(&response.body)
    }
}

fn decode_json_rpc_result(body: &[u8]) -> Result<serde_json::Value, BoltV3OperatorArtifactError> {
    let rpc_response: JsonRpcResponse = serde_json::from_slice(body).map_err(|_| {
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_response",
        }
    })?;
    if rpc_response.error.is_some() {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_error",
        });
    }
    rpc_response
        .result
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        })
}

#[async_trait(?Send)]
pub(crate) trait OnChainCollateralRpc: Send + Sync {
    async fn chain_id(&self) -> Result<u64, BoltV3OperatorArtifactError>;
    async fn block_number(&self) -> Result<u64, BoltV3OperatorArtifactError>;
    async fn eth_call_u256_word_at(
        &self,
        contract_address: &str,
        calldata: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError>;
    async fn code_sha256_at(
        &self,
        contract_address: &str,
        block_tag: &str,
    ) -> Result<String, BoltV3OperatorArtifactError>;
    async fn storage_word_at(
        &self,
        contract_address: &str,
        slot: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError>;
    async fn block_header(
        &self,
        block_number: u64,
    ) -> Result<OnChainBlockHeader, BoltV3OperatorArtifactError>;
}

#[async_trait(?Send)]
impl OnChainCollateralRpc for OnChainCollateralRpcClient<'_> {
    async fn chain_id(&self) -> Result<u64, BoltV3OperatorArtifactError> {
        OnChainCollateralRpcClient::chain_id(self).await
    }

    async fn block_number(&self) -> Result<u64, BoltV3OperatorArtifactError> {
        OnChainCollateralRpcClient::block_number(self).await
    }

    async fn eth_call_u256_word_at(
        &self,
        contract_address: &str,
        calldata: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        OnChainCollateralRpcClient::eth_call_u256_word_at(
            self,
            contract_address,
            calldata,
            block_tag,
        )
        .await
    }

    async fn code_sha256_at(
        &self,
        contract_address: &str,
        block_tag: &str,
    ) -> Result<String, BoltV3OperatorArtifactError> {
        OnChainCollateralRpcClient::code_at(self, contract_address, block_tag)
            .await
            .map(|code| crate::bolt_v3_source_integrity::sha256_hex_lower(&code))
    }

    async fn storage_word_at(
        &self,
        contract_address: &str,
        slot: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        OnChainCollateralRpcClient::storage_word_at(self, contract_address, slot, block_tag).await
    }

    async fn block_header(
        &self,
        block_number: u64,
    ) -> Result<OnChainBlockHeader, BoltV3OperatorArtifactError> {
        OnChainCollateralRpcClient::block_header(self, block_number).await
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OnChainBlockHeader {
    pub(crate) number: u64,
    pub(crate) hash: String,
    pub(crate) timestamp_secs: u64,
}

#[derive(Deserialize)]
struct OnChainBlockHeaderWire {
    number: String,
    hash: String,
    timestamp: String,
}

fn quantity_from_hex(value: &str) -> Result<u64, BoltV3OperatorArtifactError> {
    u64::from_str_radix(
        value.strip_prefix(ON_CHAIN_COLLATERAL_HEX_PREFIX).ok_or(
            BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
                field: "on_chain_collateral.rpc_result",
            },
        )?,
        16,
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "on_chain_collateral.rpc_result",
    })
}

fn parse_hex_bytes(value: &serde_json::Value) -> Result<Vec<u8>, BoltV3OperatorArtifactError> {
    let encoded = value
        .as_str()
        .ok_or(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        })?;
    let encoded = encoded.strip_prefix(ON_CHAIN_COLLATERAL_HEX_PREFIX).ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        },
    )?;
    if encoded.is_empty() {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        });
    }
    hex::decode(encoded).map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "on_chain_collateral.rpc_result",
    })
}

pub(super) fn normalized_evm_address(value: &str) -> Result<String, BoltV3OperatorArtifactError> {
    let rest = value.strip_prefix(ON_CHAIN_COLLATERAL_HEX_PREFIX).ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.address",
        },
    )?;
    if rest.len() != ON_CHAIN_COLLATERAL_EVM_ADDRESS_HEX_LEN
        || !rest.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.address",
        });
    }
    Ok(rest.to_ascii_lowercase())
}

fn balance_of_calldata(owner: &str) -> String {
    let selector = erc20_selector_hex(ON_CHAIN_COLLATERAL_BALANCE_OF_SIGNATURE);
    format!("{ON_CHAIN_COLLATERAL_HEX_PREFIX}{selector}{owner:0>64}")
}

fn allowance_calldata(owner: &str, spender: &str) -> String {
    let selector = erc20_selector_hex(ON_CHAIN_COLLATERAL_ALLOWANCE_SIGNATURE);
    format!("{ON_CHAIN_COLLATERAL_HEX_PREFIX}{selector}{owner:0>64}{spender:0>64}")
}

fn erc20_selector_hex(signature: &str) -> String {
    hex::encode(&keccak256(signature.as_bytes())[..4])
}

fn parse_u256_word_hex(value: &str) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
    let rest = value.strip_prefix(ON_CHAIN_COLLATERAL_HEX_PREFIX).ok_or(
        BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        },
    )?;
    if rest.len() != ON_CHAIN_COLLATERAL_EVM_WORD_HEX_LEN
        || !rest.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        });
    }
    let bytes =
        hex::decode(rest).map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "on_chain_collateral.rpc_result",
        })?;
    let mut word = [0_u8; 32];
    word.copy_from_slice(&bytes);
    Ok(word)
}

fn u256_word_to_decimal_string(word: &[u8; 32], decimals: u32) -> String {
    let mut digits = vec![0_u8];
    for byte in word {
        let mut carry = u32::from(*byte);
        for digit in &mut digits {
            let value = u32::from(*digit) * 256 + carry;
            *digit = (value % 10) as u8;
            carry = value / 10;
        }
        while carry != 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    let integer = digits
        .iter()
        .rev()
        .map(|digit| char::from(b'0' + *digit))
        .collect::<String>();
    decimal_string_from_integer_units(integer, decimals)
}

fn decimal_string_from_integer_units(mut integer: String, decimals: u32) -> String {
    if decimals == 0 {
        return trim_decimal_string(integer, String::new());
    }
    let decimals = decimals as usize;
    if integer.len() <= decimals {
        let mut padded = String::with_capacity(decimals + 1);
        padded.extend(std::iter::repeat_n('0', decimals + 1 - integer.len()));
        padded.push_str(&integer);
        integer = padded;
    }
    let split_at = integer.len() - decimals;
    let fractional = integer.split_off(split_at);
    trim_decimal_string(integer, fractional)
}

fn trim_decimal_string(integer: String, fractional: String) -> String {
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let fractional = fractional.trim_end_matches('0');
    if fractional.is_empty() {
        integer.to_string()
    } else {
        format!("{integer}.{fractional}")
    }
}

fn balance_allowance_params(signature_type: SignatureType) -> GetBalanceAllowanceParams {
    GetBalanceAllowanceParams {
        asset_type: Some(AssetType::Collateral),
        token_id: None,
        signature_type: Some(signature_type),
    }
}

fn signature_type_label(value: super::PolymarketSignatureType) -> &'static str {
    match value {
        super::PolymarketSignatureType::Eoa => "eoa",
        super::PolymarketSignatureType::PolyProxy => "poly_proxy",
        super::PolymarketSignatureType::PolyGnosisSafe => "poly_gnosis_safe",
    }
}

fn sha256_text(value: &str) -> String {
    crate::bolt_v3_source_integrity::sha256_hex_lower(value.as_bytes())
}

#[derive(Serialize)]
struct ClobV2CollateralAccountingBalanceAllowanceProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    execution_client_id: &'a str,
    base_url_http_sha256: &'a str,
    request_path: &'static str,
    asset_type: &'static str,
    signature_type: &'static str,
    balance_unit: &'static str,
    p_usd_balance: &'a str,
    p_usd_allowance: &'a str,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ClobV2OnChainCollateralAccountingProof {
    schema_version: u32,
    record_kind: &'static str,
    execution_client_id: String,
    chain_id: u64,
    rpc_url_sha256: String,
    collateral_token_address_sha256: String,
    funder_sha256: String,
    ctf_exchange_spender_sha256: String,
    neg_risk_ctf_exchange_spender_sha256: String,
    block_tag: String,
    balance_unit: String,
    p_usd_balance: String,
    ctf_exchange_p_usd_allowance: String,
    neg_risk_ctf_exchange_p_usd_allowance: String,
    effective_p_usd_allowance: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collateral_accounting_proofs_label_scaled_p_usd_values() {
        assert_eq!(CLOB_V2_COLLATERAL_BALANCE_UNIT, "p_usd");
    }

    #[test]
    fn on_chain_calldata_encodes_erc20_balance_and_allowance_selectors() {
        let owner = normalized_evm_address("0xAa00000000000000000000000000000000000011")
            .expect("owner address should normalize");
        let spender = normalized_evm_address("0xbB00000000000000000000000000000000000022")
            .expect("spender address should normalize");

        assert_eq!(owner, "aa00000000000000000000000000000000000011");
        assert_eq!(
            balance_of_calldata(&owner),
            "0x70a08231000000000000000000000000aa00000000000000000000000000000000000011"
        );
        assert_eq!(
            allowance_calldata(&owner, &spender),
            "0xdd62ed3e000000000000000000000000aa00000000000000000000000000000000000011000000000000000000000000bb00000000000000000000000000000000000022"
        );
    }

    #[test]
    fn parse_u256_word_hex_rejects_non_word_results() {
        assert!(parse_u256_word_hex("1").is_err());
        assert!(parse_u256_word_hex("0x1").is_err());
        assert!(
            parse_u256_word_hex(
                "0x00000000000000000000000000000000000000000000000000000000000000zz"
            )
            .is_err()
        );
    }

    #[test]
    fn governed_proxy_capture_decodes_through_the_production_json_rpc_parser() {
        let result = decode_json_rpc_result(include_bytes!(
            "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-collateral-proxy-implementation.json"
        ))
        .expect("governed proxy response should decode");
        let encoded = result
            .as_str()
            .expect("governed proxy response should contain a word");
        assert_eq!(
            parse_u256_word_hex(encoded).expect("governed proxy word should decode"),
            word("0x0000000000000000000000006bbcef9f7ef3b6c592c99e0f206a0de94ad0925")
        );
    }

    #[test]
    fn u256_word_to_decimal_string_formats_zero_fractional_and_max_values() {
        let zero = word("0x0000000000000000000000000000000000000000000000000000000000000000");
        let one_micro = word("0x0000000000000000000000000000000000000000000000000000000000000001");
        let one_pusd = word("0x00000000000000000000000000000000000000000000000000000000000f4240");
        let max = word("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");

        assert_eq!(u256_word_to_decimal_string(&zero, USDC_DECIMALS), "0");
        assert_eq!(
            u256_word_to_decimal_string(&one_micro, USDC_DECIMALS),
            "0.000001"
        );
        assert_eq!(u256_word_to_decimal_string(&one_pusd, USDC_DECIMALS), "1");
        assert_eq!(
            u256_word_to_decimal_string(&max, 0),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
    }

    fn word(value: &str) -> [u8; 32] {
        parse_u256_word_hex(value).expect("test word should parse")
    }
}
