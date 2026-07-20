# Tasks: Production Trade Readiness

**Input**: `specs/024-production-trade-readiness/spec.md`, `specs/024-production-trade-readiness/plan.md`, and `specs/024-production-trade-readiness/evidence.md`
**Branch/PR**: `goal/024-production-trade-readiness`, PR #480
**Policy**: one readiness PR; no order-intent-layer work; no #466 decomposition-ledger work.

## Phase 1: Baseline And Task-List Approval

**Purpose**: Lock scope and approve this task list before production code work resumes.

- [x] T001 Record current git, PR, issue, Speckit, readiness-ledger, code, and T038-branch evidence in `specs/024-production-trade-readiness/evidence.md`.
- [x] T002 Update PR #480 body to reference `specs/024-production-trade-readiness/` as the active readiness task list, branch `goal/024-production-trade-readiness`, and to keep PR #479/#466 out of scope.
- [x] T003 Remove #466 verifier-characterization/decomposition files from PR #480; the merged PR history records the removed paths.
- [x] T004 Send `specs/024-production-trade-readiness/{spec.md,plan.md,tasks.md,evidence.md}` plus `AGENTS.md` policy context for task-list review only; record verdicts in `specs/024-production-trade-readiness/external-tasklist-review.md`.
- [x] T005 Resolve every blocking task-list review finding in `specs/024-production-trade-readiness/tasks.md` and record disposition in `specs/024-production-trade-readiness/external-tasklist-review.md`.

## Phase 2: T038 Branch And Existing Issue Hygiene

**Purpose**: Avoid blind ports and close/update issue state that current evidence already resolves.

- [x] T006 [P] Complete a targeted `t038-operator-config-snapshot` port audit in `specs/024-production-trade-readiness/t038-port-audit.md`, comparing each unique old branch behavior to current #480/main no-submit code and recording any exact missing patch.
- [x] T007 [P] Verify whether #409 PortfolioSnapshot acceptance criteria are satisfied by current source and tests; record close/update evidence in `specs/024-production-trade-readiness/issue-409-portfolio-snapshot.md`.
- [x] T008 [P] Update #385 evidence with the current distinction between historical T038 no-submit success and missing final-packet T131/T122 no-submit proof in `specs/024-production-trade-readiness/issue-385-no-submit.md`.

## Phase 3: User Story 2 - Real Decision Evidence

**Goal**: Close T124/T125 with real current-head runtime artifacts, not fixtures or static generation.

**Independent Test**: final-packet generation rejects missing real market-selection and strategy-input evidence, then accepts only current-head artifact paths and hashes bound by `[live_canary.operator_evidence]`.

- [x] T009 [US2] Add a RED final-packet test in `tests/bolt_v3_operator_artifacts.rs` proving fixture/static market-selection evidence cannot satisfy T124.
- [x] T010 [US2] Produce or wire the real current-head runtime market-selection artifact path in `src/bolt_v3_operator_artifacts.rs` without hardcoded venue, market, price, quantity, or timeout values.
- [x] T011 [US2] Add a RED final-packet test in `tests/bolt_v3_operator_artifacts.rs` proving missing real runtime JSONL strategy-input chain cannot satisfy T125.
- [x] T012 [US2] Produce or wire the real current-head runtime strategy-input evidence chain in `src/bolt_v3_operator_artifacts.rs` and `src/strategies/binary_oracle_edge_taker.rs`.
- [x] T013 [US2] Confirm T124/T125 bind through existing `[live_canary.operator_evidence]` `strategy_input_evidence_path`/`strategy_input_evidence_sha256` and `decision_evidence_path`; final approved `config/live.local.toml` values remain T037 with the full final packet.
- [x] T014 [US2] Run focused T124/T125 tests plus runtime-literal verification and record the evidence in `specs/024-production-trade-readiness/evidence.md`.

## Phase 4: User Story 3 - Pre-Run State Collectors

**Goal**: Close T126 by replacing caller-supplied proof gaps with source-owned collectors.

**Independent Test**: pre-run proof generation fails for each missing collector and passes only when every required pre-run source proof is present.

