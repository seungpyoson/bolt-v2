# PR #388 T124-T128 Root-Problem Memos

Date: 2026-05-23
PR: #388
Branch: `codex/production-readiness-evidence-audit`
Historical review head: `3ac5b3a40367803ea126bb2fa07232d73d235c04`
Base recorded by PR API: `08d26ae05b03d448b8917f2aaf39a4c37bd0b38d`
`origin/main` recorded at review time: `500f0aa0b423fd852dcde8b52658f14b9545670f`

Historical state anchor:

- `git fetch origin` completed after sandbox escalation for worktree git metadata.
- `git status --short --branch` reported branch `codex/production-readiness-evidence-audit...origin/codex/production-readiness-evidence-audit` with unrelated dirty dotfiles: modified `.codex/config.toml`, deleted `.codex/hooks.json`, deleted `.gemini/settings.json`.
- `gh pr view 388 --repo seungpyoson/bolt-v2 --json state,title,url,headRefOid,baseRefOid,mergeStateStatus,isDraft` reported the historical review target as PR open, non-draft, clean merge state, with target SHA `3ac5b3a40367803ea126bb2fa07232d73d235c04`.
- `gh pr checks 388 --repo seungpyoson/bolt-v2` reported checks passing for that historical PR head, with deploy and same-sha-main-evidence skipped.
- `gh api repos/seungpyoson/bolt-v2/branches/main --jq .commit.sha` reported remote main at review time `500f0aa0b423fd852dcde8b52658f14b9545670f`; `gh api repos/seungpyoson/bolt-v2/pulls/388 --jq '{base:.base.sha, head:.head.sha, mergeable_state:.mergeable}'` reported PR base `08d26ae05b03d448b8917f2aaf39a4c37bd0b38d`, historical target SHA `3ac5b3a40367803ea126bb2fa07232d73d235c04`, mergeable `true`, mergeable_state `clean`.

Scope note:

- These are Phase 0 diagnosis memos only. They do not claim no-submit, tiny-canary, or production-trade readiness.
- Evidence came from current source inspection and read-only subagents `019e54fb-400f-7262-82dd-ad3f7f0c2b3e`, `019e54fb-54f6-7d60-8d82-cbf1e59e0c21`, `019e54fb-6969-7681-8b84-cd0389d4fdd6`, `019e54fb-826e-7640-b213-039141f9c324`, and `019e54fb-970e-7252-ba73-0ce1b0a716c0`.

## Phase 0 External Review

Source packet: this memo plus `src/bolt_v3_operator_artifacts.rs`, `src/bolt_v3_config.rs`, `src/bolt_v3_decision_evidence.rs`, `tests/bolt_v3_operator_artifacts.rs`, and `tests/bolt_v3_cli.rs`.

- Claude Code job `7daf624e-b225-412b-8298-0e8d79dc82e0`: APPROVE, no blockers.
- Gemini job `98581317-9341-4eac-ac5c-345fee0342ce`: APPROVE, no blockers.
- Kimi Code CLI job `203113f9-2f81-4199-ab64-7b42405997e8`: APPROVE, no blockers.
- GLM job `job_8612d4fd-47f6-4f4f-a459-fc53e47e8934`: APPROVE, no blockers.
- DeepSeek job `job_40d82334-2b6a-4a13-82fd-0b746b9df764`: APPROVE, no blockers.
- Grok job `job_241058b2-d776-4f78-a117-62e79a012733`: APPROVE, no blockers.

Implementation notes carried forward from review:

- When T125-T127 static-generation success paths become reachable, add those output files to the cleanup ledger used by `write_static_operator_artifacts`.
- Bound market-selection source reads on the strategy-input writer path instead of using an unbounded `read_to_end`.
- Keep embedded market-selection source paths bounded, symlink-rejected, and parent-dir-rejected; do not reject absolute paths without changing the existing operator-evidence path contract.
- Treat market-selection source as bound into the final packet through strategy-input/pre-run proof hashes, not as a top-level `[live_canary.operator_evidence]` field.

