# Quickstart: Hyperliquid Execution Adapter

## Plan Gate

1. Confirm branch and base:

```bash
git status --short --branch
git show --no-patch --format='%H %s' HEAD
```

2. Confirm Speckit artifacts:

```bash
SPECIFY_FEATURE_DIRECTORY=specs/025-hyperliquid-execution-adapter .specify/scripts/bash/setup-tasks.sh --json
```

3. Run relay-Claude adversarial review on the plan before implementation.

## Implementation Gate

Do not write implementation code until the plan review approves and the user explicitly approves implementation.

## MVP Verification Targets

```bash
cargo fmt --check
cargo clippy --locked --lib -- -D warnings
cargo test --locked bolt_v3_provider_binding
cargo test --locked bolt_v3_production_entrypoint
cargo test --locked hyperliquid
```

## No-Submit Proof

The first executable readiness proof must show:

- NT Hyperliquid adapter constructed through provider binding.
- Credentials resolved from SSM only.
- `HYPERLIQUID_*` environment fallback rejected or scrubbed.
- Product matrix discovered and recorded.
- Fee readiness uses official request weights.
- No exchange-mutating request was attempted.

## Live Submit Gate

Live submit remains blocked until a later slice provides a current approval artifact and product-specific proof. Standard perps are the first candidate surface; spot, HIP-3, and HIP-4 stay fail-closed until separately proven.

## Implementation Evidence Packet - 2026-06-02

Branch: `codex/025-hyperliquid-execution-adapter`

Base:
- `origin/main`: `2938bc6f6e7553e436f074163a9e5db8b4c56b11`
- PR 480 merge: `92ef8e7dfeee7baa7f5eb4eb2d13017c18fa0afe`
- Plan-review approval: relay-Claude job `33d2b208-23d3-454b-9024-15719c585a09` on planning head `3da058eea22a9863ddf3b068b625947fab88f004`

Implementation commits:
- `4355c974` provider registration, SSM-only secret gates, env-fallback rejection, signer owner validation
- `bacb0a80` product matrix artifact
- `9fff450d` no-submit readiness artifact, exchange-mutation guard, `userFees` weight inventory
- `af8cb7b3` live-submit approval artifact schema
- `b3bc0965` one-time live-submit approval consumption
- `cb7a2399` standard-perps NT adapter mapping gated by consumed approval
- `27046f0d` spot, HIP-3, and HIP-4 fail-closed missing-proof rejection
- `45ffdde4` latency-profile ops metadata and artifact export

Verification:
- `cargo fmt --check` - PASS
- `git diff --check` - PASS
- `rg -n "TODO|fix later|dbg!|println!|eprintln!" src/bolt_v3_providers/hyperliquid.rs src/bolt_v3_operator_artifacts.rs tests/bolt_v3_provider_binding.rs tests/hyperliquid_no_submit.rs` - PASS, no matches
- `cargo clippy --locked --lib -- -D warnings` - PASS
- `cargo test --locked --test bolt_v3_provider_binding` - PASS, 16 tests
- `cargo test --locked --test bolt_v3_production_entrypoint` - PASS, 5 tests
- `cargo test --locked --test hyperliquid_product_matrix` - PASS, 8 tests
- `cargo test --locked --test hyperliquid_no_submit` - PASS, 8 tests
- `cargo test --locked --test hyperliquid_live_submit_artifact` - PASS, 5 tests
- `cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_standard_perps` - PASS, 2 tests

Current live-submit state:
- Standard perps map to the NT Hyperliquid execution adapter only with a consumed, exact approval artifact.
- Spot, HIP-3, and HIP-4 remain discoverable but fail-closed for live submit with product-specific missing-proof messages.
- Latency profile is TOML-owned ops metadata only; it exports an artifact and cannot bypass submit approval gates.
