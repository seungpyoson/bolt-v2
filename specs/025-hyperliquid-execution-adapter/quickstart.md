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
- Hyperliquid fee-provider resolution goes through the provider binding, returns no fee bound before warmup, and warms from `userFees.userCrossRate` for the SSM-resolved account address.
- Live-submit approval artifacts bind configured product-submit proof artifact path and sha256; the bound proof must also validate as `bolt_v3.hyperliquid_product_submit_proof.v1` for the exact provider id, product surface, TOML checksum, and required proof references.
- Latency ops metadata cannot bypass live-submit gates.
- Shared exchange-mutation counters fail closed if a mutating request is observed.

## Adapter Mapping Gate

Execution adapter mapping remains blocked unless a current approval artifact is consumed for the exact configured product surface. Standard perps, spot, HIP-3, and HIP-4 all map through the NT Hyperliquid execution adapter only after that surface-bound mapper gate passes. Production `build_bolt_v3_live_node` scopes trade transport clients to the active strategy and enabled canary proof-policy references before provider approval loading, then asks provider bindings to load live-submit approvals for the scoped client set. Hyperliquid reads the configured product-submit proof artifact under its own configured byte cap, verifies its sha256, validates the proof schema for the exact provider id/product surface/TOML checksum plus order/fill/rounding/fee references, then consumes the configured approval artifact under the separate approval-artifact byte cap, persists `used_at`, carries its order limits into shared submit admission, and passes the consumed approval through the provider-neutral runtime-approval bundle before NT client registration. HIP-4 proof schema validation additionally requires a settlement proof reference. Static/direct target surface mismatches are rejected before artifact consumption, so a one-time approval is not spent by a route configuration that the mapper already knows must fail. Operators can materialize that configured artifact with `bolt-v2 provider-artifacts generate-live-submit-approval --config <root.toml> --client-key <client> --product-surface <surface> --expires-at-unix-seconds <expiry>`; the command resolves signer identity through Rust AWS SSM and does not accept raw key material or an alternate artifact path. Hyperliquid `[data]` maps independently to the NT market-data adapter for live instruments, quotes, and order books without opening live submit. HIP-4 execution also requires positive TOML-owned outcome settlement polling, and Hyperliquid permits the existing `updown` market family only when the execution client selects the HIP-4 outcomes product surface. Hyperliquid also advertises `hyperliquid_instrument` for static/direct standard-perps, spot, and HIP-3 routing identity; each static target must match one of the execution client's configured and approved product surfaces (a single live node arms at most one surface per execution client), must declare TOML-owned canary sizing constraints such as `quantity_step`, and does not enable binary rotating-market selection. The Hyperliquid provider also owns the canary proof artifact collector, so `provider-artifacts collect-canary-proof-artifacts` can write a no-resolution gate session, candidate source, and order intent for a static Hyperliquid instrument with the exact configured execution client. The pinned NT `userFees` underweight is accounted by Bolt's Hyperliquid provider egress model using the official 20-weight policy. Hyperliquid fee-provider construction is wired through the provider registry and warms through NT `userFees`, caching `userCrossRate` as the taker fee bound in basis points for the requested instrument; before successful warmup, fee lookup returns no bound so strategy sizing remains fail-closed instead of assuming zero fees. The product matrix now marks all four surfaces approval-gated: they are openable only through the exact consumed approval, bound product proof, route compatibility, and shared submit-admission limits.

## Post-Merge Production Live Arming Gate - 2026-06-03

PR #544 merged the selected-client live-submit artifact secret-resolution slice
to `main` at `e6c5e5b993bf1a29658ce7685999ce32b5a4dec6`, and that merge
commit's `CI`, `CodeQL`, and `actionlint` workflows completed successfully.
This is the current accepted adapter baseline.

At that head, the non-mutating arming preflight remains fail-closed before SSM
resolution or approval consumption:

```bash
cargo run --locked --bin bolt-v2 -- \
  operator-artifacts preflight-live-submit-arming \
  --config config/root.toml \
  --client-key hyperliquid_perps
```

Expected current result:

```text
clients.hyperliquid_perps is not configured for live-submit arming preflight
```

Current tracked config is still unarmed for Hyperliquid live production execution. PR #809 (2026-06-18) adds the structural execution client this gate's "next live-arming slice" list anticipated, but leaves it unarmed (the `hyperliquid_perps` client key and bare preflight command above are the pre-#809 baseline; the current client key is `hyperliquid_execution`, preflight/generation now require `--product-surface <surface>`, and the CLI subcommand was renamed from `operator-artifacts` to `provider-artifacts` — so the `operator-artifacts preflight-live-submit-arming` block above is a frozen pre-#809 transcript and is not runnable at the current head):

