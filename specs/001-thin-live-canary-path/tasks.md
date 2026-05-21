# Tasks: Thin Bolt-v3 Live Canary Path

**Input**: Design documents from `/specs/001-thin-live-canary-path/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`

All code tasks use TDD. For each behavior: write failing test, run it and capture expected failure, implement minimal code, run green test, run phase verification, then commit. Do not batch unrelated slices.

## Phase 1: Planning And Evidence

**Purpose**: Lock constraints before runtime code.

- [x] T001 Record current-state evidence in `specs/001-thin-live-canary-path/research.md`.
- [x] T002 Replace `.specify/memory/constitution.md` with bolt-v3 constitution.
- [x] T003 Create feature spec, implementation plan, data model, contracts, quickstart, and tasks under `specs/001-thin-live-canary-path/`.
- [x] T004 Update `AGENTS.md` SPECKIT block to point to `specs/001-thin-live-canary-path/plan.md`.
- [x] T005 Run the active no-mistakes binary's `status` and `runs --limit 5`; when an issue-specific soak binary is active, use the operator-provided override path and record triage result in final handoff and shared soak log if a run exists.
- [x] T006 Verify planning artifacts with `rg -n "(?i:TB[D]|TO[D]O|fix[[:space:]]+later|NE[E]DS[[:space:]]+CLARIFICATION)|\\[[A-Z][A-Z0-9]*(_[A-Z0-9]+)+\\]" .specify/memory/constitution.md specs/001-thin-live-canary-path` and `git diff --check`.

## Phase 2: Production Entrypoint Adoption (US1)

**Goal**: Production binary enters one bolt-v3 build/run path.

**Independent Test**: `cargo test --test bolt_v3_production_entrypoint` fails before implementation because `src/main.rs` still calls `node.run()` directly, then passes after legacy runtime path is removed or made unreachable.

- [x] T007 [US1] Write failing test `tests/bolt_v3_production_entrypoint.rs::main_uses_bolt_v3_runner_wrapper_only` asserting `src/main.rs` contains no production direct `node.run()` and imports/calls `run_bolt_v3_live_node`.
- [x] T008 [US1] Run `cargo test --test bolt_v3_production_entrypoint main_uses_bolt_v3_runner_wrapper_only -- --nocapture`; expected failure references current direct `node.run()` in `src/main.rs`.
- [x] T009 [US1] Refactor `src/main.rs` to load bolt-v3 TOML, validate, build via `build_bolt_v3_live_node`, and run via `run_bolt_v3_live_node`.
- [x] T010 [US1] Remove or isolate legacy production config/ruleset runtime so it cannot be selected in production.
- [x] T011 [US1] Run `cargo test --test bolt_v3_production_entrypoint`, `cargo test --test bolt_v3_live_canary_gate`, and `cargo test --test config_parsing`.
- [x] T012 [US1] Run the active no-mistakes binary's `status`; if unavailable, record that fact instead of blocking the code slice.

## Phase 3: Generic Strategy And Runtime Registration (US3)

**Goal**: Bolt-v3 live-node build path registers configured strategies through a strategy binding, without core concrete strategy leakage.

**Independent Test**: `cargo test --test bolt_v3_strategy_registration` proves injected fake strategy binding can register through core and unsupported strategy fails closed.

- [x] T013 [US3] Write failing tests in `tests/bolt_v3_strategy_registration.rs` for fake binding registration, unsupported strategy rejection, and no concrete strategy key in core registration code.
- [x] T014 [US3] Run `cargo test --test bolt_v3_strategy_registration -- --nocapture`; expected failures show missing bolt-v3 strategy registration surface.
- [x] T015 [US3] Add `src/bolt_v3_strategy_registration.rs` with a generic `StrategyBinding` interface and production binding table.
- [x] T016 [US3] Wire strategy registration into `src/bolt_v3_live_node.rs` after NT client registration and before runner entry.
- [x] T017 [US3] Run `cargo test --test bolt_v3_strategy_registration` and `cargo test --test bolt_v3_provider_binding`.

## Phase 4: Initial Binary-oracle Edge Taker Activation (US3)

**Goal**: Initial taker strategy is configured by reference roles and strategy parameters, not hardcoded provider assumptions.

**Independent Test**: Existing strategy tests plus new config tests prove Polymarket option venue, Chainlink primary reference, and multiple exchange reference roles are configured through TOML.