## T124

TASK: Add source-bound market-selection evidence generation for T115.

ROOT PROBLEM: T115 needs `market-selection-source.json`, but the artifact must come from configured market-family selection and NT instrument facts plus runtime strategy-decision proof. Static/no-submit generation cannot synthesize it.

CURRENT CODE PATH: `write_market_selection_source_artifact_from_decision_evidence_file` reads a bounded complete decision-evidence chain, checks configured target and price-to-beat source, rejects missing/unusable price-to-beat, requires `market_selection_timestamp_ms`, builds source evidence from loaded TOML and `InstrumentAny`, then validates it against the runtime snapshot before writing. The raw builder uses `target_runtime_fields_from_target`, `market_selection_candidate_windows_from_target`, and `select_binary_option_market_from_target`.

CURRENT FAIL-CLOSED POINT: `write_market_selection_source_artifact` still returns `MARKET_SELECTION_SOURCE_BLOCKER`, and static artifact generation records that blocker instead of writing `market-selection-source.json`.

MISSING SOURCE PROOF: No final current-head T115 flow consumes strategy-input evidence whose embedded market-selection source path/hash came from a real generated `market-selection-source.json`. The positive tests use fixture decision evidence, not an operator packet generated from a real runtime capture.

RUNTIME PATH THAT MUST OWN VALUE: The strategy runtime must own the selected market identity, timestamp, price-to-beat source/value, instrument ids, and order decision chain. Operator artifact code may only validate and promote those values.

ARTIFACT EXPECTED: `market-selection-source.json` as `Phase8MarketSelectionSourceEvidenceFile`: record/source, candidate windows, selected outcome, condition/slug/question ids, up/down instrument ids, observed timestamp, start timestamp, and end timestamp.

RED TEST TO PROVE GAP: A final-packet/pre-run proof path test that attempts to consume strategy-input evidence without a runtime-chain-generated market-selection source path/hash should fail before any packet write. Existing gap coverage is `market_selection_source_writer_fails_closed_until_strategy_decision_inputs_exist`; existing positive coverage is fixture promotion only.

WHY THIS IS NOT HARDCODED: The intended value is selected from loaded strategy target fields and NT `BinaryOption` metadata through market-family dispatch. The writer must not contain venue, market, symbol, price, quantity, or strategy literals beyond schema names.

CONFIDENCE: 100% for current code-path diagnosis, backed by source inspection and scout confirmation. Not 100% for runtime readiness because no real current-head artifact packet exists.

## T125

TASK: Add source-bound strategy-input evidence generation for T115.

ROOT PROBLEM: Current code can promote a bounded runtime decision-evidence chain into `strategy-input.json`, but no real current-head operator packet proves that the runtime produced and bound that chain.

CURRENT CODE PATH: `binary_oracle_edge_taker` builds a `BoltV3StrategyInputEvidenceSnapshot` from strategy runtime state before order intent/admission, records it through `BoltV3DecisionEvidenceWriter`, `read_latest_entry_decision_evidence_chain` validates a complete snapshot/intent/admission chain, and `write_strategy_input_evidence_artifact_from_decision_evidence_file` promotes it with a market-selection-source reference.

CURRENT FAIL-CLOSED POINT: `write_strategy_input_evidence_artifact` returns `T046 remains blocked: missing source-bound price-to-beat strategy decision input`; static generation records the blocker and writes no `strategy-input.json`.

MISSING SOURCE PROOF: Missing real runtime JSONL with strategy snapshot, order intent, and admission for current head; missing matching market-selection source artifact; missing final T115 hash/config binding.

RUNTIME PATH THAT MUST OWN VALUE: `binary_oracle_edge_taker` must own price-to-beat source/value, reference quote timestamp, spot, volatility, time-to-end, pricing inputs, edge inputs, fee inputs, selected side, instrument ids, price, quantity, and client order id. Artifact code must validate only.

