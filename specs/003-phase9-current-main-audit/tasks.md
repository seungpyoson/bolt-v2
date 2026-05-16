# Tasks: Phase 9 Current-main Audit

**Input**: Design documents from `/specs/003-phase9-current-main-audit/`
**Prerequisites**: Current-main worktree, literal coverage, policy coverage, verifier runs, roadmap inspection.
**Mode**: Audit-only. Runtime implementation tasks require separate approval.

## Phase 1: Fresh-main Anchor

- [x] T001 Fetch and prune origin.
- [x] T002 Verify original audit anchor `HEAD`, `main`, and `origin/main` were all `23acab30b73990302765ea441550fabcbf03f570`.
- [x] T003 Create and use worktree `.worktrees/022-bolt-v3-phase9-current-main-audit`.
- [x] T004 Record PR #328 merge provenance.

## Phase 2: Required Coverage

- [x] T005 Run literal coverage scan and store output at `/private/tmp/bolt-v3-phase9-literal-coverage.txt`.
- [x] T006 Run policy coverage scan and store output at `/private/tmp/bolt-v3-phase9-policy-coverage.txt`.
- [x] T007 Inspect `scripts/verify_bolt_v3_runtime_literals.py`.
- [x] T008 Inspect `scripts/verify_bolt_v3_provider_leaks.py`.
- [x] T009 Inspect `scripts/verify_bolt_v3_core_boundary.py`.
- [x] T010 Inspect `scripts/verify_bolt_v3_naming.py`.
- [x] T011 Inspect roadmap and status docs under `docs/bolt-v3/` and prior specs under `specs/001-*` and `specs/002-*`.

## Phase 3: Classification

- [x] T012 Classify config-owned strategy/runtime values.
- [x] T013 Classify production runtime residuals.
- [x] T014 Classify policy hardcodes and fail-closed scope constraints.
- [x] T015 Classify schema/protocol labels and NT/API glue.
- [x] T016 Classify diagnostic strings and test fixtures.
- [x] T017 Classify stale docs/specs.
- [x] T018 Classify debt/AI-slop markers.
- [x] T019 Classify SSM-only and pure Rust runtime evidence.
- [x] T020 Classify runtime-capture failure concern.

## Phase 4: Audit Artifacts

- [x] T021 Write Phase 9 spec.
- [x] T022 Write Phase 9 plan.
- [x] T023 Write Phase 9 tasks.
- [x] T024 Write Phase 9 requirements checklist.
- [x] T025 Write Phase 9 audit report.

## Proposed Follow-up Tasks Requiring Approval

