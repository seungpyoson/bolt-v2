# AI Slop Cleanup Report

Scope: PR #331 merge-owned edits and Phase 9 review artifacts only.

Behavior Lock:

- `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture` compiled and exposed a real stale-fixture failure: preflight expected `AcceptedByGate`, but the merged fixture used old no-submit readiness linkage keys and the P6 gate correctly returned `RejectedByGate`.
- Behavior to preserve: live-canary gate must validate readiness linkage, including approval hash, executable identity, and config bundle checksum. Fixtures must satisfy the contract; production gate must not be weakened.
- After fixture/schema fixes, focused P6 tests passed: `bolt_v3_tiny_canary_preconditions` 54 passed, `bolt_v3_live_canary_gate` 32 passed, `bolt_v3_no_submit_readiness` 21 passed, and `bolt_v3_strategy_registration` 6 passed.

Cleanup Plan:

1. Dead code/conflict residue: delete only proven merge markers, stale conflict alternatives, or compiler-proven unused residue.
2. Duplication: defer helper consolidation until focused P6 tests are green.
3. Naming/error handling: update only names required by current schema, TOML field contract, or failing tests.
4. Test reinforcement: prefer public behavior tests around gate/readiness/preflight contracts.

## Categorized Issues

| Category | Finding | Current Disposition |
| --- | --- | --- |
| Boundary violation | Treating current failure as a generic P6 patch hides whole PR #331 P0-P9 obligation. | Corrected in `plan.md` and `tasks.md`; P7-P9 remain explicit. |
| Missing test lock | Merge conflict resolution touched tiny-canary/readiness code before refreshed plan/tasks were explicit. | Stop condition added; targeted tests drive remaining semantic edits. |
| Stale fixture | `tests/bolt_v3_tiny_canary_preconditions.rs` wrote legacy no-submit readiness fields. | Fixed by updating fixture to current linkage schema; gate stayed fail-closed. |
| Schema mismatch | Archetype mapping emitted `price_to_beat_source`, but `BinaryOracleEdgeTakerConfig` did not accept it. | Fixed by adding `price_to_beat_source` to the runtime strategy config field list; strategy registration test passed. |
| Dual-path residue | Merge carried a `no_contract_mode_behaves_as_before` test, but current lake conversion requires a `VenueContract`. | Removed stale no-contract test to preserve one contract-backed conversion path. |
| Template overwrite | `setup-plan.sh --json` copied a template over `specs/013-production-live-readiness/plan.md`. | Restored filled plan content from staged merge state. |
| Scope confusion | Active SpecKit pointer now targets `specs/013-production-live-readiness/`, while PR #331 Phase 9 execution state lives under `specs/021-bolt-v3-phase9-current-main-audit/`. | Both are documented: 013 is downstream/main production-readiness surface; 021 is PR #331 completion plan. |

## Passes Completed

1. Pass 1: Dead code deletion - no deletion made; no dead conflict residue proven yet.
2. Pass 2: Duplicate removal - deferred; no duplicate helper consolidation needed for P6 green.
3. Pass 3: Naming/error handling cleanup - limited to schema-aligned fixture/update work.
4. Pass 4: Test reinforcement - focused P6 tests now green.

## Quality Gates

- Focused P6 tests: PASS.
- Full tests: PASS, `just test` ran 665 tests, 665 passed, 2 skipped.
- Format/literal/provider checks: PASS, `just fmt-check`.
- Source-fence: PASS, `just source-fence`.
- Clippy: PASS, `just clippy`.
- Build: PASS, `just build`.
- Dependency bans: PASS, `just deny` ended with `bans ok`; duplicate warnings remain warning-class output.
- Diff hygiene: PASS, `git diff --check`.
- Conflict markers: PASS, repository marker scan returned no matches.

## Changed Files In This Cleanup Pass

- `specs/021-bolt-v3-phase9-current-main-audit/plan.md` - regenerated current PR #331 completion plan.
- `specs/021-bolt-v3-phase9-current-main-audit/tasks.md` - regenerated executable P6-P9 task list.
- `specs/021-bolt-v3-phase9-current-main-audit/ai-slop-cleanup-report.md` - replaced stale "no cleanup performed" report with current behavior-lock cleanup plan.
- `specs/013-production-live-readiness/plan.md` - restored filled plan after SpecKit setup copied template.
- `tests/bolt_v3_tiny_canary_preconditions.rs` - updated no-submit readiness fixture to current linkage schema.
- `src/bolt_v3_live_canary_gate.rs` - removed silent default construction in linkage mismatch reporting.
- `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` - updated current source-fence allowlist for schema/linkage/config-bundle/tiny-canary literals.
- `src/strategies/binary_oracle_edge_taker.rs` - accepted mapped `price_to_beat_source` runtime config field.
- `tests/venue_contract.rs` - removed stale no-contract mode test incompatible with current contract-required API.

## Remaining Risks

- P7-P9 remain pending.
- Merge commit not yet created.
- PR #331 exact-head CI not yet verified after push.