- [x] T015 [P] [US3] Add RED tests for venue account, open-orders, and positions collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T016 [US3] Implement venue account, open-orders, and positions source collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T017 [P] [US3] Add RED tests for funding and margin source collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T018 [US3] Implement funding and margin source collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T019 [P] [US3] Add RED tests for approved egress identity source collector in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T020 [US3] Implement approved egress identity source collector in `src/bolt_v3_operator_artifacts.rs`.
- [x] T021 [P] [US3] Add RED tests for CLOB V2 adapter signing, collateral accounting, and fee behavior collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T022 [US3] Implement CLOB V2 adapter signing, collateral accounting, and fee behavior collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T023 [US3] Bind all T126 collector outputs into source-owned pre-run-state artifact generation and CLI wiring; final `config/live.local.toml` `pre_run_state_path`/`pre_run_state_sha256` binding remains T037.
- [x] T024 [US3] Run focused T126 tests plus runtime-literal verification and record evidence in `specs/024-production-trade-readiness/evidence.md`.
- [x] T024A [US3] Add TDD repair proving venue-account state sources must match the configured execution client and target identity before zero-order/zero-position evidence can satisfy T126.
- [x] T024B [US3] Add TDD repair proving pre-run source collectors derive the expected price-to-beat source from TOML, not caller or CLI overrides.
- [x] T024C [US3] Add RED public-interface tests proving host-clock source materialization derives reference time from a TOML-owned execution-client provider endpoint and does not accept caller-supplied timestamps.
- [x] T024D [US3] Implement `operator-artifacts collect-pre-run-host-clock-source` so T036 can write `host-clock-source.json` from configured provider time with bounded output and no raw timestamp/stdout leakage.
- [x] T024F [US3] Add TDD-backed `operator-artifacts collect-pre-run-clob-v2-adapter-signing-source` so T036 can write the CLOB V2 adapter-signing source proof from the pinned NT signing source and a local ephemeral signature-recovery self-test.
- [x] T024G [US3] Add TDD-backed `operator-artifacts collect-pre-run-clob-v2-fee-behavior-source` so T036 can write the CLOB V2 fee-behavior source proof from pinned NT fee parser sources and a local deterministic NT fee-behavior self-test.
- [x] T024H [US3] Add TDD-backed `operator-artifacts collect-pre-run-egress-identity-source` so T036 can write the egress identity source proof from TOML-owned approved identity hash and TOML-owned observed probe source.
- [x] T024I [US3] Add TDD-backed `operator-artifacts collect-pre-run-clob-v2-collateral-accounting-source` so T036 can write the CLOB V2 collateral accounting source proof from source-owned pUSD balance/allowance evidence, TOML-owned `max_notional_per_order`, and approved fee-rate source hash.
- [x] T024E [US3] Replace remaining caller-supplied pre-run source inputs for venue account/open orders/positions and funding/margin with real source-owned materializer commands before T036 final-packet assembly.

## Phase 5: User Story 4 - Abort-Plan Collectors

**Goal**: Close T127 by replacing caller-supplied abort proof gaps with source-owned collectors.

**Independent Test**: abort-plan proof generation fails for each missing abort collector and passes only when every required abort proof is present.

- [x] T025 [P] [US4] Add RED tests for NT accepted and venue pending abort collectors in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T026 [US4] Implement NT accepted and venue pending abort collectors in `src/bolt_v3_operator_artifacts.rs`.
- [x] T027 [P] [US4] Add RED tests for partial-fill abort collector in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T028 [US4] Implement partial-fill abort collector in `src/bolt_v3_operator_artifacts.rs`.
- [x] T029 [P] [US4] Add RED tests for network-partition abort collector in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T030 [US4] Implement network-partition abort collector in `src/bolt_v3_operator_artifacts.rs`.
- [x] T031 [P] [US4] Add RED tests for panic-gate and service-policy collector in `tests/bolt_v3_operator_artifacts.rs`.
- [x] T032 [US4] Implement panic-gate and service-policy collector in `src/bolt_v3_operator_artifacts.rs`.
- [x] T033 [US4] Bind all T127 collector outputs into source-owned abort-plan artifact generation and CLI wiring; final `config/live.local.toml` `abort_plan_path`/`abort_plan_sha256` binding remains T037.
- [x] T034 [US4] Run focused T127 tests plus runtime-literal verification and record evidence in `specs/024-production-trade-readiness/evidence.md`.
- [x] T034A [US4] Add TDD repair proving final-packet verification rejects abort-plan artifacts built from synthetic/caller-supplied proof hashes instead of collector-derived source proofs.

## Phase 6: User Story 5 - Final Packet

**Goal**: Close T128 with a blocker-free final packet that consumes T124-T127 real artifacts.

**Independent Test**: `operator-artifacts verify-final` passes only for the exact root TOML and final packet with matching artifact hashes.

