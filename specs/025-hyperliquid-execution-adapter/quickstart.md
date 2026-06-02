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

## Fail-Closed Proof

The first executable proof must show:

- NT Hyperliquid adapter constructed through provider binding.
- NT Hyperliquid market-data adapter maps for live instruments, quotes, and order books without live-submit approval.
- Credentials resolved from SSM only.
- `HYPERLIQUID_*` environment fallback rejected or scrubbed.
- Product matrix discovered and recorded.
- Fee readiness uses official request weights.
- Latency ops metadata cannot bypass live-submit gates.
- Shared exchange-mutation counters fail closed if a mutating request is observed.

## Adapter Mapping Gate

Execution adapter mapping remains blocked unless a current approval artifact is consumed for the exact configured product surface. Standard perps, spot, HIP-3, and HIP-4 all map through the NT Hyperliquid execution adapter only after that surface-bound mapper gate passes. Production `build_bolt_v3_live_node` asks provider bindings to load live-submit approvals, and Hyperliquid consumes the configured approval artifact, persists `used_at`, carries its order limits into shared submit admission, and passes the consumed approval through the provider-neutral runtime-approval bundle before NT client registration. Operators can materialize that configured artifact with `bolt-v2 operator-artifacts generate-live-submit-approval --config <root.toml> --client-key <client> --expires-at-unix-seconds <expiry>`; the command resolves signer identity through Rust AWS SSM and does not accept raw key material or an alternate artifact path. Hyperliquid `[data]` maps independently to the NT market-data adapter for live instruments, quotes, and order books without opening live submit. HIP-4 execution also requires positive TOML-owned outcome settlement polling, and Hyperliquid advertises the existing `updown` market family so HIP-4 outcome targets can pass the shared execution-client routing gate. The pinned NT `userFees` underweight is accounted by Bolt's Hyperliquid provider egress model using the official 20-weight policy. Live submit remains blocked by remaining product proof and non-HIP-4 routing work.

## Implementation Evidence Packet - 2026-06-02

Branch: `codex/025-hyperliquid-execution-adapter`

Base:
- `origin/main`: `2938bc6f6e7553e436f074163a9e5db8b4c56b11`
- PR 480 merge: `92ef8e7dfeee7baa7f5eb4eb2d13017c18fa0afe`
- Plan-review approval: relay-Claude job `33d2b208-23d3-454b-9024-15719c585a09` on planning head `3da058eea22a9863ddf3b068b625947fab88f004`

Implementation commits:
- `4355c974` provider registration, SSM-only secret gates, env-fallback rejection, signer owner validation
- `bacb0a80` product matrix artifact
- `9fff450d` exchange-mutation guard and `userFees` weight inventory
- `af8cb7b3` live-submit approval artifact schema
- `b3bc0965` one-time live-submit approval consumption
- `cb7a2399` standard-perps NT adapter mapping gated by consumed approval
- `27046f0d` prior MVP spot, HIP-3, and HIP-4 fail-closed missing-proof rejection
- `45ffdde4` latency-profile ops metadata and artifact export
- follow-up slice: spot, HIP-3, and HIP-4 NT adapter mapping gated by consumed surface-bound approval
- current slice update: NT Hyperliquid market-data adapter mapping through `[data]`
- current slice update: production live-node Hyperliquid approval artifact loading and persisted consumption
- current slice update: consumed Hyperliquid approval order limits tighten shared submit-admission caps
- current slice update: Hyperliquid provider advertises `updown` market-family support for HIP-4 outcome routing gate compatibility
- current slice update: Hyperliquid provider egress model accounts the official `userFees` request weight before live-submit validation
- current slice update: operator CLI can generate configured Hyperliquid live-submit approval artifacts through the provider binding

Verification:
- `cargo fmt --check` - PASS
- `git diff --check` - PASS
- `rg -n "TODO|fix later|dbg!|println!|eprintln!" src/bolt_v3_providers/hyperliquid.rs src/bolt_v3_operator_artifacts.rs tests/bolt_v3_provider_binding.rs tests/hyperliquid_fail_closed.rs` - PASS, no matches
- `cargo clippy --locked --lib -- -D warnings` - PASS
- `cargo clippy --locked --bin bolt-v2 -- -D warnings` - PASS
- `cargo test --locked --test bolt_v3_provider_binding hyperliquid` - PASS, 12 tests
- `cargo test --locked --test bolt_v3_production_entrypoint` - PASS, 5 tests
- `cargo test --locked --test hyperliquid_product_matrix` - PASS, 8 tests
- `cargo test --locked --test hyperliquid_fail_closed` - PASS, 6 tests
- `cargo test --locked --test hyperliquid_fail_closed user_fees_weight_policy_accounts_official_weight_and_nt_inventory` - PASS, 1 test
- `cargo test --locked --test bolt_v3_provider_binding provider_binding_writes_hyperliquid_live_submit_approval_from_configured_runtime` - PASS, 1 test
- `cargo test --locked --test bolt_v3_cli bolt_v3_cli_exposes_live_submit_approval_artifact_command_without_raw_secret_inputs` - PASS, 1 test
- `cargo test --locked --test hyperliquid_live_submit_artifact` - PASS, 6 tests
- `CARGO_TARGET_DIR=/private/tmp/bolt-v2-hyperliquid-target cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_` - PASS, 10 tests
- `cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_hip4_execution_accepts_updown_market_family_target_after_consumed_approval` - PASS, 1 test
- `CARGO_TARGET_DIR=/private/tmp/bolt-v2-hyperliquid-target cargo test --locked --lib live_node_adapter_mapping_consumes_hyperliquid_live_submit_approval_artifact` - PASS, 1 test
- `CARGO_TARGET_DIR=/private/tmp/bolt-v2-hyperliquid-target cargo test --locked --test bolt_v3_submit_admission live_submit_approval_limits_tighten_canary_caps_before_nt_submit` - PASS, 1 test
- `just source-fence` - PASS

Current live-submit state:
- Hyperliquid `[data]` maps to NT `HyperliquidDataClientFactory` with explicit TOML-owned endpoints, timeouts, refresh cadence, environment, and transport backend.
- Standard perps, spot, HIP-3, and HIP-4 map to the NT Hyperliquid execution adapter at the mapper boundary only with a consumed approval artifact for the exact configured product surface.
- Production `build_bolt_v3_live_node` consumes Hyperliquid approval artifacts through the provider binding, persists `used_at`, and applies consumed approval order limits in shared submit admission; no-submit and all-configured mapping paths do not spend approvals.
- `operator-artifacts generate-live-submit-approval` writes the configured Hyperliquid artifact path from TOML plus resolved SSM secrets, binding base SHA, config checksum, signer fingerprint, product surface, limits, and expiry without raw secret CLI inputs.
- A consumed approval for one surface cannot authorize a different surface.
- HIP-4 requires positive `outcome_settlement_poll_secs` in TOML before mapper handoff and can pass the shared `updown` market-family execution-client routing gate.
- Hyperliquid REST egress is modeled at 1200 weight/min and derates order-command validation by the official 20-weight `userFees` policy while the pinned NT adapter still reports 2.
- Latency profile is TOML-owned ops metadata only; it exports an artifact and cannot bypass submit approval gates.
- Hyperliquid-specific no-submit readiness artifacts are not part of this slice.