- `config/root.toml` now declares one `[clients.hyperliquid_execution]` client (all four product surfaces, per-surface `[clients.hyperliquid_execution.execution.live_submit.<surface>]` blocks), but every `product_proof_artifact_sha256` is an all-zero fail-closed sentinel and no approval artifact is consumed, so the client cannot arm.
- `config/strategies/binary_oracle_<asset>.toml` still routes execution through `polymarket_main`.
- Metadata-only SSM parameter discovery found Bolt-owned Binance, Polymarket,
  and Chainlink parameter names in `eu-west-1`; it found no Hyperliquid or
  abbreviated `hl` parameter names in `eu-west-1` or `ap-northeast-2`.
- No Hyperliquid product-submit proof artifact, live-submit approval artifact, Hyperliquid no-submit report, or live canary approval-consumption proof is tracked.

The next live-arming slice must start from current `main` and provide, in one
operator-reviewed packet:

- a TOML-owned Hyperliquid execution client under `[clients.<id>]`;
- SSM parameter names, visible in the configured `[aws].region`, for the
  Hyperliquid signer/account material required by that execution mode;
- a product-compatible strategy target and `execution_client_id` pointing to
  the Hyperliquid client;
- current product-submit proof artifact references for order, fill, rounding,
  fee, and HIP-4 settlement proof when applicable;
- the generated Hyperliquid product-submit proof artifact path and sha256;
- a TOML-bound one-time live-submit approval id, approval artifact path, byte
  caps, max order count, and max order notional;
- exact-head final-packet/no-submit evidence; and
- renewed explicit operator approval before any live canary attempt, approval
  consumption, or submit-capable live run.

Until those exist, the merged code is production-grade execution
infrastructure only; it is not production live execution.

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
- `27046f0d` prior MVP spot, HIP-3, and HIP-4 missing-proof rejection before approval-gated enablement
- `45ffdde4` latency-profile ops metadata and artifact export
- follow-up slice: spot, HIP-3, and HIP-4 NT adapter mapping gated by consumed surface-bound approval
- current slice update: NT Hyperliquid market-data adapter mapping through `[data]`
- current slice update: production live-node Hyperliquid approval artifact loading and persisted consumption
- current slice update: consumed Hyperliquid approval order limits tighten shared submit-admission caps
- current slice update: Hyperliquid provider advertises `updown` market-family support for HIP-4 outcome routing gate compatibility
- current slice update: Hyperliquid rejects `updown` targets on non-HIP-4 product surfaces before execution mapping can open
- current slice update: Hyperliquid provider advertises `hyperliquid_instrument` for static/direct Hyperliquid instrument route identity
- current slice update: static Hyperliquid target product surfaces fail closed when they do not match any of the execution client's configured/approved product surfaces
- current slice update: production Hyperliquid approval loading rejects static target surface mismatches before spending one-time approval artifacts
- current slice update: proof-policy-only trade transport drops unrelated execution clients before provider approval loading
- current slice update: Hyperliquid provider egress model accounts the official `userFees` request weight before live-submit validation
- current slice update: operator CLI can generate configured Hyperliquid live-submit approval artifacts through the provider binding
- current slice update: Hyperliquid fee-provider construction resolves through the provider binding and warms from NT `userFees.userCrossRate`
- current slice update: Hyperliquid live-submit approval artifacts require configured product-submit proof path and sha256 bindings
- current slice update: Hyperliquid product-submit proof artifacts use a TOML-owned byte cap separate from the live-submit approval artifact byte cap
- `26bd6ea5` Hyperliquid product-submit proof artifact writer and schema validation
- `3b543fae` provider-neutral product-proof CLI routed through `ProviderBinding`
- `7e993673` provider binding test initializer fix after full test compile
- current slice update: provider-neutral `provider-artifacts generate-product-submit-proof` writes Hyperliquid product-proof artifacts through the provider binding

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
- `cargo test --locked --test bolt_v3_cli product_submit_proof` - PASS, 2 tests
- `cargo test --locked --test hyperliquid_live_submit_artifact` - PASS, 17 tests
- `CARGO_TARGET_DIR=/private/tmp/bolt-v2-hyperliquid-target cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_` - PASS, 13 tests
- `cargo test --locked --lib live_node_invalid_product_submit_proof_schema_does_not_spend_hyperliquid_approval_artifact` - PASS, 1 test
- `cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_hip4_accepts_updown_market_family_target_after_consumed_approval` - PASS, 1 test
- `cargo test --locked --test bolt_v3_instrument_filters market_identity_plan_accepts_hyperliquid_static_instrument_target` - PASS, 1 test
- `cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_standard_perps_accepts_static_instrument_target_after_consumed_approval` - PASS, 1 test
- `cargo test --locked --test bolt_v3_adapter_mapping hyperliquid_static_instrument_target_surface_must_match_execution_surface` - PASS, 1 test
- `cargo test --locked --lib live_node_static_target_surface_mismatch_does_not_spend_hyperliquid_approval_artifact` - PASS, 1 test
- `cargo test --locked --lib trade_transport_config_keeps_only_proof_policy_client_without_strategies` - PASS, 1 test
- `cargo test --locked --test bolt_v3_provider_binding provider_binding_builds_hyperliquid_fee_provider_that_fails_closed_without_fee_proof` - PASS, 1 test
- `CARGO_TARGET_DIR=/private/tmp/bolt-v2-hyperliquid-target cargo test --locked --lib live_node_adapter_mapping_consumes_hyperliquid_live_submit_approval_artifact` - PASS, 1 test
- `CARGO_TARGET_DIR=/private/tmp/bolt-v2-hyperliquid-target cargo test --locked --test bolt_v3_submit_admission live_submit_approval_limits_tighten_canary_caps_before_nt_submit` - PASS, 1 test
- `just source-fence` - PASS