- [x] T035 [US5] Add a RED end-to-end final-packet test in `tests/bolt_v3_operator_artifacts.rs` that fails until T124-T127 artifacts and `[live_canary.operator_evidence]` exist together.
- [x] T035A [US5] Add a non-live CLI hash step for computing the canonical `approval_envelope_sha256` required before `operator-artifacts assemble-final` can write the approval envelope and operator packet.
- [x] T035B [US5] Add a source-owned non-live entry-decision evidence generator so T036 can write the configured runtime decision-evidence JSONL without AWS/SSM, no-submit, live venue, or trading side effects.
- [x] T035C [US5] Add a pre-run `operator-artifacts verify-final` verification stage so T038 can verify the final packet before T043/T044 live/no-submit result evidence exists, while preserving strict post-run evidence verification.
- [x] T035D [US5] Add a non-live `operator-artifacts update-operator-evidence-toml` command so T037 can patch only `[live_canary.operator_evidence]` from bounded JSON without printing approval IDs, artifact paths, or secrets.
- [x] T035E [US5] Harden `operator-artifacts update-operator-evidence-toml` so T037 cannot patch `[live_canary.operator_evidence]` until the configured static artifact paths exist and match their hashes.
- [x] T035F [US5] Add a non-live `operator-artifacts generate-operator-evidence-json` command so T037 operator-evidence JSON is generated from materialized artifact paths and canonical approval-envelope hash computation instead of hand assembly.
- [x] T036A [US5] Add a source-input collector that writes replayable `entry-decision-source.json` and `instrument-source.json` from bounded source proofs, requiring TOML-bound source-report provenance for `price_to_beat`, fee-rate proof, and two-sided selected-market books before T036 assembly.
- [x] T036B [US5] Add a non-live `operator-artifacts collect-entry-decision-proof-sources` command that materializes the four entry-decision proof source files from bounded operator-approved source inputs before T036 source-input collection.
- [x] T036C [US5] Add a non-live `operator-artifacts generate-base-static` command that writes only unblocked base static artifacts (`ssm-manifest.json`, `financial-envelope.json`, `approval-nonce.json`) without blocker-manifest semantics before T036/T037 final-packet assembly.
- [x] T036D [US5] Keep price-to-beat feed configuration fail-closed until `config/live.local.toml` points at an operator-approved real Chainlink feed id instead of the shipped placeholder strategy config.
- [x] T036E [US5] Move egress-identity pre-run source inputs to top-level `[live_canary]` fields so T036 can materialize `egress-identity-source.json` before T037 patches `[live_canary.operator_evidence]`.
- [x] T036F [US5] Capture the real EC2/EIP observed egress identity file and approved sha256 in ignored `config/live.local.toml`, then rerun `collect-pre-run-egress-identity-source` without inventing local-machine evidence.
- [x] T036G1 [US5] Align venue-account flatness with pinned NT Polymarket reconciliation by ignoring zero/dust Data API rows through NT's `DUST_POSITION_THRESHOLD` while preserving active-position fail-closed behavior.
- [x] T036G [US5] Resolve the current configured venue-account `preexisting_position_absent` blocker by proving the approved account is flat or switching `config/live.local.toml` to an operator-approved flat account before `pre-run-state.json` assembly.
- [x] T036G2 [US5] Inventory market/venue/account-agnostic external-provider-snapshot hard-stop gates in `specs/024-production-trade-readiness/provider-snapshot-hard-stop-inventory.md` and classify immediate readiness fixes separately from final-packet/no-submit/tiny-canary gates.
- [x] T036G3 [US5] Add TDD-backed shared confirmation/fail-closed coverage for provider-snapshot readiness gates: transient open orders/positions and low CLOB balance/allowance may clear after configured confirmation, but persistent blocking state or confirmation fetch failure still hard-blocks.
- [x] T036H0 [US5] Send the corrected resolution/reference gate ownership model to Claude, Gemini, DeepSeek, GLM, and Grok for adversarial review; record REQUEST_CHANGES dispositions and accepted code evidence in `specs/024-production-trade-readiness/external-gate-architecture-review.md`.
- [x] T036H0A [US5] Send the revised T036H plan to Claude, Gemini, DeepSeek, GLM, and Grok for adversarial review; record REQUEST_CHANGES dispositions and accepted plan gaps in `specs/024-production-trade-readiness/external-gate-architecture-review.md`.
- [x] T036H0B [US5] Record the concrete end-to-end gate dataflow contract in `specs/024-production-trade-readiness/gate-dataflow-contract.md`, including config, target subscription, archetype, selected market, provider evidence, entry readiness, decision evidence, tiny-canary evidence, CLI, registration, runtime, and replay boundaries.
- [x] T036H0C [US5] Send `specs/024-production-trade-readiness/gate-dataflow-contract.md`, `specs/024-production-trade-readiness/plan.md`, `specs/024-production-trade-readiness/tasks.md`, and the cited source paths to Claude, Gemini, DeepSeek, GLM, and Grok for contract-only adversarial review; record verdicts and dispositions in `specs/024-production-trade-readiness/external-gate-architecture-review.md`.
- [x] T036H0D [US5] Clean up the official T036H task/contract language so the readiness gate is market, venue, and provider agnostic: no Binance/BTCUSDT canonical reference, no Polymarket-only selected-market identity, no price-only `resolution_price` role, and no closed provider-kind list that excludes HIP-4, Deribit, sports, politics, entertainment, outcome-oracle, venue-native, or no-resolution markets.
- [x] T036H0E [US5] Sync PR #480's final tree with current `origin/main` after the NT 0.58/HIP-4 merge, preserve the local Binance/BTCUSDT removal intent in the task contract, and resolve config/example/fixture drift without replacing it with hardcoded Polymarket, hardcoded UP-only, or any other static provider/instrument pair.
- [x] T036H0F [US5] Send the cleaned provider-agnostic contract/task delta to Claude, Gemini, DeepSeek, GLM, and Grok for exact-delta task-list review; record approvals, waivers, and any required dispositions in `specs/024-production-trade-readiness/external-gate-architecture-review.md` before T036H1 begins.
- [x] T036H1 [US5] Add RED config tests in `tests/config_parsing.rs` proving root `[gate_providers.<id>]` accepts the canonical provider TOML from `gate-dataflow-contract.md`: registry-backed provider kind, semantic capabilities, freshness max-age/clock-skew, client binding, and exactly one provider-specific subtable, while rejecting provider-specific fields under `[parameters.runtime]`, under the wrong provider subtable, unregistered provider kinds, or `test_double` providers in live/local operator TOML.
- [x] T036H2 [US5] Add RED target-subscription tests in `tests/config_parsing.rs` proving `[target.gate_subscriptions.<role>]` accepts the canonical subscription TOML from `gate-dataflow-contract.md`: required flag, allowed provider ids or kinds, deterministic `provider_preference`, no-resolution policy, value-kind policy, and market mappings, while rejecting missing required roles, provider capabilities such as `market_metadata` used as gate roles, ambiguous mappings, single static provider assumptions for rotating markets, multiple matching providers without preference, provider-kind/value-kind mismatch, and invalid no-resolution usage.
- [x] T036H3 [US5] Add RED archetype and fixture migration tests in `tests/config_parsing.rs` proving `config/strategies/binary_oracle.toml` and `tests/fixtures/bolt_v3/strategies/binary_oracle.toml` cannot retain provider-specific runtime fields such as `price_to_beat_source`, `price_to_beat_feed_id`, `price_to_beat_report_schema_version`, `price_to_beat_report_decimal_scale`, hardcoded data client/instrument references, or `forced_flat_stale_chainlink_ms` under `[parameters.runtime]`, and proving `binary_oracle_edge_taker::gate_requirements()` exposes only static role/class/value-kind requirements equivalent to `ArchetypeGateRequirement`.
- [x] T036H4 [US5] Add RED selected-market requirement tests in `src/bolt_v3_market_families/mod.rs` and `src/bolt_v3_market_families/updown.rs` proving selected markets expose or config-resolve generic `selected_market_identity` fields: target id, venue, market family, market id, market-complete sorted instrument/outcome ids, market class, resolution kind, resolution identity, value kind, metadata provenance hash, and canonical SHA-256 `selected_market_key`, and fail closed on missing, ambiguous, mismatched mapping, strategy-subset-only instrument ids, venue-specific required fields, noncanonical key derivation, or `|` in any selected-market key component.
- [x] T036H5 [US5] Add RED provider-evidence normalization tests in `tests/bolt_v3_operator_artifacts.rs` proving `GateEvidence` carries role, provider id, provider kind, selected-market key, collector/source timestamps, fresh-until timestamp, value kind, normalized value payload, provider provenance payload, and artifact hash references, and rejects timeout/partial/default evidence.
- [x] T036H6 [US5] Add RED entry-readiness join tests in `tests/bolt_v3_operator_artifacts.rs` proving archetype role/value-kind requirement, target subscription, selected-market requirement, provider capability, and evidence must all match, a static single-provider subscription cannot satisfy dynamic rotation, and multiple matching providers fail closed unless `provider_preference` deterministically selects one.
- [x] T036H7 [US5] Add RED role-separation and no-resolution tests in `tests/bolt_v3_operator_artifacts.rs` proving resolution evidence cannot satisfy decision/reference evidence, decision/reference evidence cannot satisfy resolution evidence, price evidence cannot satisfy outcome/metadata requirements, required-resolution archetypes fail against no-resolution markets, and explicit no-resolution-compatible roles pass without a provider.
- [x] T036H8 [US5] Add RED evidence-lifecycle tests in `tests/bolt_v3_operator_artifacts.rs` proving evidence is keyed by selected-market identity and role, staleness is compared with session `created_at_ms`, source/collector clock skew is enforced, and stale or previous-market evidence cannot satisfy a later selected market even when timestamp-fresh.
- [x] T036H9 [US5] Add RED decision-evidence tests in `tests/bolt_v3_operator_artifacts.rs` and `src/bolt_v3_decision_evidence.rs` proving `BoltV3StrategyInputEvidenceSnapshot` stores `gate_session_hash`, `selected_market_key`, and per-role normalized evidence identity, and cannot satisfy final readiness from a provider-specific `price_to_beat_source` string without the matching readiness session.
- [x] T036H10 [US5] Add RED tiny-canary evidence tests in `tests/bolt_v3_tiny_canary_preconditions.rs` and `tests/bolt_v3_tiny_canary_operator.rs` proving `Phase8StrategyInputSafetyAudit` and `Phase8FinancialEnvelopeEvidenceFile` validate readiness session path/hash or normalized evidence identity, not only `price_to_beat_source == expected_price_to_beat_source`.
- [x] T036H10B [US5] Add RED live-canary gate tests in `tests/bolt_v3_live_canary_gate.rs` proving live-canary readiness accepts provider-neutral `gate_session_path` plus `expected_gate_session_sha256`, rejects source-string-only readiness, and fails closed when the gate session selected-market key does not match the canary target.
- [x] T036H11 [US5] Add RED CLI contract tests in `tests/bolt_v3_cli.rs` proving generic entry-decision/final-packet/live-canary/tiny-canary commands accept provider-neutral `--gate-session` and `--expected-gate-session-sha256`, reject legacy Chainlink-shaped flags such as `--price-report`, `--expected-price-report-sha256`, and `--price-to-beat-source`, and prove provider collector commands positively accept provider-specific inputs only behind configured `provider_id` plus selected-market requirement binding.
- [x] T036H12 [US5] Add RED strategy-registration and runtime no-bypass tests in `tests/bolt_v3_strategy_registration.rs` and `src/strategies/binary_oracle_edge_taker.rs` proving registration requires a readiness-created gate session for required roles and runtime/replay cannot set `market.price_to_beat` directly from `BinaryOracleEntryDecisionEvidenceSource` or any raw provider-specific source without normalized evidence.
- [x] T036H12A [US5] Add RED final-packet binding tests in `tests/bolt_v3_operator_artifacts.rs` proving `operator-evidence-packet.json` and final-packet verification reject any strategy instance with required roles unless the packet binds the readiness gate session path and sha256.
- [x] T036H12B [US5] Add RED live-node registration wiring tests in `tests/bolt_v3_live_canary_gate.rs` or `tests/bolt_v3_strategy_registration.rs` plus `src/bolt_v3_live_node.rs` proving live-node build/registration receives the readiness gate session before strategy registration and cannot rely on a later live-canary gate to backfill missing registration evidence.
- [x] T036H13 [US5] Implement gate config schema and validation in `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, and provider validators under `src/bolt_v3_providers/`. This slice is config-only: parse root gate providers and target subscriptions, validate provider ids/kinds/capability names/value-kind entries, reject missing `provider_kind` and empty capabilities, validate freshness (`max_age_ms > 0`, `max_clock_skew_ms <= max_age_ms`), validate SSM-owned provider fields, enforce exactly one matching provider-specific subtable, reject `test_double` in live/local operator TOML, and produce explicit old-schema migration errors when provider-specific runtime fields appear under `[parameters.runtime]`. Do not add selected-market, `GateEvidence`, `EntryReadinessGateSession`, consumer rewires, or provider collectors in this slice.
- [x] T036H14 [US5] Implement archetype role/value-kind-only refactor in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`: remove provider-specific runtime fields and hardcoded reference instruments, expose provider-neutral `gate_requirements()` with roles/classes/value kinds/no-resolution policy only, and migrate `config/strategies/binary_oracle.toml` plus `tests/fixtures/bolt_v3/strategies/binary_oracle.toml` to the new gate schema with no compatibility shim.
- [x] T036H15 [US5] Implement selected-market requirement extraction in `src/bolt_v3_market_families/mod.rs` and `src/bolt_v3_market_families/updown.rs`, including config-resolved identity mapping when venue metadata does not provide resolution identity, market-complete sorted instrument/outcome ids, `metadata_provenance_sha256`, and canonical `selected_market_key` derivation from sorted selected-market identity JSON.
- [x] T036H16 [US5] Implement provider-neutral `GateEvidence`, `GateSatisfaction`, canonical `EntryReadinessGateSession`, and entry-readiness join in `src/bolt_v3_operator_artifacts.rs`, `src/bolt_v3_providers/mod.rs`, and `src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs`. The join must enforce role/value-kind separation, selected-market binding, freshness/clock-skew policy, deterministic `provider_preference`, explicit no-resolution satisfaction only when archetype/subscription/market all allow it, and canonical `session_hash` derivation.
- [x] T036H17 [US5] Implement readiness-session consumption as separate verified consumer boundaries in `src/bolt_v3_decision_evidence.rs`, `src/bolt_v3_tiny_canary_evidence.rs`, `src/main.rs`, `src/bolt_v3_strategy_registration.rs`, `src/strategies/binary_oracle_edge_taker.rs`, and live/final packet gate paths so decision evidence, tiny canary, live canary, CLI commands, registration, runtime, replay, and final-packet verification consume normalized evidence instead of provider-specific strings.
- [x] T036H18 [US5] Add concrete thin Bolt readiness collection functions under the provider/operator-artifact surface only after the neutral interfaces exist; use data-driven dispatch on configured provider kind, not a trait/plugin framework; cover Chainlink Data Streams and existing NT Hyperliquid HIP-4/venue-native metadata as initial bindings, do not rebuild upstream adapters, and prove each binding captures configured source evidence only when the selected market and target subscription require that provider kind.
- [x] T036H19 [US5] Add final RED/GREEN rotation and no-global-provider tests in `tests/bolt_v3_operator_artifacts.rs` proving Chainlink, existing NT Hyperliquid HIP-4/venue-native, no-resolution, and test-double-backed non-Chainlink provider kinds such as Pyth, exchange-index, Deribit/index, and non-price outcome-oracle markets can rotate without code changes and no provider is globally required unless selected-market metadata and target subscription require it.
- [x] T036I [US5] Add TDD-backed `operator-artifacts collect-chainlink-price-report-source` so T036 can materialize the Chainlink Data Streams report source from TOML-owned REST endpoint fields and SSM-owned credentials without reviving the retired runtime Chainlink client.
- [x] T036I1 [US5] Make Chainlink Data Streams report collection market-resolution-aware with TOML-owned `(resolution_identity, value_kind) -> feed_id/schema/scale` bindings, and preserve the previous-code BTC/USD and ETH/USD testnet feed mappings without treating the old generic fixture feed as a token mapping.
- [x] T036I2 [US5] Restore Chainlink Data Streams credential resolution to the previous working two-SSM-parameter shape (`api_key_ssm_parameter`, `api_secret_ssm_parameter`) instead of requiring a new JSON credential document, and prove the real collector reaches Chainlink testnet for BTC/USD without printing secrets.
- [x] T036I3 [US5] Add source-owned on-chain pUSD balance/allowance collateral proof over NT's HTTP transport and Polymarket spender constants so T036 is not blocked by authenticated CLOB `/balance-allowance` omitting `allowance`.
- [x] T036I4 [US5] Move entry-decision fee proof ownership from caller-supplied `--fee-bps-by-instrument-id` inputs to the Polymarket selected-instrument source-input collector, deriving effective taker fee bps from NT `instrument_taker_fee` and `compute_commission` against the selected book ask prices.
- [x] T036I5 [US5] Move entry-decision reference quote and realized-volatility proof ownership from caller-supplied quote/volatility CLI values to NT quote-observation source evidence, deriving midpoint and realized volatility through the existing `binary_oracle_edge_taker` reference quote and volatility logic.
- [x] T036I6 [US5] Add Chainlink Data Streams binding coverage verification so every configured Chainlink `(resolution_identity, value_kind)` strategy target mapping has exactly one TOML-owned feed binding, no Chainlink feed binding is silently unreachable, and an alternate configured-market regression proves feed selection is token-agnostic with no asset fallback.
- [x] T036I7 [US5] Replace the stale/static `config/strategies/binary_oracle.local.toml` reference-data blocker with a source-owned, market-agnostic decision-reference proof path: the configured provider/source must prove the underlying reference value and realized volatility without reviving Binance/BTC as canonical defaults, without substituting Polymarket outcome quotes for underlying spot/reference prices, and without static condition/instrument IDs that drift across rotating markets.
- [x] T036 [US5] Assemble blocker-free `static-artifacts-manifest.json`, `approval-envelope.json`, and `operator-evidence-packet.json` from real artifacts and record paths in `specs/024-production-trade-readiness/final-packet.md`.
- [x] T037 [US5] Update the approved root TOML operator-evidence block in `config/live.local.toml` with final artifact paths and hashes without printing secrets.
- [x] T038 [US5] Run `operator-artifacts verify-final` against the exact root TOML and final packet; record command, head, hashes, and result in `specs/024-production-trade-readiness/evidence.md`.