ARTIFACT EXPECTED: `strategy-input.json` plus SHA-256, generated from runtime decision evidence and a matching market-selection-source artifact, then referenced by `[live_canary.operator_evidence]`.

RED TEST TO PROVE GAP: A packet-level test that static generation or packet assembly cannot produce/consume `strategy-input.json` unless the source decision-evidence chain exists and hashes match. Existing positive unit coverage proves fixture-chain promotion only.

WHY THIS IS NOT HARDCODED: The source values are emitted by strategy runtime state and checked against loaded TOML/financial envelope before writing. No config/runtime values should be injected as constants to bridge the missing run artifact.

CONFIDENCE: 100% for current source diagnosis, backed by source inspection and scout confirmation. Not 100% for readiness because no real current-head decision-evidence artifact is present.

## T126

TASK: Add source-bound pre-run-state evidence generation for T115.

ROOT PROBLEM: The writer accepts a full `Phase8PreRunStateSourceProofs` bundle, but most real source-owned collectors do not exist. Only release-manifest and market/window proof collectors are present.

CURRENT CODE PATH: `write_pre_run_state_artifact` validates the financial envelope, then fails closed. `write_pre_run_state_artifact_from_source_proofs` can write a file only if caller supplies every required proof field. Current collectors are `collect_pre_run_release_manifest_source_proof` and `collect_pre_run_market_window_source_proof`.

CURRENT FAIL-CLOSED POINT: `write_pre_run_state_artifact` returns `T121 remains blocked: T046 source-bound pre-run state evidence is unproven`. Live gate also requires configured `pre_run_state_path`/`pre_run_state_sha256`, hash shape, file hash match, and approval-envelope match.

MISSING SOURCE PROOF: Missing collectors for host clock, venue account/open orders/positions, funding/margin, single-runner lock, egress identity, CLOB V2 adapter signing, CLOB collateral accounting, and CLOB fee behavior.

RUNTIME PATH THAT MUST OWN VALUE: Runtime/preflight collectors must own the proof booleans and proof hashes. The writer only copies and validates supplied proof fields against loaded TOML-derived identity.

ARTIFACT EXPECTED: `pre-run-state.json` with loaded TOML-bound `execution_client_id` and `configured_target_id`, plus all required true booleans and lowercase SHA-256 proof hashes. Any false or invalid field must keep the final artifact absent.

RED TEST TO PROVE GAP: A collector-level test that release-manifest and market/window proofs alone cannot produce `pre-run-state.json`, and that the production generation path reports the first missing runtime-owned proof without writing an artifact.

WHY THIS IS NOT HARDCODED: Existing collectors derive from source files or bounded strategy-input artifacts. Remaining fields must come from live/preflight source-owned checks, not fixture hashes or constant booleans.

CONFIDENCE: 100% for current code-path diagnosis, backed by source inspection and scout confirmation. Not 100% for readiness because most collectors are missing.

## T127

TASK: Add source-bound abort-plan evidence generation for T115.

ROOT PROBLEM: The abort-plan writer can persist a caller-supplied proof bundle, but there are no real source-owned collectors for the required abort paths.

CURRENT CODE PATH: Static generation calls `write_abort_plan_artifact`, which derives the financial envelope and fails closed. `write_abort_plan_artifact_from_source_proofs` can write an artifact from `Phase8AbortPlanSourceProofs`.

CURRENT FAIL-CLOSED POINT: `write_abort_plan_artifact` returns `AbortPrerequisiteUnproven { prerequisite: "panic gate and service policy" }`; CLI/static-manifest tests assert this blocker and no successful `abort-plan.json`.

MISSING SOURCE PROOF: Missing source-owned proof for cancel-if-open, NT-accepted/venue-pending, partial-fill, network-partition during submit, and panic-gate/service-policy behavior.

