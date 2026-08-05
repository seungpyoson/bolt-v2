# T117 Order-ID Binding Plan

> **Historical implementation record — not an active plan.** Do not execute
> commands or tasks from this file. Current `main`, `AGENTS.md`, and tracked
> issues are authoritative.

## Pre-Change Evidence

- `src/strategies/binary_oracle_edge_taker.rs:50` exposes `order_id_tag`, `use_uuid_client_order_ids`, and `use_hyphens_in_client_order_ids`, but no fixed canary `client_order_id` value.
- `config/strategies/binary_oracle.example.toml:4` and `config/strategies/binary_oracle.example.toml:6` configure the order-id tag and UUID client-order-id mode.
- `src/strategies/binary_oracle_edge_taker.rs:3519` and `src/strategies/binary_oracle_edge_taker.rs:3721` generate exit and entry client order ids inside the strategy immediately before order construction.
- `src/strategies/binary_oracle_edge_taker.rs:4715` passes the generated client order id into the NT order factory as `Some(client_order_id)`.
- NT `crates/common/src/generators/client_order_id.rs:187` uses `UUID4::new()` when UUID mode is enabled; the id is therefore not known when the operator packet is assembled.
- NT order state starts with no venue order id and sets one from order accepted or updated events after venue interaction.
- The pinned Polymarket adapter can compute an expected venue order id after signed CLOB order construction, but signed order construction includes random salt; it is not known from static operator-packet inputs.
- Before this slice, `src/bolt_v3_config.rs`, `src/bolt_v3_live_canary_gate.rs`, `src/bolt_v3_tiny_canary_evidence.rs`, and `tests/bolt_v3_tiny_canary_operator.rs` required `client_order_id_hash` and `venue_order_id_hash` before the live runner could enter.

## Proposed Contract

Pre-run operator evidence must bind only values knowable before submit:

- exact head and root TOML hash
- no-submit report, executable identity, and config bundle checksum
- SSM manifest, strategy input, financial envelope, pre-run state, abort plan, approval nonce, and approval envelope hashes
- approval window and one-shot approval-consumption proof
- canary output paths and submit caps

Pre-run operator evidence must not require true `client_order_id_hash` or true `venue_order_id_hash` unless a later design adds a config-owned deterministic id source and tests prove that source reaches the submitted order.

Post-run evidence must bind actual order ids:

- decision evidence and NT submit event must agree on actual `client_order_id_hash`
- venue order state must bind actual `client_order_id_hash` and actual `venue_order_id_hash`
- cancel, restart reconciliation, and post-run hygiene proofs must agree with the same actual ids when present
- final canary evidence must be rejected if post-run proofs disagree or omit the required actual ids

## TDD Slice

1. RED: add a live-canary gate test proving an operator-evidence packet without pre-run order-id hashes can pass the pre-consumption gate when all other evidence is valid.
2. GREEN: make `client_order_id_hash` and `venue_order_id_hash` non-required for pre-run operator evidence and approval-consumption validation.
3. RED: add a Phase 8 harness test proving approval-consumption writing no longer requires `BOLT_V3_PHASE8_CLIENT_ORDER_ID_HASH` or `BOLT_V3_PHASE8_VENUE_ORDER_ID_HASH`.
4. GREEN: remove those env requirements from the pre-run envelope and consumption writer.
5. RED: add post-run proof tests deriving actual order-id hashes from produced evidence and rejecting disagreement across venue state, cancel, restart reconciliation, and post-run hygiene.
6. GREEN: derive final live-order refs from post-run proof files instead of predeclared env hashes.
7. REFACTOR: remove stale schema/docs references that describe pre-run order-id hashes as required.

## Local Implementation Candidate

- `tests/config_parsing.rs::bolt_v3_operator_evidence_allows_unassigned_order_ids` proves `config/root.toml` omits pre-run order-id hashes while `[live_canary.operator_evidence]` still parses.
- `tests/bolt_v3_live_canary_gate.rs::live_canary_pre_consumption_gate_accepts_without_pre_run_order_id_hashes` proves the pre-consumption gate accepts a valid packet without approval consumption and without pre-run order-id hashes.
- `tests/bolt_v3_tiny_canary_preconditions.rs::operator_approval_envelope_consumes_time_bound_nonce_once` proves approval consumption still writes and the live gate accepts the proof without order-id hashes.
- `tests/bolt_v3_tiny_canary_operator.rs::phase8_operator_harness_binds_live_proof_to_runtime_admission_and_spool` proves the operator harness no longer references the pre-run order-id hash env vars and derives the final live-order refs from post-run proof files.
- Local verification before external review: `cargo test --test config_parsing`, `cargo test --test bolt_v3_live_canary_gate`, `cargo test --test bolt_v3_tiny_canary_preconditions`, `cargo test --test bolt_v3_tiny_canary_operator`, `cargo fmt --check`, and `git diff --check` passed.

## Non-Goals

- Do not execute T116.
- Do not submit, cancel, replace, transfer, or deploy.
- Do not add venue-specific hardcodes.
- Do not add env secret fallback or non-SSM credential source.
- Do not weaken max-order or max-notional submit caps.