- [x] T018 [US3] Write failing validation tests proving strategy config accepts multiple exchange reference roles and rejects missing primary oracle/reference roles.
- [x] T019 [US3] Run targeted strategy/config tests; expected failure shows current config cannot express all required reference roles.
- [x] T020 [US3] Extend strategy-archetype validation in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` to validate reference roles generically.
- [x] T021 [US3] Extend fixtures under `tests/fixtures/bolt_v3/` only for operator-visible runtime values, not test-local timing scaffolding.
- [x] T022 [US3] Run `cargo test --test config_parsing` and targeted tests for the initial registered taker strategy.

## Phase 5: Mandatory Decision Evidence (US2)

**Goal**: No submit path can exist without bolt-v3 decision evidence.

**Independent Test**: Strategy construction and submit tests fail closed when evidence writer is absent or persistence fails.

- [x] T023 [US2] Write failing tests that construct the strategy without decision evidence and expect construction rejection.
- [x] T024 [US2] Write failing tests that simulate evidence persistence failure and expect submit rejection before NT submit.
- [x] T025 [US2] Remove optional/fallback evidence submit path from the registered strategy implementation.
- [x] T026 [US2] Make bolt-v3 strategy registration provide mandatory decision evidence.
- [x] T027 [US2] Run targeted strategy tests and source-fence search for fallback direct submit branches.

## Phase 6: Submit Admission Consumes Gate Report (US2)

**Goal**: `BoltV3LiveCanaryGateReport` bounds are enforced before every live submit.

**Independent Test**: `cargo test --test bolt_v3_submit_admission` proves order count, notional cap, cap equality, missing/unarmed report, double-arm/stale-arm, global submit budget, cancel exclusion, and evidence failure all reject before NT submit without consuming admission budget before evidence persists.

- [x] T028 [US2] Write one failing public behavior test in `tests/bolt_v3_submit_admission.rs` for unarmed admission rejecting before NT submit with `NotArmed`.
- [x] T029 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture`; expected failures show missing submit admission module.
- [x] T030 [US2] Add `src/bolt_v3_submit_admission.rs` with shared admission state armed only from `BoltV3LiveCanaryGateReport`.
- [x] T031 [US2] Continue vertical TDD, one behavior at a time, for count cap, notional cap, cap equality, evidence-failure-before-admission, success ordering, global entry/exit/replace-submit budget, cancel exclusion, and double-arm/stale-arm behavior.
- [x] T032 [US2] Wire one shared admission handle from live-node build through strategy contexts into `run_bolt_v3_live_node`, then wire strategy submit calls through decision evidence, submit admission, admission permit, and NT submit.
- [x] T033 [US2] Run `cargo test --test bolt_v3_submit_admission`, targeted strategy submit tests, and source-fence checks across `src/strategies/**/*.rs` and `src/bolt_v3_archetypes/**/*.rs` for direct `submit_order` bypasses.

## Phase 7: Authenticated No-submit Readiness (US4)

**Goal**: Real SSM/venue connect-disconnect produces a redacted report consumed by PR #305 gate.

**Independent Test**: Local tests cover report schema and zero-order guard. Ignored operator test produces real artifact only with explicit approval.

Live-readiness evidence note: T038 is checked only for the no-submit EC2/EIP
connectivity proof recorded below. T046 remains unchecked until explicit
operator-run evidence produces a satisfied tiny-canary artifact. PR #331 P9
cites unchecked live-canary work only as a blocker, not as deferred approval to
trade.