## Phase 7: Exact-Head Verification

**Goal**: Close T130 before any final-packet no-submit or tiny-capital canary operation.

**Independent Test**: local checks and GitHub CI target the same pushed head.

- [x] T039 [US5] Run focused readiness tests: `tests/bolt_v3_operator_artifacts.rs`, `tests/bolt_v3_tiny_canary_preconditions.rs`, `tests/bolt_v3_tiny_canary_operator.rs`, `tests/bolt_v3_live_canary_gate.rs`, and `tests/bolt_v3_cli.rs`.
- [x] T040 [US5] Run full local verification: `cargo fmt --check`, `git diff --check`, runtime-literal verification, source/slop/hardcode/secret scans, and readiness test suites; record output summary in `specs/024-production-trade-readiness/evidence.md`.
- [x] T041 [US5] Push PR #480 and record exact-head GitHub CI evidence in `specs/024-production-trade-readiness/evidence.md`.

## Phase 8: Approved Operations

**Goal**: Close T131/T122 and T116/T046 only after final packet and T130 pass.

**Independent Test**: final-packet no-submit passes before tiny-capital canary; both are bound to exact head, root TOML, final packet, and retained evidence hashes.

- [x] T043 [US5] Execute T131/T122 final-packet EC2/EIP no-submit rerun with the verified root TOML and final operator packet; record evidence in `specs/024-production-trade-readiness/final-no-submit.md`. **Evidence STALE pending exact-head rerun: HEAD has moved (~100 commits past the recorded no-submit head) and CI is RED at HEAD `5097e6bc`. The no-submit must be regenerated at the frozen `FINAL_HEAD` (on EC2 / the allowlisted EIP) before it can gate T044. See recovery-plan.md R8/R9.**
- [ ] T043A [US5] Validate the PR-enabled data-client adapters before treating them as production-usable: record a venue-neutral matrix in `specs/024-production-trade-readiness/data-adapter-production-readiness.md` proving config-owned LiveNode wiring, NT data-path behavior beyond metadata-only smoke, freshness/reconnect/rate-limit/error handling, credential/no-execution boundaries for data-only clients, and no venue/market hardcodes.
- [x] T043B [US5] Validate the selected tiny-capital trade path separately from the all-venue data-client claim: record in `specs/024-production-trade-readiness/tiny-canary.md` that the configured canary path has production-usable selected-market data evidence, final-packet/no-submit evidence at the current head, pre-consumption approval freshness checks, max-order/max-notional bounds, submit-admission/reconciliation/post-run proof contracts, and fail-closed behavior before live capital. **Evidence STALE pending exact-head rerun: the "at the current head" final-packet/no-submit evidence predates the current HEAD by ~100 commits and CI is RED at HEAD `5097e6bc`. T043B's at-head evidence must be re-affirmed at the frozen `FINAL_HEAD` immediately before T044 approval consumption. See recovery-plan.md R8/R9.**
- [ ] T044 [US5] Execute T116/T046 tiny-capital canary with the verified root TOML and final operator packet; record evidence in `specs/024-production-trade-readiness/tiny-canary.md`.
- [ ] T045 [US5] Run post-run artifact/log secret scan and record retention/purge decision in `specs/024-production-trade-readiness/post-run-hygiene.md`.
- [ ] T046 [US5] Update #369, #385, #409, #360, and PR #480 with exact final readiness status and record links in `specs/024-production-trade-readiness/readiness-ledger.md`.
- [ ] T047 [US5] Run the final hardcode/architecture cleanup audit across code, examples, fixtures, specs, and operator TOML: remove or justify remaining runtime literals, static venue/market examples, stale provider-specific assumptions, and architecture drift before completion. **NOT complete: contradicted by live CI at HEAD `5097e6bc` (run 26673253247) — clippy is RED (`operator_artifacts.rs:7837,:7857` needless_borrow; `binary_oracle_edge_taker.rs:5972` useless_conversion) and the runtime-literal allowlist is out of sync (5 stale rows + ~60 unclassified literals). T047 cannot be marked complete until CI is green at HEAD, including the full `just source-fence` (all ~13 verifier pairs incl `verify_bolt_v3_provider_leaks.py`), not only the runtime-literal self-test. See recovery-plan.md R1/R5/R7.**