Current live-submit state:
- Hyperliquid `[data]` maps to NT `HyperliquidDataClientFactory` with explicit TOML-owned endpoints, timeouts, refresh cadence, environment, and transport backend.
- Standard perps, spot, HIP-3, and HIP-4 map to the NT Hyperliquid execution adapter at the mapper boundary only with a consumed approval artifact for the exact configured product surface.
- Production trade transport scoping keeps only active strategy-bound clients or the enabled canary proof-policy execution client before provider approval loading, so unrelated execution clients cannot spend approval artifacts in proof-only runs.
- Production `build_bolt_v3_live_node` consumes Hyperliquid approval artifacts through the provider binding, persists `used_at`, and applies consumed approval order limits in shared submit admission; no-submit and all-configured mapping paths do not spend approvals.
- Product-submit proof files and live-submit approval files are read under separate TOML-owned byte caps before one-time approval consumption; the product proof must schema-bind the same provider id, product surface, TOML checksum, and required proof references before `used_at` is written.
- Static/direct target surface mismatches fail before production approval consumption and leave the approval artifact `used_at` unset.
- `provider-artifacts generate-live-submit-approval` writes the configured Hyperliquid artifact path from TOML plus resolved SSM secrets, binding base SHA, config checksum, signer fingerprint, product surface, limits, product-submit proof path/hash, and expiry without raw secret CLI inputs.
- `provider-artifacts generate-product-submit-proof` routes through `ProviderBinding`, accepts only provider/config identity and proof-reference paths/checksums, and writes the Hyperliquid product-proof schema without raw secret inputs.
- A consumed approval for one surface cannot authorize a different surface.
- HIP-4 requires positive `outcome_settlement_poll_secs` in TOML before mapper handoff and can pass the shared `updown` market-family execution-client routing gate.
- Static/direct Hyperliquid instrument targets can pass the shared execution-client routing gate through `hyperliquid_instrument` only when their target product surface matches one of the execution client's configured and approved product surfaces (a single live node arms at most one surface per execution client); binary rotating-market selection remains fail-closed for that family, and canary proof sizing constraints are TOML-owned on the target.
- `updown` targets can pass Hyperliquid route validation only for HIP-4 outcomes execution clients; non-HIP-4 surfaces reject that family before NT execution mapping opens.
- `provider-artifacts collect-canary-proof-artifacts` is wired for Hyperliquid static instrument targets and writes a no-resolution gate session plus candidate/order-intent artifacts bound to the configured Hyperliquid execution client.
- Hyperliquid REST egress is modeled at 1200 weight/min and derates order-command validation by the official 20-weight `userFees` policy while the pinned NT adapter still reports 2.
- Hyperliquid execution strategies resolve a provider-owned fee provider that returns no fee bound before warmup, then caches the account `userFees.userCrossRate` taker fee bound in basis points after successful warmup.
- Latency profile is TOML-owned ops metadata only; it exports an artifact and cannot bypass submit approval gates.
- Hyperliquid-specific no-submit readiness artifacts are not part of this slice.