- [x] T034 [US4] Write failing schema tests for no-submit readiness report producer and gate consumer compatibility.
- [x] T035 [US4] Write zero-order source/behavior fence proving readiness code cannot call submit, cancel, replace, or amend order APIs.
- [x] T036 [US4] Implement minimal no-submit readiness runner using existing bolt-v3 build and controlled-connect/disconnect boundaries.
- [x] T037 [US4] Run local readiness tests with mock SSM resolver and no network.
- [x] T038 [US4] With explicit operator approval, run ignored real SSM/venue no-submit readiness and store redacted report path outside tracked secrets.
  - Current proof packet: `docs/bolt-v3/2026-05-21-t038-binance-operator-proof-packet.md` records the current-head EC2/EIP no-submit proof that satisfies T038 only. It is not T046 canary approval and not production trade readiness.
  - 2026-05-21 18:34:44 KST evidence attempt at head `3190803c5cb51ffeaebbd80a029c4a65bf3291c4`: `cargo run --bin bolt-v2 -- no-submit-readiness --config config/live.local.toml` wrote `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json` outside tracked secrets with mode `-rw-------`, but this relative-path attempt is retained only as failed-connect history after the later two-config audit. It is not used as config-identity proof. Readiness remains blocked because `controlled_connect` failed after Binance reference data did not connect/produce the configured quote within `[live_canary].reference_quote_wait_timeout_seconds=20`; `reference_readiness` was skipped because controlled connect failed.
  - 2026-05-21 19:34:17 KST non-secret follow-up probe at head `d69b43c22ce22d018bc1c39006bbd2e7d642c372`: pinned `binance_reference`, fetched both configured SSM parameters successfully without printing values, observed the configured Binance API key was nonempty, validated the API secret as Ed25519 PKCS#8 key material, reached Binance `/api/v3/time` with HTTP `200`, then signed read-only `/api/v3/account` and received HTTP `401` with Binance code `-2015` (`Invalid API-key, IP, or permissions for action.`). This rules out empty configured SSM values and malformed Ed25519 private-key shape in this probe, but still leaves wrong configured SSM parameter target, key pairing/state, IP whitelist, permission, account, or environment configuration as possible blockers; it is not a satisfied no-submit readiness report and T038 stayed unchecked at that time.
  - 2026-05-21 21:11:39 KST metadata audit executed at pre-doc-commit head `7dcda025f987d80f261500ca3094fb42ab9ce9de`: `cargo run --quiet --bin bolt-v2 -- secrets check --config /Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml` and `cargo run --quiet --bin bolt-v2 -- secrets resolve --config /Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml` both completed without printing secret values. The ignored worktree config has SHA-256 `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`, mode `0600`, SBE WebSocket endpoint, and live-canary freshness fields; `/Users/spson/Projects/Claude/bolt-v2/config/live.local.toml` has SHA-256 `62e6b2dd793753e77f7042376adf6be1c9245969393c695a50e5de65946bacc7`, mode `0644`, JSON WebSocket endpoint, and no recorded freshness fields. The configured Binance SSM path hashes match across both configs, but metadata-only AWS SSM inspection showed the API-key parameter as `SecureString` version `1` last modified `2026-04-19T18:47:41.113000+09:00` and the API-secret parameter as `SecureString` version `2` last modified `2026-05-20T09:12:33.893000+09:00`. Five external reviewers (Gemini `60d5d717-8c75-4224-8469-5d42ff67a2bf`, Claude `7d37939d-55da-43cc-9860-5d7441e03d2c`, GLM `job_fe2699da-d790-4d74-ba3a-03217b6b09b5`, DeepSeek `job_76cdd847-8126-4ae2-83a7-b322c23427a6`, Kimi `da8ccf8d-3931-4f1c-b5f2-174fe3330e81`) approved the classification: no code change is supported, T038/T046 stay unchecked, the SSM version/date asymmetry makes key-secret pairing the lead hypothesis but not proof, and the next T038 attempt must pin absolute config path, raw config SHA, resulting config bundle checksum, exact head, and a satisfied stage-complete no-submit report.
  - 2026-05-21 21:28:36 KST non-secret Binance auth probe executed at pre-doc-commit head `dfd60bd5d10779ec6ea48c39a7a066b2cf382a48`: printed no secret values, derived only the configured Ed25519 public-key fingerprint from the SSM API secret (`sha256=1d29db2eb2abf9f63afc99dd580125d83c9966a94e38d875f7adf0e5581c3df9`, derived public key length `32` bytes), reached Binance `/api/v3/time` with HTTP `200`, then signed read-only `/api/v3/account` and received HTTP `401` with Binance code `-2015` (`Invalid API-key, IP, or permissions for action.`). This is blocker evidence only: it does not prove root cause, does not prove the key is active or paired, does not produce a satisfied no-submit readiness report, and leaves wrong configured SSM target, key pairing/state, IP whitelist, permission, account, environment, or Binance-side key state as possible blockers. Follow-up selected-source review of this wording also approved with no blockers: Gemini `e236bc8a-2465-40ea-bf4f-52490a2ded3c`, Claude `190343b2-065f-4470-84b3-a8596bce16c4`, GLM `job_450b4d53-fb29-4f3e-8f60-d28c8f30ecb8`, DeepSeek `job_967b9d7a-3b23-48ca-b379-14997b6350d5`, and Kimi `2380be2e-60ad-48d7-8c5d-c48a95a824c8`. T038 stayed unchecked at that time.
  - 2026-05-21 22:00:06 KST approved current-head T038 rerun at head `c4f65cdc3f68f23668c8be37da7270df8bc4f167`: `secrets check` and `secrets resolve` passed against absolute config path `/Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml` without printing secret values; config SHA-256 was `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`, mode `0600`. `cargo run --bin bolt-v2 -- no-submit-readiness --config /Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml` wrote `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json`, mode `-rw-------`, size `1283`, report SHA-256 `5918e03c3cfa66243a56d55c43b075a39bd345bad25a52bc895274b4c32ecb1a`, schema `bolt-v3.no-submit-readiness.v2`, generated timestamp `1779368467` (`2026-05-21 22:01:07 KST`), config bundle checksum `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`, and executable identity `c9e55c6df8fff29eeac1ad9f8fe8325d1c5251e50337065351c16f528411d04a`. Stages `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_disconnect`, and `report_write` were satisfied; `controlled_connect` failed because live reference quote evidence was not observed; `reference_readiness` was skipped. Runtime evidence again showed `polymarket_main` data/execution connected, `binance_reference` data not connected, `DataEngine.check_connected() == false`, `ExecEngine.check_connected() == true`, NT did not start the trader, and Binance SBE rejected the WebSocket handshake with `Invalid X-MBX-APIKEY header`. Process check found no lingering no-submit runner beyond the check command itself. This is fresh blocker evidence only and still does not produce a satisfied no-submit readiness report. T038 stayed unchecked at that time.
  - 2026-05-21 23:28:49 KST approved T038 rerun at head `ac656c2bdd9c5457a3682aa29355d94c48715049`: operator attested the Binance key type, active state, configured public-key fingerprint match, and EIP `34.248.143.2` allowlist entry. Local evidence verified `config/live.local.toml` uses Binance mainnet REST `https://api.binance.com` and SBE WS `wss://stream-sbe.binance.com/ws`; `secrets check` and `secrets resolve` passed without printing secret values. The no-submit run wrote `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json`, mode `0600`, size `1283`, report SHA-256 `1ea225543fad0f739e711b2842db254bc9a52f6677eba015a84f032a69c4b5a4`, schema `bolt-v3.no-submit-readiness.v2`, generated timestamp `1779373729`, config bundle checksum `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`, and executable identity `ffb56ce27899987b5028e2913dfd203d78297eb89968b99016bdfdb5f5d4ace3`. Stages `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_disconnect`, and `report_write` were satisfied; `controlled_connect` failed; `reference_readiness` was skipped. Runtime evidence showed the command ran from local macOS `SP-MB-Pro.local`, local public IP probe returned `58.232.146.158`, AWS `describe-addresses` showed EIP `34.248.143.2` attached to EC2 instance `i-0b68843392a62e359`, and AWS `describe-instances` showed that instance state `stopped`. Follow-up read-only EC2 access checks showed SSH ingress on security group `sg-08921a4b725682171` only from `59.8.178.135/32` and `118.129.66.2/32`, not the current local IP, and `ssm describe-instance-information` returned no managed-instance record while the instance was stopped. This narrows the current blocker to runner-IP mismatch for the local rerun plus EC2 access precheck before rerunning from the allowed EIP; it still does not produce a satisfied no-submit readiness report. T038 stayed unchecked at that time, and the next T038 proof must run from the allowed EIP or an explicitly allowed runner IP.
  - 2026-05-22 00:39:07 KST approved EC2/EIP T038 rerun at head `1245264f294ae096155bffc3236fb692cc46b46f`: operator approved starting EC2 instance `i-0b68843392a62e359`; AWS reported it `running`, public IP `34.248.143.2`, SSM `Online`. Local `just build` produced a current-head Linux aarch64 binary after clearing repo-owned failed-build cache; verified local and EC2 binary SHA-256 `7ef548c74688fc96ef3f06726df1838fb0742fe59176d386211ba3d680eccdc7`, and EC2 `--help` exposed `no-submit-readiness`. The approved config was staged to EC2 without printing contents; `/tmp/config/live.local.toml` SHA-256 matched `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`, mode `0600`, size `5024`; `/tmp/config/strategies/binary_oracle.example.toml` SHA-256 matched `3961588674c44e2265ad1797856be6e2a4f386ca2c55b7691e4e0f3c500e22b1`. `secrets check` and `secrets resolve` passed on EC2 without printing secret values. `/tmp/bolt-v2-t038-1245264f no-submit-readiness --config /tmp/config/live.local.toml` connected Binance SBE (`Connected: client_id=binance_reference`), Polymarket data/execution, observed reference readiness, disconnected cleanly, and wrote `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json` on EC2. The report had mode `0644`, size `935`, SHA-256 `53b945f92a2c747345ff65fb551ebf337cc4a5b5ab5f9552a92a4c6f68fb4126`, schema `bolt-v3.no-submit-readiness.v2`, generated timestamp `1779377947` (`2026-05-22 00:39:07 KST`), config bundle checksum `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`, executable identity `7ef548c74688fc96ef3f06726df1838fb0742fe59176d386211ba3d680eccdc7`, and all seven stages `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_connect`, `reference_readiness`, `controlled_disconnect`, and `report_write` were `satisfied`. This satisfies T038 only and proves the prior local Binance `Invalid X-MBX-APIKEY header` blocker was runner-IP/allowlist mismatch for the no-submit path.
  - Control-surface side finding from the same EC2 session: starting the instance auto-started pre-existing `bolt-v2.service` (`ExecStart=/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml`, installed binary SHA-256 `4c95cd843f3329e4d267f0c9db91997f9ba8b411be2e9efbe89aab57b4f45078`, installed config SHA-256 `fa7d129c2d17bc6762458b7f48591797a4130ac5d523ab7e09ed340764d3eb06`). This was not the T038 current-head no-submit runner. It was stopped and disabled; follow-up `systemctl` showed `inactive` and `disabled`, and process check showed no `bolt-v2` process. Targeted journal review for the final auto-start window showed the stale service did not start trader because engine clients were not connected (`Not starting trader: engine client(s) not connected`) after a Binance SBE schema mismatch, but it is a production control blocker to resolve before any canary or production trading.
