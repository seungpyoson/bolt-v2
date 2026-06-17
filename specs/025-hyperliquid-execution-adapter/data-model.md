# Data Model: Hyperliquid Execution Adapter

## HyperliquidClientBlock

- `provider_id`: existing Bolt provider identifier.
- `data`: optional `HyperliquidDataConfig`.
- `execution`: optional `HyperliquidExecutionConfig`.
- `secret_set`: `HyperliquidSecretSet` only when execution is configured.
- `latency_profile`: optional `HyperliquidLatencyProfile`.

Validation:
- Must be declared in TOML.
- Must not contain raw secret material.
- Must map through `ProviderBinding`.

## HyperliquidDataConfig

- `environment`: explicit network/profile name from TOML.
- `base_url_ws`
- `base_url_http`
- `proxy_url`
- `http_timeout_secs`
- `ws_timeout_secs`
- `update_instruments_interval_mins`
- `transport_backend`

Validation:
- Data-only config requires no SSM secret block.
- Endpoints, timeouts, refresh cadence, environment, and transport backend are TOML-owned.
- NT data config leaves `private_key` unset in this slice.

## HyperliquidExecutionConfig

- `environment`: explicit network/profile name from TOML.
- `execution_mode`: `HyperliquidExecutionMode`.
- `product_surfaces`: enabled discovery surfaces.
- `account_id`
- execution endpoints and retry policy.
- `live_submit`: optional per-surface map keyed by product surface (`standard_perps`, `spot`, `hip3_builder_perps`, `hip4_outcomes`), declared as `[clients.<id>.execution.live_submit.<surface>]`. Each surface entry requires `approval_id`, `approval_artifact_path`, `approval_artifact_max_bytes`, `max_order_count`, `max_order_notional`, `product_proof_artifact_path`, `product_proof_artifact_sha256`, and `product_proof_artifact_max_bytes`. Every configured `live_submit` surface must also appear in `product_surfaces`.

Validation:
- Execution requires SSM-backed secrets.
- Live-submit mapping requires consumed surface-bound approval loaded through the provider binding before production live-node adapter mapping.
- Consumed approval order limits must tighten shared submit-admission count and notional caps for the approved execution client.
- Production live-node approval loading must read the bound product-submit proof artifact under its own configured artifact byte cap, verify its sha256, and validate its schema before consuming the one-time approval or opening live execution mapping.
- The product-submit proof artifact uses `record_kind = "bolt_v3.hyperliquid_product_submit_proof.v1"`, binds `provider_key`, `provider_id`, `product_surface`, and `toml_checksum`, and carries `order_proof`, `fill_proof`, `rounding_proof`, and `fee_proof` artifact references. HIP-4 outcome product proofs also require `settlement_proof`; non-HIP-4 surfaces reject a settlement proof on this schema.

## HyperliquidSecretSet

- `private_key_ssm_path`
- `account_address_ssm_path`
- `vault_address_ssm_path`

Validation:
- Every secret value resolves from SSM via Rust AWS SDK.
- Raw private keys and environment fallback are rejected.
- API-wallet modes require account address.

## HyperliquidExecutionMode

Values:
- `direct_account`
- `vault`
- `master_account_api_wallet`
- `subaccount_api_wallet`

Validation:
- Mode controls required SSM paths and NT config construction.
- Mode is part of approval-artifact binding.

## HyperliquidSignerOwner

- `signer_fingerprint`
- `provider_id`
- `execution_mode`
- `account_address`
- `process_owner_id`

Validation:
- One owner per signer/API wallet in the runtime.
- Duplicate owners fail closed.

## HyperliquidProductMatrix

- `standard_perps`: discovery status, submit status, source evidence.
- `spot`: discovery status, submit status, source evidence.
- `hip3_builder_perps`: discovery status, submit status, source evidence.
- `hip4_outcomes`: discovery status, submit status, source evidence.

Validation:
- Discovery does not imply submit readiness.
- Unknown or unsupported surfaces remain fail-closed.

## HyperliquidLiveSubmitApprovalArtifact

- `schema_version`
- `record_kind`
- `provider_key`
- `approval_id`
- `base_sha`
- `provider_id`
- `product_surface`
- `toml_checksum`
- `signer_fingerprint`
- `order_limits`
- `product_submit_proof`
- `expires_at`
- `used_at`

Validation:
- One-time use.
- Current runtime must match every bound field.
- Expired, stale, reused, or broader-than-config artifacts are rejected.

## HyperliquidEgressModel

- `read_requests`
- `fee_requests`
- `exchange_mutation_requests`
- `request_weights`

Validation:
- Official request weights are used.
- Local info-node reads do not bypass accounting.

## HyperliquidLatencyProfile

- `local_info_node_url`
- `placement_profile`
- `measurement_artifact_path`

Validation:
- Values come from TOML.
- Profile cannot change submit gates or signer ownership.