## Dependencies

- T001-T005 must complete before implementation resumes.
- T006-T008 may run in parallel after T005.
- T009-T014, T015-T024, and T025-T034 can be parallelized by collector group after T005, but each implementation task depends on its RED test.
- T035-T036G3 require T009-T034.
- T036H0F must complete before code implementation resumes unless the operator explicitly waives the cleaned-contract exact-delta review.
- T036H1-T036H12B are the required RED-test slice before implementation; each must fail before T036H13-T036H18 are implemented.
- T036H13-T036H18 require T036H1-T036H12B and should be implemented in boundary order: config-only schema validation, archetype role declaration and fixture migration, selected-market identity, provider/operator evidence and session join, consumer/session surfaces, thin provider readiness bindings over existing upstream adapters.
- T036H19 requires T036H13-T036H18 and must complete before T036.
- T036-T038 require T036H1-T036H19 and T036I.
- T039-T041 require T035-T038.
- T043 requires T041.
- T043A requires T043.
- T043B requires T043 and the T043A matrix row for the selected trade path to be production-usable.
- T044 requires T043B and renewed explicit operator approval. The remaining non-selected T043A venue rows gate the PR's multi-venue data-client production-usability claim, not the selected trade path's tiny-capital canary.
- T045-T046 require T044.