RUNTIME PATH THAT MUST OWN VALUE: Abort/lifecycle policy collectors must own the proof booleans and evidence hashes. The writer must remain a validator and serializer.

ARTIFACT EXPECTED: `abort-plan.json` with config-derived `execution_client_id` and `configured_target_id`, plus all five abort-path booleans true and each path bound to a lowercase SHA-256 evidence hash.

RED TEST TO PROVE GAP: A production collector test that asks for abort-plan generation without source-owned abort-path proofs and asserts no artifact is written. Existing writer tests do not prove collector ownership.

WHY THIS IS NOT HARDCODED: The accepted writer path derives identity from loaded TOML and requires proof hashes; fixture hashes and true booleans are valid only in tests.

CONFIDENCE: 100% for current code-path diagnosis, backed by source inspection and scout confirmation. Not 100% for readiness because collectors are absent.

## T128

TASK: Add approval-envelope/operator-packet assembly that consumes existing artifact paths/hashes, writes no secrets, avoids circular hashes, and refuses static manifests with blockers.

ROOT PROBLEM: Assembly and verification surfaces exist, but a real final packet is impossible until T124-T127 artifacts exist and `[live_canary.operator_evidence]` binds their paths/hashes.

CURRENT CODE PATH: `write_static_operator_artifacts` writes static refs and blockers; `assemble_operator_packet_from_static_manifest` validates a blocker-free manifest and configured operator evidence, then writes `approval-envelope.json` and `operator-evidence-packet.json`; `verify_final_operator_packet` is exposed by CLI as `operator-artifacts verify-final`.

CURRENT FAIL-CLOSED POINT: Assembly rejects missing `[live_canary.operator_evidence]`, non-empty static-manifest blockers, config-bundle drift, missing required artifact refs, and path/hash/file mismatch. Current static generation records blockers for market selection, strategy input, pre-run state, and abort plan.

MISSING SOURCE PROOF: Missing source-owned artifacts from T124-T127 and missing final config-owned operator evidence block. Without those, no blocker-free static manifest or packet can be legitimate.

RUNTIME PATH THAT MUST OWN VALUE: `[live_canary.operator_evidence]` in TOML owns all final packet paths and hashes. The live canary gate must consume those values and validate file hashes plus approval envelope before runner entry.

ARTIFACT EXPECTED: blocker-free `static-artifacts-manifest.json`, non-circular `approval-envelope.json`, and `operator-evidence-packet.json` containing only path/SHA fields suitable for TOML-owned operator evidence.

RED TEST TO PROVE GAP: Existing `approval_packet_assembly_refuses_static_manifest_with_blockers` proves refusal. The next useful RED is an end-to-end final-packet test that fails until T124-T127 source-owned artifacts and `[live_canary.operator_evidence]` exist together.

WHY THIS IS NOT HARDCODED: Assembly copies loaded TOML operator-evidence values and verified artifact refs. It does not synthesize runtime policy, secrets, raw nonce material, SSM paths, venue ids, or order ids.

CONFIDENCE: 100% for current code-path diagnosis, backed by source inspection and scout confirmation. Not 100% for final packet readiness because prerequisite artifacts are missing.

## Critical Path

1. Rebase or merge current `origin/main` if required before further implementation, because remote main has advanced past the PR base from `08d26ae0` to `500f0aa0`.
2. Implement source-owned production collectors/generation paths before checking T124-T128:
   - T124/T125: connect real runtime decision-evidence artifacts to operator artifact generation, with market-selection source bound through strategy-input/pre-run proof hashes.
   - T126: add remaining host/account/funding/single-runner/egress/CLOB collectors.
   - T127: add abort-path collectors.
   - T128: assemble only after the static manifest is blocker-free and TOML owns all paths/hashes.
3. Only after T124-T128 artifacts are real and bound, run T130 aggregate verification.
4. Only after T130 passes, resume T131/T122 final-packet EC2/EIP no-submit rerun.