- [x] T039 [US4] Run `cargo test --test bolt_v3_live_canary_gate` against the redacted report fixture shape.

## Phase 8: Tiny-capital Live Canary (US5)

**Goal**: One approved capped live order proves production-shaped bolt-v3 spine through NT.

**Independent Test**: Local tests prove all preconditions and fail-closed paths. Operator artifact proves real submit/venue result/cancel/reconciliation.

- [x] T040 [US5] Write failing canary precondition tests requiring exact config checksum, approval id, gate report, submit admission state, and decision evidence.
- [x] T041 [US5] Write ignored operator test or command harness that submits at most one configured canary order after explicit approval.
- [x] T042 [US5] Implement canary operator harness using the production bolt-v3 path and NT adapter submit only.
- [x] T043 [US5] Add strategy-driven cancel path evidence capture for open canary orders.
- [x] T044 [US5] Add restart reconciliation evidence capture through NT adapter state.
- [x] T045 [US5] Run local fail-closed tests, exact-head CI, no-mistakes triage, and external review after branch is clean and pushed.
- [ ] T046 [US5] With explicit operator approval, run tiny-capital canary and store redacted artifact with exact SHA and config checksum.

## Phase 9: Review Remediation - No-submit Evidence Freshness (US4)

**Goal**: A no-submit readiness report is accepted only when it is fresh, exact-head, config-bound, and stage-complete.

**Independent Test**: `cargo test --test bolt_v3_live_canary_gate -- --nocapture` rejects stale, missing-freshness, wrong-binary, wrong-config, and unsatisfied readiness reports before runner entry.

