# Contract: Hyperliquid Provider Binding

## Boundary

Hyperliquid support enters Bolt only through `ProviderBinding`. No strategy file or raw client module may own submit mechanics, venue rules, rounding, fillability, minimum order size, fee-adjusted sizing, or exchange mutations.

## Required Provider Binding Surface

- `validate_client`
- `required_secret_blocks`
- `secret_field_names`
- `forbidden_env_vars`
- `resolve_secrets`
- `configured_secret_paths`
- `map_adapters`
- `build_fee_provider`
- `venue_egress_model`
- operator artifact export

## Fail-Closed Requirements

The provider binding must reject:

- Raw Hyperliquid secrets in TOML.
- Missing SSM paths for required credentials.
- Any `HYPERLIQUID_*` environment fallback visible at NT handoff.
- API-wallet mode without account address.
- Duplicate signer/API-wallet owner in one runtime.
- Raw `src/clients/hyperliquid.rs`.
- Strategy-file submit mechanics.
- Standard-perps live submit without a matching approval artifact.
- Spot, HIP-3, or HIP-4 live submit without product-specific proof.
- No-submit readiness if any exchange-mutating request occurs.

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

A live-submit approval artifact is valid only when all fields match the current runtime:

- `approval_id`
- `base_sha`
- `provider_id`
- `product_surface`
- `toml_checksum`
- `signer_fingerprint`
- `order_limits`
- `expires_at`
- `used_at` absent before consumption

After consumption, `used_at` must be recorded and the artifact must not authorize another submit.
