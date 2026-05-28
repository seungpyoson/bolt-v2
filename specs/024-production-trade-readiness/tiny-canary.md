# T044 Tiny-Capital Canary

Status: not completed.

T044 remains gated on renewed explicit operator approval because it is a live tiny-capital canary. The canary may submit at most one live order under the configured bounds:

- `max_live_order_count = 1`
- `max_notional_per_order = "1.00"`

## T043B Selected Trade-Path Gate

Status: open.

The selected tiny-capital path is gated separately from the all-venue T043A matrix. The current T043A matrix records the configured Polymarket data row as production-usable for same-run metadata-selected targets, while six unrelated data-only venue rows remain fail-closed. Those unrelated rows gate the PR's broad multi-venue data-client production-usability claim; they do not by themselves block a Polymarket-only tiny-capital canary.

T043B must be recorded against the current head before T044 can run:

- The selected data row in the T043A matrix is production-usable for the configured canary path.
- The final packet and no-submit readiness evidence are regenerated and verified at the current head and root TOML.
- The pre-consumption live-canary gate rejects stale source-owned strategy input before approval consumption.
- The live canary remains capped by the configured max-order and max-notional bounds above.
- The operator harness keeps approval consumption after entry validation, binds result paths to the runtime capture spool, and requires submit-admission, venue-order-state, restart-reconciliation, optional cancel, and post-run-hygiene proofs before writing live-canary completion evidence.
- Any blocked attempt must produce blocked-before-submit evidence without live order refs; any successful attempt must produce the live proof refs required by `Phase8CanaryEvidence::live_canary_proof`.

T043B is not live execution. It is the last non-live selected-path proof before requesting renewed T044 approval.

Current-head local T043B verification at `e20e0274a94dac954aa4b36c316170d793963f65`:

- `cargo test --locked --test bolt_v3_live_canary_gate pre_consumption_gate_rejects_stale_source_owned_strategy_input_before_approval -- --nocapture`: passed, 1 passed, 0 failed.
- `cargo test --locked --test bolt_v3_tiny_canary_operator phase8_operator_harness -- --nocapture`: passed, 7 passed, 0 failed, 1 ignored. The ignored test is the live operator harness entrypoint and remains excluded from normal local test runs; the passing sibling tests verify its source shape, approval-consumption order, runtime spool binding, submit-admission/live-proof binding, and post-run proof wait contract.

T043B remains open because the selected-path topology fix below must be committed before the final current-head operator packet can be regenerated and verified. Any commit after packet generation changes `HEAD` and intentionally makes the packet stale.

Current-head packet refresh attempt after the local gate checks:

- Temp root: `/private/tmp/bolt-v2-t043b-e20e0274/live.local.toml`.
- The ignored live TOML copy was stale for the current readiness-probe schema; the temp copy required explicit `market_data_kind`, `book_type`, and `allow_metadata_target_sampling` fields before it could pass config validation. The real ignored live TOML must be refreshed before any live canary attempt.
- `operator-artifacts generate-base-static` passed and wrote `ssm-manifest.json` sha256 `501002f491b4aad097cad6524a439ae6968d751e822d278cdb5e0816f7597c22`, `financial-envelope.json` sha256 `076b7ce1374abf89ed553adef9064f7c6c410f485484dcfcf6624d6b776afd33`, and `approval-nonce.json` sha256 `b9aa4bd1b6df6d91266b96916ebe989475310ce90bba5dc13131f010784da773`.
- `operator-artifacts generate-abort-plan-from-source-collectors` passed with current source files and wrote `/private/tmp/bolt-v2-t043b-e20e0274/abort-plan.json`, sha256 `8bca1bddfc927973a3819dfc5bb211795034fdf560b9ce1561a93c28aae69182`.
- The refreshed packet assembled: operator-evidence JSON sha256 `dc3f5d8f6f5ca9083724eed3759184c5b0d4f1a91bf587cdc6b8024e9e073923`, temp root TOML sha256 `5728517f39bc232de9a5f28571376c00c521cfb697fee108a4569c1afab0f1f6`, static manifest sha256 `7476748fda6086587319c28f14116e66a4854ac84e82d9155779cf0be6bd8309`, approval envelope sha256 `39735123203cc6b88d17154ae7924315140ca0729f0115095bbebee8fa57bc90`, and operator packet sha256 `a29d6f1716c4bcabe1aeed4e0bc5142a0265a13b124142b97664032fa345a727`.
- `operator-artifacts verify-final --verification-stage pre-run` failed closed with `strategy_input_replay.market_selection_source.decision_evidence_path is invalid or not bound to decision evidence`.
- Attempting to regenerate `market-selection-source.json` from the current worktree decision-evidence JSONL failed closed with `missing source-bound market selection from NT instrument facts`.

No no-submit run, live runner, approval consumption, order submit/cancel, transfer, on-chain mutation, or trade was executed in this refresh attempt. T043B now requires a fresh source-bound decision chain for the current root before the packet/no-submit refresh can pass.

Selected-path topology repair:

- Root cause: the no-submit and live build paths registered every client in the loaded root TOML. NT requires every registered client to connect before the node reaches `Running`, so unrelated data-only clients can mask or block the selected tiny-capital trade path.
- Fix direction: derive the selected trade transport scope from strategy-owned TOML bindings: each loaded strategy's `execution_client_id` plus every configured `reference_data.*.data_client_id`. The broad T043A clients remain configured in root TOML and remain available to source-owned per-client probes; they are not registered into the selected trade runner unless the strategy references them.
- Runtime evidence from the same temp root that previously exposed the blocker: `cargo run --locked --bin bolt-v2 -- no-submit-readiness --config /private/tmp/bolt-v2-t043b-e20e0274/live.local.toml` exited 0 after registering only `DataClient-polymarket_main` and `ExecutionClient-polymarket_main`, reaching `All engine clients connected`, starting the no-submit probe actor, stopping via handle, and writing a readiness report.
- Report evidence: `var/bolt-v3-live/reports/no-submit-readiness.json` schema `bolt-v3.no-submit-readiness.v2`, executable identity `d40c097290e9620a90632532842569eaab645a555db1c41e0bb9a82d5cb71dc9`, config bundle checksum `2ea35975a274f175a5bc17d4c6d7f8811b18b950ef385ba97cb788effed06978`, generated at Unix seconds `1780009626`, with all seven stages satisfied: `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_connect`, `reference_readiness`, `controlled_disconnect`, and `report_write`.
- Regression evidence: `cargo test --locked --lib trade_transport_config_keeps_only_strategy_bound_clients -- --nocapture` passed; `cargo test --locked --lib no_submit_transport -- --nocapture` passed; `cargo test --locked --test bolt_v3_no_submit_readiness -- --nocapture` passed, 34 passed; `cargo test --locked --test bolt_v3_live_canary_gate pre_consumption_gate_rejects_stale_source_owned_strategy_input_before_approval -- --nocapture` passed.

No live runner, approval consumption, order submit/cancel, transfer, on-chain mutation, or trade was executed in the topology repair. The next T043B step after committing the repair is to regenerate the final operator packet at the new head and rerun no-submit against that exact head.

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