## MVP

The current next slice is T044 tiny-capital canary after renewed explicit operator approval. T043B is locally closed with final-packet pre-run verification and no-submit readiness evidence at `978618f85e12b81ea56dab2f2e11aa6156d022e0`, plus later post-doc checkpoint reruns; because every evidence-record or repair commit changes `HEAD`, rerun final-packet verification and no-submit at the exact head immediately before T044 approval consumption if any later commit exists. The operator-approved T044 retry at `9fa15005` failed closed during source-owned entry-decision evidence generation before approval consumption; repair commit `b9a15da3` now rejects non-boundary `price_to_beat` reports. A later source-only attempt at `7efad2cb` passed boundary/reference/venue source collection but failed closed because the configured strategy produced `no_side_selected` with both sides negative EV, so no decision evidence or approval-consuming live step was run. A later approved live attempt at `78a03da5` generated and verified fresh source/no-submit evidence, consumed the one-time approval, connected the configured selected data/execution clients, then failed closed before submit when runtime entry evaluation produced `no_side_selected`; post-live venue account-state evidence matched pre-run with zero open orders and zero open positions. The current repair makes the production live runner and Phase 8 operator harness write blocked-before-submit canary evidence when the live runner returns with zero admitted orders, while preserving the stricter successful-live-proof path for admitted orders. On 2026-05-29 the operator froze broad T043A adapter remediation for today's canary sequence: the all-venue T043A matrix currently proves 7 configured data rows, including the configured selected data row, while 4 unrelated fail-closed data-only rows remain explicit residual scope. T043A remains open for the broader multi-venue data-client production-usability claim until every requested data-only venue row is production-usable or explicitly dispositioned as unsupported by pinned NT/current venue behavior. After T044, the remaining work is T045 post-run hygiene and T046 issue/PR/readiness-ledger updates. **T047 local cleanup is NOT complete: live CI at HEAD `5097e6bc` (run 26673253247) is RED — clippy warnings and an out-of-sync runtime-literal allowlist remain. T047 stays open until CI is green at the frozen `FINAL_HEAD`, including the full `just source-fence` (~13 verifier pairs, not just the runtime-literal self-test). See recovery-plan.md R1/R5/R7.** No hardcoded runtime values remains a cross-cutting invariant for every later slice. Final GitHub CI should be run once at the end after remaining implementation/evidence commits, not after every docs-only update.
