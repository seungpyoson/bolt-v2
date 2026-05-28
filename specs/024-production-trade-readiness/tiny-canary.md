# T044 Tiny-Capital Canary

Status: not completed.

T044 remains gated on renewed explicit operator approval because it is a live tiny-capital canary. The canary may submit at most one live order under the configured bounds:

- `max_live_order_count = 1`
- `max_notional_per_order = "1.00"`

## Latest Non-Live Preflight

Preflight head: `4302d2498eaefab25677cfaead643ff4b4c5de08`

The configured ignored `config/live.local.toml` operator packet at this point still bound stale code head `655baed29dac51001fd0c604067d7c178a558291`, so the stale packet correctly failed build-head pre-run verification:

- Command: `cargo run --locked --bin bolt-v2 -- operator-artifacts verify-final --config config/live.local.toml --operator-packet /private/tmp/bolt-v2-t044-refresh-1779956700/operator-evidence-packet.json --verification-stage pre-run`
- Result: failed closed with `[live_canary.operator_evidence].head_sha does not match build head_sha`.

A packet for preflight head `4302d2498eaefab25677cfaead643ff4b4c5de08` was assembled and verified on a temporary config copy under `/private/tmp/bolt-v2-t044-preflight-4302d249` without mutating the ignored live TOML:

- Copied root config: `/private/tmp/bolt-v2-t044-preflight-4302d249/live.local.toml`
- Copied relative strategy config: `/private/tmp/bolt-v2-t044-preflight-4302d249/strategies/binary_oracle.local.toml`
- `operator-artifacts generate-base-static --config /private/tmp/bolt-v2-t044-preflight-4302d249/live.local.toml --output-dir /private/tmp/bolt-v2-t044-preflight-4302d249/base-static --strategy-instance-id bitcoin_updown_main`: passed.
- `operator-artifacts generate-operator-evidence-json`: wrote `/private/tmp/bolt-v2-t044-preflight-4302d249/operator-evidence-4302d249.json`, sha256 `fbf28de6d379cc229b722ef293ffe4ce070c1fd1916a02482d7213a92ada0456`.
- `operator-artifacts update-operator-evidence-toml --config /private/tmp/bolt-v2-t044-preflight-4302d249/live.local.toml --operator-evidence-json /private/tmp/bolt-v2-t044-preflight-4302d249/operator-evidence-4302d249.json --max-operator-evidence-json-bytes 65536`: passed; temp root TOML sha256 `01c3f1e35653d4ab064e761db53016a93a37578aa48798fcf0e5b90b827011e9`.
- `operator-artifacts write-manifest-from-operator-evidence --config /private/tmp/bolt-v2-t044-preflight-4302d249/live.local.toml --output /private/tmp/bolt-v2-t044-preflight-4302d249/static-artifacts-manifest-4302d249.json`: passed; manifest sha256 `4ef5fc9bdd52f7d6f458488306b226c6d0b8428b149f1f7de248a4d614103600`.
- `operator-artifacts assemble-final --config /private/tmp/bolt-v2-t044-preflight-4302d249/live.local.toml --static-manifest /private/tmp/bolt-v2-t044-preflight-4302d249/static-artifacts-manifest-4302d249.json --operator-packet /private/tmp/bolt-v2-t044-preflight-4302d249/operator-evidence-packet-4302d249.json`: passed.
  - approval envelope sha256: `7d367934f8726f57d4e34140a055b85eda8e2df05ad0ada9e369d8a9d3114152`
  - operator packet sha256: `54aac09399434dc14668a060ee8f2fc8c0783229b8e1f32136e809bb81f4016f`
  - static manifest sha256: `4ef5fc9bdd52f7d6f458488306b226c6d0b8428b149f1f7de248a4d614103600`
- `operator-artifacts verify-final --config /private/tmp/bolt-v2-t044-preflight-4302d249/live.local.toml --operator-packet /private/tmp/bolt-v2-t044-preflight-4302d249/operator-evidence-packet-4302d249.json --verification-stage pre-run`: passed and verified the hashes above.

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