- [x] T047 [P] [US4] Write failing stale-report rejection tests in `tests/bolt_v3_live_canary_gate.rs` for missing `generated_at_unix_seconds`, expired report age, and report age above TOML-owned `[live_canary].readiness_report_max_age_seconds`.
- [x] T048 [US4] Add TOML-owned readiness report age config in `src/bolt_v3_config.rs`, `config/root.example.toml`, `src/bolt_v3_no_submit_readiness.rs`, and `src/bolt_v3_live_canary_gate.rs`.
- [x] T049 [P] [US4] Write failing no-submit stage-detail tests in `tests/bolt_v3_no_submit_readiness.rs` proving partial connect, skipped reference readiness, and stale reference cache cannot produce a gate-acceptable report.
- [x] T050 [US4] Extend no-submit readiness stage evidence in `src/bolt_v3_no_submit_readiness.rs` and `src/bolt_v3_live_node.rs` so failed NT client connect/reference states stay failed and redacted stage details are preserved.
- [x] T051 [US4] Run `cargo test --test bolt_v3_no_submit_readiness -- --nocapture` and `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.

## Phase 10: Review Remediation - Production Canary Approval Envelope (US5)

**Goal**: Production `Run` enforces the same operator evidence envelope required by the tiny-canary contract, not only harness-local checks.

**Independent Test**: `cargo test --test bolt_v3_live_canary_gate -- --nocapture` and `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture` reject missing operator evidence, invalid approval windows, nonce mismatch, stale approval, SSM manifest mismatch, strategy-input mismatch, financial-envelope mismatch, and pre-run evidence mismatch.

- [x] T052 [P] [US5] Write failing production-gate tests in `tests/bolt_v3_live_canary_gate.rs` requiring `[live_canary].operator_evidence` for production `Run`.
- [x] T053 [P] [US5] Write failing operator-envelope regression tests in `tests/bolt_v3_tiny_canary_operator.rs` for approval window, nonce, SSM manifest hash, strategy-input hash, financial-envelope hash, and pre-run evidence hash.
- [x] T054 [US5] Validate `LiveCanaryOperatorEvidenceBlock` inside `src/bolt_v3_live_canary_gate.rs` before submit admission arms in `src/bolt_v3_live_node.rs`.
- [x] T055 [US5] Update `docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md`, `specs/001-thin-live-canary-path/checklists/production-readiness.md`, and `specs/001-thin-live-canary-path/quickstart.md` with the production-enforced operator evidence fields.
- [x] T056 [US5] Run `cargo test --test bolt_v3_live_canary_gate -- --nocapture` and `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`.

## Phase 11: Review Remediation - Submit Lifecycle Safety (US2)

**Goal**: The canary submit budget cannot strand exposure without an explicit config-owned lifecycle policy.

**Independent Test**: `cargo test --test bolt_v3_submit_admission -- --nocapture` proves entry, replace-submit, exit-submit, and cancel-only decisions follow configured lifecycle semantics and cannot bypass admission.

- [x] T057 [P] [US2] Write failing admission tests in `tests/bolt_v3_submit_admission.rs` for submit intent kind, risk-reducing exit after entry, cancel-only exclusion, and config-owned lifecycle policy.
- [x] T058 [US2] Add submit intent classification to `src/bolt_v3_submit_admission.rs` and `src/strategies/binary_oracle_edge_taker.rs` without adding venue, symbol, or strategy hardcodes.
- [x] T059 [US2] Update `specs/001-thin-live-canary-path/spec.md`, `specs/001-thin-live-canary-path/contracts/live-canary-gates.md`, and `specs/001-thin-live-canary-path/data-model.md` so FR-009 names the accepted lifecycle semantics.
- [x] T060 [US2] Run `cargo test --test bolt_v3_submit_admission -- --nocapture` and source-fence searches for direct `submit_order` bypasses in `src/strategies/` and `src/bolt_v3_archetypes/`.

## Phase 12: Review Remediation - Observability, Secrets, and Ledger Hygiene (US3, US5)

**Goal**: Operator evidence is diagnostic, secret-safe, and tracked by open readiness work.

**Independent Test**: Runtime-capture verification proves PortfolioSnapshot capture is represented, config parsing rejects ambiguous SSM paths, credential redaction tests still pass, and trace docs no longer imply closed issues prove live readiness.

- [x] T061 [P] [US5] Write failing runtime-capture test or verifier fixture in `scripts/test_verify_runtime_capture_yaml.py` for `PortfolioSnapshot` subscription and JSONL spool coverage.
- [x] T062 [US5] Implement PortfolioSnapshot capture or an explicit waiver gate in `src/nt_runtime_capture.rs`, `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml`, and `docs/bolt-v3/research/runtime-capture/bolt-current-capture.yaml`.
- [x] T063 [P] [US3] Write failing SSM path hygiene tests in `tests/config_parsing.rs` rejecting leading/trailing whitespace in `*_ssm_path` TOML values.
- [x] T064 [US3] Reject ambiguous SSM paths in `src/bolt_v3_validate.rs` and keep `src/secrets.rs` byte-exact for resolved secret values.
- [x] T065 [P] [US3] Write or update credential redaction tests in `tests/bolt_v3_credential_log_suppression.rs` proving provider credentials remain redacted and never printed.
- [x] T066 [US3] Replace raw provider credential storage with redacted/zeroizing types in `src/bolt_v3_providers/polymarket.rs` and `src/bolt_v3_providers/binance.rs`, or update docs to stop claiming that hardening if it is intentionally deferred.
- [x] T067 [US5] Update `docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md`, `docs/bolt-v3/2026-05-18-production-readiness-contract.md`, and `specs/001-thin-live-canary-path/checklists/production-readiness.md` so #409 is explicit and #360 closure is not used as proof that T046 is complete.
- [x] T068 [US5] With explicit user approval only, update GitHub issue links or successor tracking for T046 and #409; otherwise record required issue mutation as a blocked operator action in `docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md`.
- [x] T069 [US3] Run `cargo test --test config_parsing -- --nocapture`, `cargo test --test bolt_v3_credential_log_suppression -- --nocapture`, `python3 scripts/test_verify_runtime_capture_yaml.py`, and `python3 scripts/verify_runtime_capture_yaml.py`.

## Phase 13: AI Slop Cleanup and Final Verification

**Goal**: Keep the remediation small, reviewable, TDD-proven, and free of stale AI-generated doc drift.

**Independent Test**: The final diff is scoped to the remediation tasks, each touched behavior has a targeted regression test, and broad source/docs checks pass.

- [x] T070 [P] Run `rg -n "(?i:TODO|fix later|temporary|placeholder|AI|slop|stale|production-ready)" src tests specs/001-thin-live-canary-path docs/bolt-v3/2026-05-20-production-readiness-end-to-end-trace.md` and remove or justify stale prose in touched files.
- [x] T071 Run `cargo fmt --check`, `git diff --check`, all targeted cargo tests from T051/T056/T060/T069, and `just source-fence` if available.
- [x] T072 Run final spec-compliance and code-quality reviews for the remediation diff before claiming readiness status.
- [x] T073 [US5] Fix review-discovered quickstart evidence-binding drift in `specs/001-thin-live-canary-path/quickstart.md`: head binding must name `[live_canary.operator_evidence].head_sha` plus build-owned head, root TOML hash must be checked via `approval_consumption_path`, and `approval_envelope_path` must not be described as read by the production gate.
- [x] T074 [US5] After T073, run `git diff --check` and `just source-fence`, then rerun scoped config/schema/docs re-review before push or external review.
- [x] T075 [US5] Diagnose exact-head PR #388 CI clippy and gate failures with `gh pr checks 388 --repo seungpyoson/bolt-v2` plus failed job logs before editing `build.rs`.
- [x] T076 [US5] Fix the exact clippy failures in `build.rs` and `src/bolt_v3_live_canary_gate.rs`, run `just clippy`, `git diff --check`, and `cargo test --test bolt_v3_live_canary_gate -- --nocapture`.
- [x] T077 [US5] Push the clippy-fix commit, re-check PR #388 exact-head CI is green, then start external quorum review.
- [x] T078 [US5] Review all Greptile PR #388 comments, then either patch and reply/resolve or disprove each comment with file/line and test evidence.
- [x] T079 [US5] Fix external-review consensus safety hardening in `src/bolt_v3_live_canary_gate.rs`, `src/bolt_v3_live_node.rs`, `tests/bolt_v3_live_canary_gate.rs`, and `tests/bolt_v3_tiny_canary_operator.rs`: reject symlinked no-submit readiness reports, fail closed on no-submit timeout sum overflow, and remove duplicate JSON keys from the unapproved-strategy-hash test.
- [x] T080 [US5] Reconcile external-review docs/ledger concerns: Chainlink provider availability vs runtime contracts, Phase 8 harness-only `BOLT_V3_PHASE8_*` env vars, PR #331 historical-anchor wording, P5 cadence-table wording, and runtime-contract freshness wording.
- [x] T081 [US5] After pushing T079, re-check PR #388 exact-head CI and retry Kimi on narrowed exact-head shards because the prior full-diff Kimi run timed out.
- [x] T082 [US5] Address Greptile post-`49d8ea3a` comments: remove broad `.git` build-script rerun trigger, align live-canary approval-consumption test fixture timestamps, and reply/resolve or disprove remaining review threads.
- [x] T083 [US5] Address Greptile post-`26c83db4` P1 comments: bind `approval_envelope_path` content via `approval_envelope_sha256`, bind client/venue order hashes into approval-consumption proof validation, and avoid re-aging approval consumption during late TOCTOU revalidation while still checking the approval window.
- [x] T084 [US5] Address local code-quality review follow-up: add a call-site regression test proving late live-canary gate revalidation still rejects an expired operator approval window without re-aging approval-consumption freshness.
- [x] T085 [US5] Address Greptile post-`5a88275b` comments: remove redundant required-stage condition, avoid stale no-submit readiness metadata fixtures, and surface `readiness_report_max_age_seconds` on `BoltV3LiveCanaryGateReport`.
- [x] T086 [US5] Address Greptile post-`ad4f2557` comments: move example approval-window timestamps into the future and document that the operator approval window must cover report validation plus late evidence re-read/re-hash latency.
- [x] T087 [US5] Address Greptile post-`d9258272` style finding: remove dead `satisfied_stage_names` bookkeeping from no-submit readiness required-stage validation.
- [x] T088 [US5] Address Greptile post-`ef72cd7b` style findings: clarify the standalone approval-consumption window guard and make bounded-read cap errors caller-neutral, then reply with the current `read_report_bytes_with_limit` call-path evidence.
- [x] T089 [US5] Address Greptile post-`a3314e8e` documentation findings: document the second operator-evidence re-read/re-hash intent and late timestamp freshness headroom, then reply with code/doc evidence.
- [x] T090 [US5] Address external-review checksum/docs drift: align `config_hash` prose with the actual framed `config_bundle_checksum`, clarify Chainlink provider availability vs runtime-contract target, mark `BOLT_V3_PHASE8_*` values as harness-only, clarify accepted updown cadence-table semantics, and align live-canary byte-cap examples.
- [x] T091 [US5] Add TDD regression coverage proving a missing operator evidence file fails closed as `OperatorEvidenceRead` before hashing.
- [x] T092 [US5] Add TDD regression coverage proving Phase 8 sha256 helpers reject uppercase hex and align helper behavior with the production live-canary gate.
- [x] T093 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run external-review consensus for the T080/T090-T092 slice.
- [x] T094 [US5] Address shared Kimi/DeepSeek external-review test gap by adding production live-canary gate regression coverage for 64-character uppercase operator-evidence hash rejection.
- [x] T095 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T094 coverage slice.
- [x] T096 [US5] Address Greptile current-head P1: stamp no-submit readiness `generated_at_unix_seconds` after controlled connect/reference/disconnect stages, immediately before report creation/write.
- [x] T097 [US5] Address Greptile current-head protocol-literal style finding: extract approval-consumption schema version and record-kind validation literals into named constants.
- [x] T098 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T096-T097 Greptile slice.
- [x] T099 [US5] Address external-review T096 test-hardening note: make the no-submit freshness source-shape guard pin `current_unix_seconds()` after controlled stages, not only the final report-builder call.
- [x] T100 [US5] Address Greptile current-head submit-lifecycle observation: investigate and either correct or formally constrain `ReplaceSubmit` counter semantics so replaces cannot inflate risk-reducing-exit budget in any supported multi-order configuration.
- [x] T101 [US5] Address Greptile current-head submit-admission serialization observation: investigate and either shorten the admission mutex critical section without weakening counter/evidence atomicity, or encode the serialization requirement with explicit tests and docs.
- [x] T102 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T099-T101 submit-admission/test-hardening slice.
- [x] T103 [US5] Address Greptile current-head tiny-canary proof incompatibility: make the Phase 8 approval-consumption writer emit the gate-required `approval_envelope_sha256`, `client_order_id_hash`, and `venue_order_id_hash` fields, with writer-to-gate regression coverage.
- [x] T104 [US5] Address Gemini T103 test-gap finding: add regression coverage proving an approval-consumption proof written by the Phase 8 harness is accepted by the actual live-canary gate.
- [x] T105 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T103/T104 proof-compatibility slice.
- [x] T106 [US5] Address Greptile current-head gate hardening findings: move approval-consumption root TOML checksum to async bounded I/O and bind configured `strategy_cancel_path` into approval-consumption proof with regression coverage.
- [x] T107 [US5] Address T106 read-only review findings: prevent Phase 8 `strategy_cancel_path` env/TOML drift from spending approval, add async bounded-reader behavior coverage, and make source-shape tests repo-root anchored.
- [x] T108 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, reply to Greptile inline findings, and run targeted external-review consensus for the T106/T107 gate-hardening slice.
- [x] T109 [US5] Address T108 external-review hardening notes: align Phase 8 SHA-256 shape validation with the live gate lowercase-only policy, add sync bounded config reader regression coverage, and reject parent-directory traversal in live-canary gate configured paths.
- [x] T110 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T109 hardening slice.
- [x] T111 [US5] Address T110 external-review hardening notes: consolidate duplicate Phase 8 SHA-256 shape helpers, reject parent-directory traversal in Phase 8 env-owned paths including optional `strategy_cancel_path`, add exact-limit bounded config reader coverage, add direct `strategy_cancel_path` traversal coverage, and regular-file-check bounded config reads.
- [x] T112 [US5] Verify, push, re-check PR #388 exact-head CI/Greptile, and run targeted external-review consensus for the T111 hardening slice.

## Phase 14: T046 Pre-run Gate Alignment (US5)

**Goal**: Remove the T046 approval-consumption ordering blocker without weakening production submit admission.

**Independent Test**: Local tests prove preflight accepts a missing approval-consumption proof only before live runner entry after all other gate inputs validate, production gate still requires the proof, an already-present proof fails closed before runner entry, and env/TOML approval-consumption path drift fails closed.

- [x] T113 [US5] Address the T046 approval-consumption ordering blocker: preflight may defer only the missing consumption proof before live runner entry, production gate must still require a valid proof, existing pre-run consumption proof must fail closed, invalid readiness reports must still fail closed, and env/TOML `approval_consumption_path` drift must fail closed.
  - 2026-05-22 local evidence on uncommitted current diff: `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture` passed `28 passed; 0 failed; 1 ignored`; `cargo test --test bolt_v3_live_canary_gate -- --nocapture` passed `66 passed; 0 failed`; `cargo test --test bolt_v3_no_submit_readiness -- --nocapture` passed `33 passed; 0 failed`; `cargo fmt --check` and `git diff --check` passed; new-line slop scan found no `TODO`, `fix later`, `AI slop`, `temporary`, or `placeholder` hits in added lines.
- [x] T114 [US5] Verify T113 with targeted local suites, formatting, diff/slop scans, exact-head PR state/CI, successful available external-review approvals, and explicit failed-slot evidence for unavailable reviewers.
  - 2026-05-22 exact-head evidence at PR #388 head `a8831d4cb7a7f7dd84694533b8e6370fe7d12550`: `gh pr view 388 --repo seungpyoson/bolt-v2 --json state,headRefOid,baseRefOid,mergeStateStatus,url` reported `state=OPEN`, `mergeStateStatus=CLEAN`, base `831368756bf5a7f8398944502dcce5fcc7c7952d`; `gh pr checks 388 --repo seungpyoson/bolt-v2` reported pass for Analyze, CodeQL, actionlint, check-aarch64, clippy, deny, detector, fmt-check, gate, nextest archive, nextest shards 1-4, source-fence, and test. Local verification passed `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture`, `cargo test --test bolt_v3_tiny_canary_operator -- --nocapture`, `cargo test --test bolt_v3_live_canary_gate -- --nocapture`, `cargo test --test bolt_v3_no_submit_readiness -- --nocapture`, `cargo fmt --check`, `git diff --check`, `just source-fence`, `just fmt-check`, and `just test` (`719 passed, 2 skipped`). External approvals: Gemini job `54b2e7a4-e434-44c5-a1cb-960cb07f851b`, GLM job `job_777ce632-8ca3-4532-8251-08315934c5a6`, Kimi jobs `21c87160-8b8a-4350-8254-48911651d290` and `774f2d45-cdd9-401c-bbce-678e791ca67d`. Failed slots were recorded, not counted as approvals: Claude subscription/OAuth jobs `cf6ca996-e3c9-4937-bcbb-6ffce0b5ec36`, `f76377b3-78ba-445b-87ca-dcf63ecb8c68`, and `e4afa0bb-e6bd-4c35-92fd-866aeaca9dcc` failed with session parse errors after source send; DeepSeek jobs `job_9f03744d-83d9-4d49-bb8e-5a3a0cd0d968` and `job_79f8887f-d8ef-44fb-9e4d-bc6efe79f46f` failed with provider/key availability errors.
  - 2026-05-22 doc-ledger follow-up at PR #388 head `6073950a01435cd1a87dae05a77c73677794b673`: `gh pr view 388 --repo seungpyoson/bolt-v2 --json state,headRefOid,baseRefOid,mergeStateStatus,url` reported `state=OPEN`, `mergeStateStatus=CLEAN`; `gh pr checks 388 --repo seungpyoson/bolt-v2` reported the same required checks passing after the T114 evidence commit.
- [ ] T115 [US5] After T113/T114 are committed and pushed, prepare a fresh exact-head T046 operator packet binding current binary identity, root config SHA, config bundle checksum, EC2/EIP no-submit report, SSM manifest, strategy-input evidence, financial envelope, pre-run state, abort plan, nonce, and approval envelope.
  - 2026-05-22 partial T115 evidence at PR #388 head `6073950a01435cd1a87dae05a77c73677794b673`: `just build` produced Linux aarch64 binary `/Users/spson/.cache/rust-verification/bolt-v2/target/aarch64-unknown-linux-gnu/release/bolt-v2`, SHA-256 `e4eca7ab15b3d5e50cf332fe9c95e7f6971d09c73158cf45bb7d09e190cc8241`; EC2 `i-0b68843392a62e359` in `eu-west-1` was running with EIP `34.248.143.2`, SSM `Online`, and `bolt-v2.service` remained `inactive`/`disabled`; `/tmp/config/live.local.toml` SHA-256 `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`, mode `0600`, and strategy config SHA-256 `3961588674c44e2265ad1797856be6e2a4f386ca2c55b7691e4e0f3c500e22b1` matched local evidence; staging command `9b960563-0b54-4fc3-9b14-24bef81a89e4` verified EC2 binary `/tmp/bolt-v2-t115-6073950a` SHA-256 `e4eca7ab15b3d5e50cf332fe9c95e7f6971d09c73158cf45bb7d09e190cc8241`; no-submit command `49519a93-2633-4efe-ae49-9f7470b4441f` ran `secrets check`, `secrets resolve`, and `no-submit-readiness` from EC2 without printing secret values and wrote report `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json`, mode `0644`, size `935`, SHA-256 `1c015d44b99c380cf1234632360c3591efdacbedfae2b1684ec67417f1f6c33d`, schema `bolt-v3.no-submit-readiness.v2`, generated timestamp `1779390498`, config bundle checksum `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`, executable identity `e4eca7ab15b3d5e50cf332fe9c95e7f6971d09c73158cf45bb7d09e190cc8241`, and all seven stages satisfied. T115 remains unchecked because there is still no current-head SSM manifest artifact, strategy-input safety evidence, financial envelope, pre-run state evidence, abort plan, approval nonce, approval envelope, or configured `[live_canary.operator_evidence]` block in `config/live.local.toml`.
- [ ] T116 [US5] With explicit operator approval, execute T046 tiny-capital canary and store the redacted artifact with exact SHA and config checksum; keep production-readiness checklist unchecked until staged-live and production gates have independent proof.

## Out Of Scope For MVP

- Backtesting engine.
- Research analytics platform.
- New Bolt-owned venue adapter.
- Bolt-owned order lifecycle or reconciliation implementation.
- Test-literal verifier expansion.

## Execution Order

Phase 1 must merge first. Phases 2-8 are sequential because each removes a live-submit blocker from the prior phase. Do not begin live operations until Phases 2-7 are complete and verified.

Review remediation phases 9-13 are source/test/doc tasks only unless T038, T046, or T068 receive explicit user approval. Implement in this order: Phase 9 freshness and no-submit truthfulness, Phase 10 production operator evidence, Phase 11 lifecycle safety, Phase 12 observability/secrets/ledger hygiene, Phase 13 cleanup and verification. T047, T049, T052, T053, T057, T061, T063, T065, and T070 are parallelizable only when workers touch disjoint files.
