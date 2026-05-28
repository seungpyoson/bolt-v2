# T044 Tiny-Capital Canary

Status: not completed.

T044 remains gated on renewed explicit operator approval because it is a live tiny-capital canary. The canary may submit at most one live order under the configured bounds:

- `max_live_order_count = 1`
- `max_notional_per_order = "1.00"`

## Current-Head Non-Live Preflight

Preflight head: `c6fe228c5aa4807d704bdae904f8695220b64dd5`

The previously configured ignored `config/live.local.toml` operator packet still binds reviewed code head `8b95eca9c2f410ff462954cff90c4734d01593cb`, so the stale packet correctly fails current-head pre-run verification:

- Command: `cargo run --locked --bin bolt-v2 -- operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t042-review-repair-final-refresh/operator-evidence-packet-8b95eca9.json --verification-stage pre-run`
- Result: failed closed with `[live_canary.operator_evidence].head_sha does not match build head_sha`.

A current-head packet was assembled and verified on a temporary config copy under `/private/tmp/bolt-v2-t044-preflight-c6fe228c` without mutating the ignored live TOML:

- Copied root config: `/private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml`
- Copied relative strategy config: `/private/tmp/bolt-v2-t044-preflight-c6fe228c/strategies/binary_oracle.local.toml`
- `operator-artifacts generate-base-static --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --output-dir /private/tmp/bolt-v2-t044-preflight-c6fe228c/base-static --strategy-instance-id bitcoin_updown_main`: passed.
- `operator-artifacts generate-operator-evidence-json`: wrote `/private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-c6fe228c.json`, sha256 `a47c59e5f49a00fa203360b6a2e7cb613363d3a35dc488a71f048fce6d7c35d1`.
- `operator-artifacts update-operator-evidence-toml --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --operator-evidence-json /private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-c6fe228c.json --max-operator-evidence-json-bytes 65536`: passed; temp root TOML sha256 `97264654371a1dd9467bc74ea42e58ac443b6579974809b1b032caf702a18012`.
- `operator-artifacts write-manifest-from-operator-evidence --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --output /private/tmp/bolt-v2-t044-preflight-c6fe228c/static-artifacts-manifest-c6fe228c.json`: passed; manifest sha256 `a4dc04d7f8dbd5a7d3210a7a5c8ea5dae78bb45227785047adef1ea1358c782d`.
- `operator-artifacts assemble-final --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --static-manifest /private/tmp/bolt-v2-t044-preflight-c6fe228c/static-artifacts-manifest-c6fe228c.json --operator-packet /private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-packet-c6fe228c.json`: passed.
  - approval envelope sha256: `ef690d8e1834b30f30fda3b9dc187ce9704db1a56a86e77a7cf1c60797f85201`
  - operator packet sha256: `32261a754f30967b84b66edad60cf710c3bcaac40d6e72e8400384dc59b4527d`
  - static manifest sha256: `a4dc04d7f8dbd5a7d3210a7a5c8ea5dae78bb45227785047adef1ea1358c782d`
- `operator-artifacts verify-final --config /private/tmp/bolt-v2-t044-preflight-c6fe228c/live.local.toml --operator-packet /private/tmp/bolt-v2-t044-preflight-c6fe228c/operator-evidence-packet-c6fe228c.json --verification-stage pre-run`: passed and verified the hashes above.

Scope and side effects: this was non-live artifact generation and verification only. It did not run `bolt-v2 run`, submit/cancel orders, transfer funds, mutate on-chain state, mutate CLOB allowance/cache state, display secrets, or patch the ignored real `config/live.local.toml`. The temporary approval window used for preflight is not reusable for the live canary; the real ignored live TOML must be refreshed after explicit operator approval and immediately re-verified before T044 execution.

## Current-Head Live Attempt: Failed Closed Before Submit

Attempt head: `84ebcf89cf80927d1449d8ba3933e4d314ded45b`

Artifact root: `/private/tmp/bolt-v2-t044-fee-fix-84ebcf89/one-shot-1779949800`

Result: not T044 completion evidence.

- Pre-run packet generation and no-submit readiness completed for the attempt root.
- The live runner consumed the operator approval and wrote `live-run/approval-consumed.json`.
- No `live-run/canary-evidence.json`, `live-run/nt-submit-event.json`, `live-run/venue-order-state.json`, `live-run/restart-reconciliation.json`, or `live-run/post-run-hygiene.json` was produced.
- The live log repeatedly reported `entry_gate_blocked` with `IntervalOpenMissing`, `WarmupIncomplete`, and `FeesNotReady`; later entries also included `ActiveBookNotPriced`.
- The configured source packet selected `market_selection_outcome = "current"` for slug `btc-updown-5m-1779949800`. The runner later evaluated a different active market without a matching source-owned `price_to_beat`, so interval-open/warmup stayed fail-closed.
- The local runner was stopped with SIGINT after no admitted order or submit artifact existed. NT disconnected the Polymarket data and execution clients cleanly and returned exit code 0.

Scope and side effects: this live attempt connected the configured Polymarket data and execution clients and consumed the one-time operator approval proof. It did not produce a submitted order artifact, venue order state artifact, canary evidence, cancel proof, transfer proof, on-chain mutation proof, or post-run hygiene proof. T044 remains open.

## Current-Head Repair Before Retry: Stale Strategy Input Approval Guard

Root cause evidence from the failed attempt:

- `strategy-input.json` carried source-owned `reference_quote_ts_event = 1779949810000` and `realized_volatility = "0.1314634586490257"`.
- The live runner entered several minutes later, so the runtime seed was already outside the configured volatility bridge window. Initial entry evaluations showed the source price was present but `realized_vol=None`; after market rollover, the static source-bound market no longer matched and `interval_open=None`.
- The pre-consumption live-canary gate accepted the stale `strategy_input_evidence` before creating `approval-consumed.json`, so an approval could be burned before the runtime could use source-owned reference evidence.

Repair:

- `check_bolt_v3_live_canary_pre_consumption_gate` now validates source-owned `strategy_input_evidence.reference_quote_ts_event` against TOML-owned `[live_canary].reference_quote_max_age_seconds` before approval consumption. This keeps the guard market/provider agnostic and uses the existing source-owned `decision_reference` path instead of reviving a runtime Chainlink client.
- Regression test first failed because stale source-owned `strategy_input` passed pre-consumption gate validation, then passed after the guard was added.

Verification:

- `cargo test --test bolt_v3_live_canary_gate pre_consumption_gate_rejects_stale_source_owned_strategy_input_before_approval -- --nocapture`: passed.
- `cargo test --test bolt_v3_live_canary_gate -- --nocapture`: 71 passed.
- `cargo test --test bolt_v3_tiny_canary_operator phase8_preflight_accepts_valid_gate_inputs_before_approval_consumption -- --nocapture`: passed.
- `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`: 31 passed, 1 ignored.
- `cargo test --test bolt_v3_strategy_registration bolt_v3_registration_context_includes_operator_readiness_gate_session -- --nocapture`: passed.

Scope and side effects: this was local code/test verification only. It did not connect to a venue, read SSM secrets, run no-submit, submit/cancel orders, transfer funds, mutate on-chain state, or consume another live approval. T044 remains open and needs a fresh source packet, pre-run verification, no-submit readiness, and tiny-capital canary retry from the new head.
