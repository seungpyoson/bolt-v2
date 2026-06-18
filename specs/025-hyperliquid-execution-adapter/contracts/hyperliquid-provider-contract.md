# Contract: Hyperliquid Provider Binding

## Boundary

Hyperliquid support enters Bolt only through `ProviderBinding`. No strategy file or raw client module may own submit mechanics, venue rules, rounding, fillability, minimum order size, fee-adjusted sizing, or exchange mutations.

## Required Provider Binding Surface

- `validate_client`
- `required_secret_blocks`
- `secret_field_names`
- `forbidden_env_vars`
- `resolve_secrets`
- `load_live_submit_approval`
- `configured_secret_paths`
- `map_adapters`
- `build_fee_provider`
- `write_live_submit_approval_artifact`
- `write_product_submit_proof_artifact`
- `collect_canary_proof_artifacts`

## Fail-Closed Requirements

The provider binding must reject:

- Raw Hyperliquid secrets in TOML.
- Missing SSM paths for required credentials.
- Any `HYPERLIQUID_*` environment fallback visible at NT handoff.
- API-wallet mode without account address.
- Duplicate signer/API-wallet owner in one runtime.
- Raw `src/clients/hyperliquid.rs`.
- Strategy-file submit mechanics.
- Standard-perps adapter mapping without a matching consumed approval artifact.
- Spot, HIP-3, or HIP-4 adapter mapping without product-specific proof and matching consumed approval.
- Any live submit path unless Bolt's Hyperliquid provider egress model accounts `userFees` at the official Hyperliquid request weight, even when the pinned NT adapter reports a lower internal base weight.

## Market Data Adapter Contract

Hyperliquid `[data]` maps through NT `HyperliquidDataClientFactory` with TOML-owned environment, HTTP and WebSocket endpoints, proxy, timeouts, instrument refresh cadence, and transport backend. The data path must not require a live-submit approval artifact and must not pass signer material unless a later authenticated-data slice defines a separate SSM-only credential contract.

## Product Surface Evidence Contract

Each product surface must record:

- Product surface name.
- NT source evidence.
- Official documentation evidence.
- Public discovery artifact.
- Supported discovery status.
- Live submit status.
- Missing proof, if fail-closed.

## Live Submit Artifact Contract

When a product surface enables live submit, runtime TOML must provide the provider-owned approval fields under `[clients.<id>.execution.live_submit.<surface>]`: `approval_id`, `approval_artifact_path`, `approval_artifact_max_bytes`, `max_order_count`, `max_order_notional`, `product_proof_artifact_path`, `product_proof_artifact_sha256`, and `product_proof_artifact_max_bytes`. One execution client may carry per-surface `live_submit` blocks for any of its `product_surfaces` (each `live_submit` surface must appear in `product_surfaces`); a single live node arms at most one surface per execution client.

A live-submit approval artifact is valid only when all fields match the current runtime:

- `approval_id`
- `base_sha`
- `provider_id`
- `product_surface`
- `toml_checksum`
- `signer_fingerprint`
- `order_limits`
- `product_submit_proof`
- `expires_at`
- `used_at` absent before consumption

Production live-node construction must call provider live-submit approval hooks before adapter mapping. The Hyperliquid hook reads the product-submit proof artifact from the bound path under the configured artifact byte cap, verifies its sha256, validates `bolt_v3.hyperliquid_product_submit_proof.v1` semantics, then consumes the approval artifact from the TOML path. Product-submit proof semantics bind provider key, provider id, product surface, TOML checksum, and order/fill/rounding/fee proof references; HIP-4 outcomes additionally require a settlement proof reference. Consumption validates the approval against the current build, config checksum, signer fingerprint, product surface, order limits, and product-submit proof binding, persists `used_at`, returns an opaque consumed approval through the provider-neutral runtime-approval bundle, and carries the artifact order limits into shared submit admission for the approved execution client.

Operator approval artifact materialization must use the same provider binding fields as consumption. The CLI command writes the TOML-configured artifact path, derives signer fingerprint from resolved SSM-backed secrets, accepts only config/client/product-surface/expiry inputs, and rejects providers without a live-submit approval writer hook.

Operator product-submit proof materialization must also route through the provider binding. The CLI command writes the requested product-proof artifact path, accepts provider key, provider id, product surface, TOML checksum, and proof-reference paths/checksums only, and rejects providers without a product-submit proof writer hook. It must not accept signer private keys, account addresses, or alternate secret sources.

After consumption, `used_at` must be recorded and the artifact must not authorize another submit.