- [x] T026 Push audit branch and open draft PR #331 without merge.
- [x] T027 Confirm exact-head PR CI green before external review: CI run `25855655415` passed detector, fmt-check, deny, test, clippy, and gate.
- [x] T028 Launch Claude review. Disposition: branch-diff selected only Phase 9 docs, so it is docs-artifact review only, not full-source coverage.
- [x] T029 Launch Gemini review. Disposition: branch-diff selected only Phase 9 docs, so it is docs-artifact review only, not full-source coverage.
- [x] T030 Complete DeepSeek direct-API review over full required bundle shards; approval-request showed `source_content_transmission: not_sent` before each approved run.
- [x] T031 Complete GLM direct-API review over full required bundle shards; approval-request showed `source_content_transmission: not_sent` before each approved run.
- [x] T032 Add explicit source dispositions for archetypes, provider registry, Binance provider, client registration, strategy registration, live canary gate, submit admission, and tiny-canary evidence.
- [x] T033 Move or explicitly re-accept `src/bolt_v3_providers/polymarket.rs:537` `auto_load_debounce_ms: 100`; behavior lock: runtime literal verifier plus provider-binding test. Disposition: moved to TOML as `auto_load_debounce_milliseconds`, verified by provider-binding and config validation tests, with DeepSeek/GLM pre/post reviews complete.
- [x] T034 Remove `src/bolt_v3_validate.rs` one-venue-per-kind architecture cap so configured `[venues.<id>]` keys own routing; behavior lock: config validation accepts multiple same-kind venue keys and provider validation still runs per key. Current disposition: local implementation removed the global provider-kind count gate; targeted red/green config test passes; Gemini/Claude/DeepSeek/GLM post-implementation review at `b897dd6` approved with no blockers.
- [x] T035 Decide whether legacy/shared config default surfaces in `src/live_config.rs`, `src/config.rs`, and legacy clients are non-bolt-v3 or must be retired; behavior lock: source fence proving bolt-v3 production entrypoint cannot load legacy defaults. Disposition: retired from current source and source-fenced by `tests/bolt_v3_production_entrypoint.rs`, with prior DeepSeek/GLM pre/post reviews complete.
- [x] T036 Add an integrated regression test for `run_bolt_v3_live_node` capture-failure notification while `node.run()` is active; behavior lock: failing test before implementation if a bugfix is approved. Disposition: red/green helper regression preserves the live-node run future after capture notification and avoids false capture-failure logging on closed notification, with DeepSeek/GLM pre/post reviews complete.
- [x] T037 Refresh or supersede stale status/spec docs that still claim missing entrypoint or missing provider verifier; behavior lock: doc/source consistency check against `src/main.rs` and verifier scripts. Disposition: status-map rows refreshed and `scripts/verify_bolt_v3_status_map_current.py` added, with DeepSeek/GLM pre/post reviews complete.
- [x] T038 Add a dedicated pure-Rust-runtime verifier if the rule remains tracked as a proven gate; behavior lock: source scan for PyO3, maturin, Python runtime invocation, and AWS CLI subprocess. Disposition: `scripts/verify_bolt_v3_pure_rust_runtime.py` added and status-map row 3 refreshed, with DeepSeek/GLM pre/post reviews complete.
- [x] T039 Move `src/bolt_v3_tiny_canary_evidence.rs` one-live-order cap out of code ownership; behavior lock: focused Phase 8 evidence tests plus literal verifier classification. Current disposition: preflight uses `[live_canary].max_live_order_count` from TOML, and live proof accepts a positive admitted submit count up to that TOML-derived cap while rejecting zero or above-cap evidence; targeted tests and runtime literal verifier pass.
- [x] T040 Generalize `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` order-shape policy; behavior lock: archetype validation and NT runtime strategy tests. Current disposition: local implementation removes hardcoded entry/exit combo rejection and projects TOML order shape into nested `entry_order`/`exit_order` strategy config tables consumed by the existing NT strategy order factory path; targeted tests pass; Gemini/Claude/DeepSeek/GLM post-implementation review at `b897dd6` approved with no blockers. DeepSeek's raw-direct-caller note is accepted as non-blocking because production projection is typed and strategy order construction fails closed on unsupported strings.
- [x] T060 Move updown cadence slug-token ownership out of Rust code; behavior lock: `cadence_slug_token` is required in TOML, non-table cadence values can build instrument filters when paired with a configured token, and the runtime literal verifier rejects stale table allowlist entries. Current disposition: local implementation removes the cadence-to-token table, minute-divisibility gate, and 32-character underlying bound; targeted instrument-filter/config/provider-binding tests and runtime literal verifier pass; Gemini/Claude/DeepSeek/GLM post-implementation review at `b897dd6` approved with no blockers. Token-misconfiguration risk remains a documented operator/config risk, not a code-owned table.
- [x] T061 Generalize validation dispatch seams for provider, market-family, and strategy-archetype registries; behavior lock: injected fake provider/family/archetype validation bindings work without editing production registry tables. Current disposition: local implementation adds injectable validation-binding paths and red/green tests for all three registry layers; static production binding labels remain explicit current-slice dispatch glue; Gemini/Claude/DeepSeek/GLM post-implementation review at `b897dd6` approved with no blockers.
- [x] T062 Move provider WebSocket `transport_backend` ownership out of NT defaults and into TOML; behavior lock: Polymarket data, Polymarket execution, and Binance data adapter mapping tests assert configured backend values reach NT config structs. Current disposition: local implementation makes `transport_backend` required in provider data/execution TOML and maps it through to NT configs; Gemini/Claude/DeepSeek/GLM post-implementation review at `b897dd6` approved with no blockers. Required-field upgrade impact is accepted and documented.
- [x] T063 Remove strategy pricing fallback from selected fast venue to reference fair value and prevent cross-market position pricing; behavior lock: reference fair value remains logged separately, entry pricing fails closed with `SpotPriceMissing` unless a configured fast venue clears lead-quality selection, and position EV cannot use the active market's fast spot after rotation. Current disposition: local implementation removes the fallback path, renames the log field away from fallback semantics, and requires managed-position market id to match the active market before using active fast spot; targeted red/green strategy and runtime tests pass; Gemini/Claude/DeepSeek/GLM post-implementation review at `b897dd6` approved with no blockers.
- [x] T064 Remove strategy outcome-side inference from hardcoded instrument-id suffixes; behavior lock: source fence rejects production `-UP.`/`-DOWN.` suffix parsing and a position event without pending/active context does not infer side from text. Current disposition: accepted from Gemini S3, implemented red/green locally, exact-head CI passed at `bf2ad6f`, Gemini/Claude/DeepSeek/GLM follow-up external review approved with no blockers, and final narrow DeepSeek/GLM follow-up review approved after `535f973` CI passed.
- [x] T065 Remove legacy `validate_live_local` `.POLYMARKET` instrument-id pin; behavior lock: `polymarket.instrument_id` accepts any non-empty NT `symbol.venue` suffix and still rejects missing/empty components. Current disposition: accepted from S5 legacy-validator review, implemented red/green locally, exact-head CI passed at `bf2ad6f`, Gemini/Claude/DeepSeek/GLM follow-up external review approved with no blockers, Claude/GLM's empty-venue and whitespace-venue test gap is addressed, and final narrow DeepSeek/GLM follow-up review approved after `535f973` CI passed.
- [x] T066 Remove active Bolt-v3 adapter `0_i64` clock sentinel; behavior lock: active adapter mapping derives `InstrumentFilterConfig` from strategy TOML, passes an NT `LiveClock` timestamp source, and the runtime-literal audit has no stale sentinel allowance. Current disposition: accepted from S5 zero-clock sentinel review, implemented red/green locally, exact-head CI passed at `bf2ad6f`, Gemini/Claude/DeepSeek/GLM follow-up external review approved with no blockers, and final narrow DeepSeek/GLM follow-up review approved after `535f973` CI passed.
- [x] T045 Review Gemini Code Assist PR comments. Disposition: accepted and fixed AI-slop evidence, bounded config reads, and expired fair-probability fail-closed behavior.
- [x] T046 Review Greptile PR/check surfaces. Disposition: accepted and fixed Greptile P2 diagnostic wording finding on oversized config reads.
- [x] T047 Bound user-configurable TOML file reads across legacy runtime config, live-local materialization, and the active Bolt-v3 root/strategy loader. Behavior lock: oversized config tests.
- [x] T048 Make expired fair-probability computation fail closed. Behavior lock: `fair_probability_helper_fails_closed_when_expired`.
- [x] T049 Rerun policy/debt marker scan with AI-slop marker terms and update proof-command evidence.
- [x] T050 Refresh branch onto current `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`; only upstream deltas from the audit anchor were `.github/workflows/stale.yml` and `.github/workflows/summary.yml`.
- [x] T051 Update Phase 9 spec, plan, checklist, and audit report to distinguish original audit source anchor from refreshed final base.
- [x] T054 Run `no-mistakes` on the Phase 9 head and disposition its findings.
- [x] T055 Fix no-mistakes BV3-P9-001 by aligning the active Bolt-v3 schema doc/examples with required `auto_load_debounce_milliseconds`.
- [x] T056 Fix no-mistakes BV3-P9-002 by treating oversized or invalid generated `live.toml` output as drift that the materializer rewrites. Behavior lock: `materialize_live_config_updates_oversized_drifted_output`.
- [x] T057 Fix no-mistakes BV3-P9-003 by expanding the pure-Rust runtime verifier to include runtime-capture and strategy modules, and wire the new Phase 9 verifiers into `just fmt-check`.
- [x] T059 Disposition no-mistakes BV3-P9-CONFIG-001 by documenting the 1 MiB pre-parse operator-config size guard as a resource-exhaustion guard, not trading policy.

## Completion Gate

- [x] T041 Run markdown/diff verification after doc creation.
- [x] T042 Commit audit-only artifacts if verification passes.
- [x] T043 Run final verification for remediation follow-up: targeted tests, `git diff --check`, `cargo fmt --check`, four Bolt-v3 verifiers, and `cargo clippy --all-targets -- -D warnings` passed.
- [x] T044 Commit and push remediation follow-up.
- [x] T052 Run final verification after current-main refresh and artifact updates.
- [x] T053 Commit and push current-main refresh follow-up.
- [x] T058 Rerun local final verification for the no-mistakes follow-up before push.
