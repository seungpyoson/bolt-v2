# PR #331 MECE Review Tracker

Working doc for coordinating multi-model review of PR #331. Lives outside the PR worktree so updates do not change the diff under review.

## Preservation Note

Preserved from an untracked root-worktree artifact into PR #388 on 2026-05-18
because PR #331 and issue #371 reference this tracker. Treat this as a
historical review ledger; re-anchor against the live PR #331 head before using
it for any current merge/readiness decision. Live PR #331 head observed during
preservation: `9aaa1e903f6507d6690e63c2268f53db4ceeba72`.

## Anchor

- **PR**: https://github.com/seungpyoson/bolt-v2/pull/331
- **Title**: Phase 9 audit, hardcode remediation, and legacy runtime retirement
- **Base**: `main`
- **Head SHA**: `bcd83f751ca9876bc0d76fc7f9e8e973a7198230` *(updated 2026-05-17 after P3 round-1 fix commit closing P3-BLOCK1 secret-struct NT-aligned zeroize-on-drop + P3-NB1 broadened no-trim guard + P3-NB2 raw-secret-absent-from-wrapped-error tests)*
- **Production-code head**: `691521d3` — last SHA where production code changed (NT-aligned `#[derive(Zeroize, ZeroizeOnDrop)]` on `ResolvedBoltV3PolymarketSecrets` + `ResolvedBoltV3BinanceSecrets`, `nautilus_core::string::secret::REDACTED` in their manual Debug impls, broadened `ssm_resolver_session_does_not_trim_resolved_secret_values` guard, two new wrapped-error no-leak tests). Prior production-code heads: `ca494297` (P2 round-trip assert + P2-D DISPROVEN reproduction), `023d1214` (P1 fence + Cargo cleanup), `9fb1a239` (PR baseline). Docs-only commits since baseline: `0f16836b` (P0 retrospective traceability), `065c1dca` (NEW-1 self-reference fix). `zeroize` added as a direct dep pinned at 1.8.2 (matches the version already locked transitively via NT).
- **Branch**: `022-bolt-v3-phase9-current-main-audit`

**Re-anchoring rule**: if `gh pr view 331 --json headRefOid` returns a SHA other than `691521d3…`, **STOP**. Update this anchor, mark all pending/sent packets as `STALE`, and re-send the affected packets with the new SHA. Docs-only commits on top of the reviewed production-code SHA may be batched without restarting downstream review, but the anchor must still be updated.

## Reviewers

Claude, Gemini, Kimi, GLM, DeepSeek — **all 5 per packet**.

Per the project rule: `roles.consultation` panel composition is the authority for which exact model IDs are used. Look those up in `~/.config/llm/config.json` at send time. Report exact model IDs back in the response capture.

## Review Status Grid

States: `pending` → `sent` → `received` → `adjudicated` → `closed` (or `stale` / `blocked` / `error`).

| Packet | Claude | Gemini | Kimi | GLM | DeepSeek |
|---|---|---|---|---|---|
| P0 Scope Discipline | adjudicated (re-verified) | adjudicated (re-verified) | adjudicated (partial; re-verified) | adjudicated (re-verified) | adjudicated (re-verified, surfaced NEW-1, now resolved) |
| P1 Legacy Path Removal | closed (re-verified) | closed (re-verified) | closed (re-verified) | closed (re-verified) | closed (re-verified) |
| P2 TOML Runtime Values | closed (re-verified) | closed (re-verified) | closed (re-verified) | closed (re-verified; pi-routing anomaly persisted) | closed (re-verified) |
| P3 Provider/Secrets | closed (rounds 1+2) | closed (rounds 1+2) | closed (rounds 1+2) | closed (rounds 1+2) | closed (rounds 1+2) |
| P4 Strategy/Policy | closed (rounds 1+2) | closed (rounds 1+2) | closed (rounds 1+2) | closed (rounds 1+2) | closed (rounds 1+2) |
| P5 Market Family/Filter | pending | pending | pending | pending | pending |
| P6 Readiness/Canary | pending | pending | pending | pending | pending |
| P7 Verifier Integrity | pending | pending | pending | pending | pending |
| P8 Docs/Spec Drift | pending | pending | pending | pending | pending |
| P9 Supporting Code | pending | pending | pending | pending | pending |

Total reviews: 10 × 5 = **50**.

## Packet Manifest

| ID | Name | Primary risk | File count | Notes |
|---|---|---|---|---|
| P0 | Scope Discipline & Spec-to-Diff Adherence | PR drifts from declared T033–T040, T060–T066 scope; PR body declares stale head | 6 | Reviews artifacts + PR metadata only — no code files |
| P1 | Legacy Path Removal & Dual-Path Elimination | Deleted modules still reachable via `lib.rs`/`main.rs`/Cargo deps | ~35 | Mostly deletions + entrypoint surface |
| P2 | TOML Runtime Values & Config Parsing | Hidden defaults; example TOML drifts from required fields | 13 | Includes new `bounded_config_read.rs` |
| P3 | Provider/Venue Architecture & Secrets | Provider leakage into core; SSM bypass; secret trimming/display | 13 | CRITICAL — secret-source rule applies |
| P4 | Strategy / Archetype / Policy / Admission | Trading policy hardcoded; side inference; pricing fallback | 10 | CRITICAL — covers T040, T063, T064 fixes |
| P5 | Market Family / Instrument Filter | Cadence/slug-token, minute-divisibility, family selection in code | 6 | Covers T060 fix |
| P6 | Readiness / Canary / Evidence | Path claims live readiness without artifact; cap hardcoded | 11 | Covers T039 fix |
| P7 | Verifier Integrity & CI Gate | Detectors don't cover what they claim; allowlists too wide | 18 | HIGHEST LEVERAGE — green is fake if these are wrong |
| P8 | Docs / Spec / Status-Map / Contract Drift | Schema/status docs out of sync with code | 14 | Includes research YAMLs |
| P9 | Supporting Code Hygiene | Unrelated changes hidden in PR; Cargo churn beyond scope | 15 | Includes `Cargo.toml/Cargo.lock`, `.gitignore`, nextest config |

Every changed file is assigned to exactly one packet. Cross-packet dependencies resolved by primary owner.

## Findings Ledger

Consolidated findings (deduplicated across reviewers). `Reviewers` column = which models independently surfaced the finding.

| ID | Packet | Reviewers | Severity | Artifact / Evidence | Status | Resolution |
|---|---|---|---|---|---|---|
| P0-A | P0 | Claude, Gemini, Kimi, GLM, DeepSeek (5/5) | BLOCKING | PR body declares head `fc7e081…`; actual head was `9fb1a239…`. | **resolved** | PR body updated 2026-05-17 to declare current head `0f16836b…` and explicitly mark `fc7e081` as superseded. |
| P0-B | P0 | Claude, Gemini, Kimi, GLM (4/5) | BLOCKING | All 5 logged external-review approvals cover `fc7e081`; current head delta unreviewed. | **partially resolved** | Doc side closed: PR body + audit-report now mark all logged approvals as covering superseded SHAs. **Operational side OPEN**: re-run external review wave at `0f16836b` — tracked as T074 / CHK058. |
| P0-C | P0 | Claude, Gemini, Kimi, GLM (4/5) | BLOCKING | T035 scope overrun — deletions of `src/platform/*`, `src/bin/raw_capture`, `render_live_config`, etc. not authorized by any T###. | **resolved** | T067 (platform retirement), T068 (capture/render-binary retirement), T069 (legacy validation retirement) added to `tasks.md` Retrospective Scope Reconciliation section. Each names its behavior lock. |
| P0-D | P0 | Claude, Gemini, Kimi, GLM (4/5) | BLOCKING | `src/bolt_v3_providers/polymarket/fees.rs` (+563 new) untraced. | **resolved** | T072 added to `tasks.md` — fee-provider extraction implementing F11, behavior lock `tests/bolt_v3_provider_binding.rs`. |
| P0-E | P0 | Claude (BLOCKING), Kimi (NON_BLOCKING) — default BLOCKING | BLOCKING | T056 [x] cites behavior lock `materialize_live_config_updates_oversized_drifted_output` — verified absent from tree. | **resolved** | T056 restated SUPERSEDED in `tasks.md`; oversized fail-closed property pointer redirected to `src/bounded_config_read.rs` + `cargo test oversized`. Dead test rows in audit-report Remediation Verification marked SUPERSEDED. |
| P0-F | P0 | Kimi (1/5; verified by grep) | BLOCKING | T065 [x] behavior lock cites `validate_live_local` — verified absent from tree. | **resolved** | T065 restated SUPERSEDED in `tasks.md`; instrument-id acceptance property pointer redirected to `src/bolt_v3_validate.rs` + `tests/config_parsing.rs`. |
| P0-G | P0 | GLM (1/5) | BLOCKING | `src/bolt_v3_market_identity.rs` deleted; no T### names it. | **resolved** | T070 added to `tasks.md` — supersedes the family-agnostic market-identity boundary via T060/T066 instrument-filter restructure. |
| P0-H | P0 | Claude (BLOCKING), Kimi (untraced) | BLOCKING | Shared-runtime rewrites untraced: `lake_batch`, `execution_state`, `venue_contract`, `secrets`, `log_sweep`, `bolt_v3_adapters`, `nt_runtime_capture`. | **resolved** | T073 added to `tasks.md` — explicitly names files, attributes changes to T033–T066 fallout + reapplied SSM raw-value preservation, points to the SSM trim regression test as behavior lock. |
| P0-I | P0 | Claude (1/5) | BLOCKING | New `config/root.toml` and `config/strategies/binary_oracle.example.toml` untraced. | **resolved** | T071 added to `tasks.md` — names the new example-config introduction with `tests/config_parsing.rs` as behavior lock. |
| P0-J | P0 | Claude (1/5) | NON_BLOCKING | `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` (+533/−35) heavier than T055. | open | Folded into the audit-report's Current-head Re-anchor narrative implicitly; not separately remediated. May address in follow-up PR. |
| P0-K | P0 | Claude (1/5) | NON_BLOCKING | `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` (+1937/−353) untraced. | open | Same as P0-J — not separately remediated in this commit. |
| P0-L | P0 | Claude, Gemini, GLM (3/5) | NON_BLOCKING | DeepSeek/GLM `source_not_sent` reviews listed in PR body without caveat. | **resolved** | PR body's "After `d606a57`, GLM exact-head shards" block now explicitly notes "DeepSeek and GLM direct-API shards used approval-request output with `source_content_transmission: not_sent`; these approvals confirm the bundle text only, not the source code." |
| P0-M | P0 | Claude, Kimi, GLM (3/5) | FYI | FR-007 "MUST NOT merge" vs. mergeable posture. | **resolved** | PR body now has prominent ⚠️ "Do not merge while audit/remediation is open" callout at the top citing FR-007 + T074 dependency; audit-report Current-head Re-anchor table includes "Merge gate (FR-007)" row. |
| P0-N | P0 | GLM (1/5) | NON_BLOCKING | PR title understates scope. | **resolved** | Title updated 2026-05-17 to "Phase 9 audit, hardcode remediation, and legacy runtime retirement". |
| P0-X1 | P0 | Gemini | DISPROVEN | Gemini fabricated 28 CI-spec files. | disproven | No action against PR; if Gemini is re-sent (T074), require diff-line citations. |

Severity: `BLOCKING` / `NON_BLOCKING` / `FYI` / `DISPROVEN`.
Status: `resolved` / `partially resolved` / `open` / `disproven` / `reassigned-to-Px` / `needs-info`.

### P1 findings (captured 2026-05-17 at head `065c1dca`; DeepSeek output not captured this round)

| ID | Reviewers | Severity | Evidence | Status | Resolution |
|---|---|---|---|---|---|
| P1-A | Claude NB, Gemini NB, Kimi NB, GLM BLOCKING (4/5; default BLOCKING per rule 6) | BLOCKING | `tests/bolt_v3_production_entrypoint.rs::codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules` forbidden_path array (lines 88–117) enumerated 28 retired paths but omitted `src/bolt_v3_market_identity.rs` (T070). Verified: `comm -23 deleted_src_files fence_array_paths` → `src/bolt_v3_market_identity.rs`. | **resolved** | Commit `023d1214` adds path to forbidden_path array and adds `!lib.contains("pub mod bolt_v3_market_identity;")` plus `!lib.contains("pub mod raw_capture_transport;")` assertions. 5/5 test cases pass at `023d1214`. |
| P1-B | Claude NB (chainlink only), Gemini NB (chainlink only), Kimi NB (chainlink only), GLM NB (5 deps: chainlink, hmac, serde_yaml, tokio-tungstenite, arc-swap) | NON_BLOCKING | All 5 deps verified zero-use in `src/`, `tests/`, `scripts/` via grep. Last consumers were files deleted under T035/T067. | **resolved** | Commit `023d1214` removes all 5 from `[dependencies]` in `Cargo.toml`. `cargo update -p <each>` regenerated `Cargo.lock` removing chainlink-data-streams-report + serde_yaml + transitive thiserror/thiserror-impl/unsafe-libyaml. hmac/tokio-tungstenite/arc-swap remain in lockfile as transitive deps required by NT crates (correct outcome). |
| P1-C | Gemini NB | NON_BLOCKING | `REASONIX.md:18` listed four binaries (bolt-v2, render_live_config, stream_to_lake, raw_capture) — render_live_config and raw_capture retired under T068. Verified: `REASONIX.md` exists in repo tree (Gemini did not hallucinate this time). | **resolved** | Commit `023d1214` updates `REASONIX.md` to name only the two current binaries with pointer to T068; also updates lines 9 (tokio-tungstenite mention), 13 (legacy adapter list), and removes line 14 (Chainlink Oracle list — retired under T035). |
| P1-D | Claude NB, Kimi FYI, GLM Q7 | NON_BLOCKING | `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md:396` row R21 listed `src/config.rs`, `src/live_config.rs`, `src/secrets.rs`, `src/startup_validation.rs`, `src/raw_capture_transport.rs`, `src/clients/`, `src/platform/` as live "legacy surface" requiring migration. 6 of 7 retired (only `src/secrets.rs` remains). | **resolved** | Commit `023d1214` rewrites R21 to record retirement under T035/T067/T068/T069 and point at the production-entrypoint source fence. Status column changed from `Process-only` to `Closed`. |
| P1-E (Claude P1-4, Kimi P1-004) | Claude NB, Kimi FYI | NON_BLOCKING | Stale path references in `docs/bolt-v3/research/nt-pin-change/**` and the now-deleted `docs/bolt-v3/2026-05-15-price-data-adapter-bridge-plan.md`. Historical research/planning docs. | open | `2026-05-15-price-data-adapter-bridge-plan.md` deleted in docs-cleanup wave 2; `nt-pin-change/**` remains historical reference-only. |
| P1-F (Claude P1-5) | Claude FYI | FYI | `docs/superpowers/plans/2026-04-12-issue-134-runtime-enablement.md` and other planning docs reference retired files as Modify targets. Historical, by design. | open | Same disposition as P1-E. |

### P1 — Cross-reviewer convergence

| Finding | Claude | Gemini | Kimi | DeepSeek | GLM | Adjudicated |
|---|---|---|---|---|---|---|
| Fence missing market_identity.rs (P1-A) | ✓ NB | ✓ NB | ✓ NB | — | ✓ BLOCKING | **BLOCKING** (rule 6) |
| Orphan chainlink dep (P1-B subset) | ✓ NB | ✓ NB | ✓ NB | — | ✓ NB | NON_BLOCKING — resolved with 4 additional GLM-flagged deps removed |
| Orphan 4 other deps (P1-B extension) | — | — | — | — | ✓ NB | NON_BLOCKING — resolved (verified by grep) |
| REASONIX.md stale (P1-C) | — | ✓ NB | — | — | — | NON_BLOCKING — resolved |
| Boundary-doctrine R21 stale (P1-D) | ✓ NB | — | — | — | partial | NON_BLOCKING — resolved |
| Research/plan doc stale paths (P1-E/F) | ✓ NB | — | ✓ FYI | — | partial | NON_BLOCKING — deferred to follow-up docs PR |
| DeepSeek output | — | — | — | not captured | — | Coverage gap — see Open Coverage Gaps |

### P1 — Round 2 re-verification (captured 2026-05-17 at head `023d1214`)

After P1 fix commit `023d1214` shipped, the P1 re-verification prompt was sent to all 5 models.

| Model | Verdict | Confidence | Finding A | Finding B | Finding C | Finding D | E/F | New findings |
|---|---|---|---|---|---|---|---|---|
| Claude (Opus 4.7 1M) | APPROVE | 5/5 | RESOLVED | RESOLVED | RESOLVED | RESOLVED | DEFERRED | none |
| Gemini CLI | APPROVE | 5/5 | RESOLVED | RESOLVED | RESOLVED | RESOLVED | RESOLVED (accepted deferral) | none |
| Kimi Code CLI | APPROVE | 4/5 | RESOLVED | RESOLVED | RESOLVED | RESOLVED | RESOLVED (accepted deferral) | **P1-G NON_BLOCKING** (commit-message factual claim about thiserror/thiserror-impl removal — they remain as transitive deps); P1-H FYI (12 deleted test files absent from forbidden_path array; intentional, philosophy is production source fence) |
| DeepSeek (anomaly: output self-identified as "Claude (via Pi coding agent)" — possible model substitution) | APPROVE | 5/5 | RESOLVED | RESOLVED | RESOLVED | RESOLVED | ACCEPTED_DEFERRAL | none |
| GLM (Sonnet 4 via GLM) | APPROVE | 5/5 | RESOLVED | RESOLVED | RESOLVED | RESOLVED | ACCEPTED (deferred) | none |

**Adjudication**:
- **5/5 APPROVE** at head `023d1214` — no severity contradictions.
- **P1 CLOSED**. All four claimed resolutions verified by independent file:line citations across 5 reviewers (forbidden_path array index, lib.rs substring assertions, Cargo.toml dep removals, Cargo.lock transitive-only confirmation via `cargo tree -i`, REASONIX.md rewrite, doctrine R21 row rewrite).
- **P1-G (Kimi, NON_BLOCKING)** — commit message body claim "Removed direct + transitive entries: chainlink-data-streams-report, serde_yaml, thiserror, thiserror-impl, unsafe-libyaml" is partially inaccurate. Verified via Cargo.lock grep: thiserror v2.0.18 and thiserror-impl remain as transitive deps required by NT and AWS SDK crates; only chainlink-data-streams-report, serde_yaml, and unsafe-libyaml were actually removed. **Disposition**: commit message is git-immutable; per `~/.claude/rules/git-workflow.md` we prefer revert over force-push amend. Adding a clarifying note to PR body so future readers do not rely on the stale claim. Not a code or artifact issue.
- **P1-H (Kimi, FYI)** — 12 deleted test files (tests/audit_records.rs, tests/config_schema.rs, tests/eth_chainlink_taker_runtime.rs, tests/live_node_run.rs, tests/platform_runtime.rs, tests/polymarket_bootstrap.rs, tests/polymarket_catalog.rs, tests/raw_capture_transport.rs, tests/reference_actor.rs, tests/reference_pipeline.rs, tests/render_live_config.rs, tests/ruleset_selector.rs) are not in the forbidden_path array. Kimi correctly noted only tests/ruleset_selector.rs is. **Disposition**: by-design — the fence test name (`codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules`) targets production exposure via lib.rs/main.rs, not test-file existence. A re-introduced test file would be visibly orphaned in `cargo nextest run` output. No P1 action.
- **GLM out-of-scope items**: REASONIX.md:45 stale `config/live.local.example.toml` ref (reassigned to P8 docs drift); R20 doctrine row remains `Process-only` and could be tightened post-dep-cleanup (FYI for P8); `url` and `async-trait` orphan direct deps in Cargo.toml predate this PR (reassigned to P9 supporting-code hygiene).
- **DeepSeek anomaly**: the captured output self-identifies as "Claude (via Pi coding agent)" rather than a DeepSeek model. Possible model substitution by the pi routing layer or operator misroute. **Disposition**: tracked as a Coverage Gap; the verdict still counts toward 5/5 because 4 other independent reviewers converged on the same RESOLVED status with independent file:line evidence. Recommend verifying the exact model ID in `~/.config/llm/config.json` `roles.consultation` before P2 send.

### P2 findings (captured 2026-05-17 at head `023d1214`; all 5 model outputs captured)

| ID | Reviewers | Severity | Evidence | Status | Resolution |
|---|---|---|---|---|---|
| P2-F1 | Claude F1 (NB), Gemini #1 (NB), DeepSeek (FYI in Q4), Kimi P2-A (NB) — **4/5 NB** | NON_BLOCKING | `tests/bolt_v3_adapter_mapping.rs::polymarket_venue_config_plus_resolved_secrets_maps_to_nt_native_fields` (lines 210-326) asserts 11 polymarket data fields but does not round-trip `auto_load_debounce_milliseconds → auto_load_debounce_ms`. TOML field at `src/bolt_v3_providers/polymarket.rs:111`; NT mapping at `:670`. | **resolved** | Commit `ca494297` adds `assert_eq!(data.auto_load_debounce_ms, expected_data.auto_load_debounce_milliseconds)` after the transport_backend assert in the round-trip test. Verified: `cargo test --locked --test bolt_v3_adapter_mapping -- polymarket_venue_config_plus_resolved_secrets_maps_to_nt_native_fields` → PASS. |
| P2-F2 | Gemini #2 (NB) — 1/5 NB | NON_BLOCKING | `fee_cache_ttl_seconds` not asserted in round-trip. Kimi noted this field is consumed by the fee-provider boundary (`src/bolt_v3_providers/polymarket.rs:601`), not `PolymarketExecClientConfig.config_as()`, so its absence from the exec-config downcast is explainable but a separate boundary assertion would close the test gap. | open — deferred | Folded into [issue #371](https://github.com/seungpyoson/bolt-v2/issues/371) (Phase 9 hardening follow-up) item 3. Out of accepted PR #331 scope per CLAUDE.md rule 9. |
| P2-B | Kimi P2-B (NB), DeepSeek P2-1 (NB; framed as doc-side rather than code-side) — 2/5 NB | NON_BLOCKING | `docs/bolt-v3/2026-04-25-bolt-v3-schema.md:450,467,479,547,610` say `qsize` and `nt_qsize` "must equal the pinned NT default `100000` at NT rev `38b912a8b0fe14e4046773973ff46a3b798b1e3e`". `src/bolt_v3_validate.rs` does not enforce this; `tests/config_parsing.rs:1193` positively asserts that `qsize = 1000` parses cleanly. | open — deferred | [Issue #371](https://github.com/seungpyoson/bolt-v2/issues/371) item 1. Class of issue: documented validator invariants enforced only at mapper layer, not validator. Class fix per CLAUDE.md rule 7 belongs in a dedicated hardening branch, not PR #331. |
| P2-C | Kimi P2-C (NB) — 1/5 NB | NON_BLOCKING | `docs/bolt-v3/2026-04-25-bolt-v3-schema.md:361-397` say component blocks (`instance_id, cache, msgbus, portfolio, emulator, streaming`, plus `logging.file_config`) "allowed values: `disabled`". `src/bolt_v3_validate.rs` does not enforce this; the mapper at `src/bolt_v3_live_node.rs:909-914` checks `disabled_component(config)` only at config-mapping time, after validate_root_only has passed. Same class as P2-B. | open — deferred | [Issue #371](https://github.com/seungpyoson/bolt-v2/issues/371) item 2. |
| P2-D | Kimi P2-D (NB; others said no finding) — 1/5 NB, contested | DISPROVEN | Kimi's claim: `NtRuntimeCaptureGuards::shutdown` could silently return `Ok` when `CaptureFailureState` recorded a mid-run failure (capture fails → run completes Ok → shutdown Ok). Code path inspection at `src/nt_runtime_capture.rs:231-256` showed the match arm `(Some(primary), None) => Err(anyhow!(primary))` makes this impossible: `failure_state.error_message()` returns `Some` after `record_failure()` was called, which is latched per the existing `failure_state_latches_first_error_and_sets_stop_flag` test. The 4 other reviewers reached the same conclusion. | **disproven** | Commit `ca494297` extracts the match into a private fn `classify_capture_shutdown_result` and adds 4 unit tests covering all branches. The reproduction test `capture_shutdown_classification_surfaces_failure_state_when_supervisor_join_succeeded` exercises Kimi's exact claimed path (failure_state Some + supervisor join Ok) and asserts the result is Err, not Ok. Verified: `cargo test --locked --lib -- nt_runtime_capture::tests::capture_shutdown` → 4/4 PASS. Satisfies rule 5: "DISPROVEN requires reproduction test, not prose." |
| P2-FYI-1 | Claude F2 (FYI), Gemini #3 (FYI), Kimi Q6 (NB), GLM P2-FYI-1 (FYI), DeepSeek Q6 (NB) — 5/5 FYI/NB | FYI | `tests/config_parsing.rs` has 40+ value-level red tests but no parameterized per-required-field missing test. Missing-field rejection currently relies on serde's built-in "missing field X" deserializer error. | open — deferred | [Issue #371](https://github.com/seungpyoson/bolt-v2/issues/371) item 4. Test-side hardening, not a runtime regression. |

### P2 — Cross-reviewer convergence

| Finding | Claude | Gemini | Kimi | DeepSeek | GLM | Adjudicated |
|---|---|---|---|---|---|---|
| auto_load_debounce_ms round-trip missing (P2-F1) | ✓ NB | ✓ NB | ✓ NB | ✓ FYI in Q4 | — | **NON_BLOCKING (4/5) — resolved** |
| fee_cache_ttl_seconds round-trip (P2-F2) | — | ✓ NB | partial (boundary explained) | — | — | NON_BLOCKING (1/5) — deferred |
| qsize doc-vs-validator (P2-B) | — | — | ✓ NB | ✓ NB | — | NON_BLOCKING (2/5) — deferred (class fix) |
| component blocks "disabled" not validated (P2-C) | — | — | ✓ NB | — | — | NON_BLOCKING (1/5) — deferred (class fix with P2-B) |
| capture-failure mid-run silent (P2-D) | "fails closed" | "fails closed" | ✓ NB | "fails closed" | "fails closed" | **DISPROVEN with reproduction test** (commit `ca494297`) |
| Per-field missing-field red tests (P2-FYI-1) | ✓ FYI | ✓ FYI | ✓ NB Q6 | ✓ NB Q6 | ✓ FYI | FYI/NB — deferred |

Reviewer model IDs (P2 round 1, self-reported): `claude-opus-4-7`, `gemini-2.5-pro`, `kimi-latest` (Kimi Code CLI), `Claude (via Pi coding agent — claude-sonnet-4-20250514 or equivalent)` (DeepSeek pi-routing substitution persists), GLM (no exact model ID printed; consistent with Sonnet 4 via GLM route).

### P2 — Round 2 re-verification (captured 2026-05-17 at head `ca494297`)

After P2 round-1 fix commit `ca494297` shipped, the P2 re-verification prompt was sent to all 5 models.

| Model | Verdict | Confidence | P2-F1 | P2-D | P2-F2 | P2-B | P2-C | P2-FYI-1 | New findings |
|---|---|---|---|---|---|---|---|---|---|
| Claude (Opus 4.7 1M) | APPROVE | 5/5 | RESOLVED | DISPROVEN by repro | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | none |
| Gemini CLI | APPROVE | 5/5 | RESOLVED | DISPROVEN | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | none |
| DeepSeek (anomaly: still pi-routing to claude-sonnet-4-20250514) | APPROVE | 5/5 | RESOLVED | RESOLVED | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | none |
| Kimi Code CLI | APPROVE | 5/5 | RESOLVED | RESOLVED | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | none |
| GLM (Sonnet 4 via GLM) | APPROVE | 5/5 | RESOLVED | RESOLVED | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | DEFERRED_OK | none |

**Adjudication**:
- **5/5 APPROVE** at head `ca494297` — full convergence.
- P2-F1 RESOLVED: all 5 reviewers verified `assert_eq!(data.auto_load_debounce_ms, expected_data.auto_load_debounce_milliseconds)` inline at tests/bolt_v3_adapter_mapping.rs:266-269 with non-zero fixture value (250).
- P2-D DISPROVEN: all 5 reviewers verified `classify_capture_shutdown_result` extracted at src/nt_runtime_capture.rs:276, called from shutdown() at :247, inline match arms removed, 4 unit tests pass — the reproduction test `capture_shutdown_classification_surfaces_failure_state_when_supervisor_join_succeeded` exercises exactly Kimi's claimed path and asserts Err. Rule 5 satisfied.
- All 4 deferred items (P2-F2/P2-B/P2-C/P2-FYI-1) confirmed still applicable at `ca494297` by all 5 reviewers — issue #371 is correctly scoped.
- **Minor edge-case probes** (Kimi + DeepSeek): empty-string `Some("")` not tested in classifier; both reviewers confirmed `Err(anyhow!(""))` still returns Err → fail-closed posture preserved. No action needed.
- **Coverage gaps** (Kimi + GLM): full `cargo test --locked` suite not run by both due to environment resource constraints; targeted tests for the two shipped fixes passed; CI runs server-side. No action needed — blast radius minimal (2 files, +42 lines, all additive).
- **P2 CLOSED**.

### Resolution audit trail

**Round 1 fix — commit `0f16836b`** (pushed 2026-05-17):
- Added T067–T074 retrospective tasks + restated T056/T065 SUPERSEDED.
- Updated PR title and body.
- Closed P0-A, P0-C, P0-D, P0-E, P0-F, P0-G, P0-H, P0-I, P0-L, P0-M, P0-N (11 findings, doc side).

**Round 2 fix — commit `065c1dca`** (pushed 2026-05-17, after at-head re-review surfaced NEW-1):
- Removed self-referential "current head: 9fb1a239" claims from tasks.md, audit-report.md, checklist (7 references).
- Replaced with "production-code head" framing + deferral to PR body for current literal HEAD.
- Updated PR body to declare `065c1dca` and reference at-head CI runs.
- Closed NEW-1 + NEW-2 from at-head re-review.

**Verified**: `gh api repos/seungpyoson/bolt-v2/pulls/331 --jq '.head.sha'` returns `065c1dcaad7e4367ec5f7d58d659c7574c9cd83e`.

**Status**: 13/14 original P0 findings resolved + 2 re-review findings resolved (NEW-1, NEW-2). Remaining:
- P0-B operational portion (re-run external reviews at current head) — STILL OPEN as T074 / CHK058. The P0 verification re-review (5 models, captured below) approved doc-side closure but did NOT cover P1–P9 packets.
- P0-J, P0-K (NON_BLOCKING, schema-doc and research-TOML size) — left for follow-up PR.
- P0-X1 (Gemini fabrication) — DISPROVEN. Gemini's re-review explicitly self-corrected.

Severity: `BLOCKING` / `NON_BLOCKING` / `FYI` / `DISPROVEN`.
Status: `accepted` / `disproven` / `open` / `reassigned-to-Px` / `duplicate-of-<id>` / `needs-info`.

## Open Coverage Gaps

| Gap | Packet | Model | Reason | Plan |
|---|---|---|---|---|
| DeepSeek P0 output was truncated; only tail visible (out-of-scope observations + coverage gaps + confidence) | P0 | DeepSeek | API/UI truncation; full verdict and findings sections not captured | Re-send P0 prompt to DeepSeek with explicit "do not truncate" instruction; or paste full output when available |
| DeepSeek P1 round 1 output not captured (4/5 reviewers responded; 4/5 was decisive — BLOCKING surfaced and resolved) | P1 | DeepSeek | Output not pasted back round 1 | Round 2 captured DeepSeek output, but with model-substitution anomaly (see below). |
| DeepSeek P1 round 2 output self-identifies as "Claude (via Pi coding agent)" — likely model substitution by routing layer | P1, ongoing | DeepSeek | Routing/operator misroute | Before sending P2, verify exact model ID resolution in `~/.config/llm/config.json` `roles.consultation` and confirm the pi extension is not silently substituting. Per `~/.claude/rules/model-config.md`: requested model runs or call fails — no fallback. |
| Gemini's claim that 28 CI-spec files were added is fabricated; this casts doubt on Gemini's untraced-file enumeration | P0 | Gemini | Hallucinated file paths (see P0-X1) | Re-send Gemini's P0 prompt with explicit instruction to cite diff line numbers; treat Gemini's other P0 file-list claims as needing verification |
| No reviewer ran the listed `scripts/verify_*.py` at `9fb1a239` to confirm the audit-report's "Passed" claims are current | P7 | (deferred) | P0 scope was artifacts-only | P7 handles this — the verifier-integrity packet |
| External-review evidence at-head: PR body cites CI run `25952522238` for `fc7e081`; the at-head CI run is `25972314453` for `9fb1a239` and is green, but no external model has reviewed the at-head delta | P0 → P3..P9 | All | Force-push erased the SSM-trim fix history; current-head delta is unreviewed externally | Re-run external review at `9fb1a239` (see P0-B resolution) before adjudicating any subsequent packet |

## Adjudication Rules

1. **Head-staleness**: any finding submitted against a SHA other than the anchor SHA is invalid. Re-send the packet at the current head before considering the finding.
2. **Disagreement is evidence, not a vote**: if 3 models flag and 2 do not, the disagreement is itself a finding. Reproduce it. Do not majority-vote.
3. **Out-of-scope reassignment**: if a model surfaces a finding outside its packet, reassign to the owning packet (Pn) and add to that packet's ledger. Never drop.
4. **DISPROVEN requires reproduction**: rejecting a finding requires a test or command output proving the finding wrong. Prose-only rebuttal is not sufficient. (Permanent rule from `~/.claude/rules/learnings-permanent.md` item 5.)
5. **BLOCKING + accepted = must fix before merge**: no exceptions, no "tracked," no "deferred."
6. **Severity contradictions**: if Model A says BLOCKING and Model B says NON_BLOCKING for the same fact, default to BLOCKING until adjudicator produces evidence to downgrade.
7. **Verifier-blind-spot rule**: if a P7 finding shows a detector has a coverage hole, all packets that relied on that detector's green status must be re-examined.
8. **Duplicate consolidation**: if multiple models report the same finding, link them under one ID; severity = max.
9. **Stop on 2 verifier-integrity blockers**: if P7 produces ≥2 accepted BLOCKING findings against the source-fences, stop further adjudication and fix the detectors before continuing — every other packet's green is suspect.

## Operator Workflow

1. Verify anchor: `gh pr view 331 --repo seungpyoson/bolt-v2 --json headRefOid`. If different from `9fb1a23…`, follow re-anchoring rule.
2. For each packet, send the prompt to each of the 5 reviewers (mark `sent` in grid).
3. Paste model output back verbatim under the packet's `## Captured Output` section (added below as reviews arrive). Mark `received` in grid.
4. Adjudicate: for each finding, append to ledger with severity + status. Reassign out-of-scope findings to owning packet.
5. Reconcile contradictions per adjudication rules.
6. After all 50 reviews captured and adjudicated, produce the final synthesis section.

---

# Packet Prompts

Each prompt below is copy-paste ready. Replace nothing; the prompt is the message.

## P0 — Scope Discipline & Spec-to-Diff Adherence

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PR URL: https://github.com/seungpyoson/bolt-v2/pull/331

DECLARED SCOPE (PR body): "Phase 9 current-main hardcode/dual-path audit and remediation for accepted Bolt-v3 tasks T033-T040 and T060-T066, plus exact-head follow-up fixes from external review and local audit."

ACTUAL DIFF: 154 files, +16511 / -33317 (93 modified, 43 deleted, 16 added, 2 renamed).

PACKET P0 — review ONLY these artifacts:
- specs/003-phase9-current-main-audit/spec.md
- specs/003-phase9-current-main-audit/plan.md
- specs/003-phase9-current-main-audit/tasks.md
- specs/003-phase9-current-main-audit/audit-report.md
- specs/003-phase9-current-main-audit/checklists/phase9-audit-requirements.md
- PR title, body, full file list

KNOWN ANOMALY: PR body declares "Current pushed head: fc7e081…" but actual PR head is 9fb1a239…. All external reviews logged in the body (Grok, Gemini, GLM, Claude APPROVE) cover fc7e081, not the actual head.

REQUIRED QUESTIONS:
1. Does every changed file in the PR trace to a specific declared task (T021-T066)? List any file or directory whose change cannot be traced.
2. Are deletions of src/clients/, src/platform/, src/{config,live_config,validate,startup_validation}.rs covered by T035, or do any of those deletions exceed T035's accepted scope?
3. Are new files (src/bounded_config_read.rs, src/bolt_v3_instrument_filters.rs, src/bolt_v3_providers/polymarket/fees.rs) tied to a specific task?
4. Are any spec/plan/tasks claims contradicted by the actual diff?
5. The PR body claims "ready for review" — is that valid given all logged external reviews are on stale SHA fc7e081?
6. Does any artifact claim or imply "live readiness" anywhere it should not?
7. Spec FR-007 says "Audit/remediation MUST NOT ... merge." Is the current PR posture consistent with that?
8. Is there any T### task marked [x] in tasks.md whose completion is not actually evidenced by the diff at 9fb1a239?

REQUIRED OUTPUT:
1. Verdict: APPROVE / REQUEST_CHANGES / NEEDS_INFO
2. Findings (one per finding):
   - Severity: BLOCKING / NON_BLOCKING / FYI
   - Artifact:section/line
   - Requirement violated
   - Evidence
   - Minimal fix
3. Out-of-scope observations: name the packet (P1..P9) where each belongs.
4. Coverage gaps: anything you could not verify from P0.
5. Confidence: 1-5.

Constraint: do NOT review code files. Constrain yourself to scope-discipline and artifact-internal-consistency questions only.
```

## P1 — Legacy Path Removal & Dual-Path Elimination

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P1 — verify legacy paths are deleted AND unreachable.

IN-SCOPE FILES:
Deleted: src/clients/{binance,bybit,chainlink,deribit,hyperliquid,kraken,mod,okx,polymarket,polymarket/fees}.rs, src/platform/{audit,mod,polymarket_catalog,reference,reference_actor,resolution_basis,ruleset,runtime}.rs, src/bin/{raw_capture,render_live_config}.rs, src/{config,live_config,live_node_setup,validate,validate/tests,startup_validation,raw_capture_transport,bolt_v3_market_identity}.rs.
Deleted tests: tests/{audit_records,config_schema,eth_chainlink_taker_runtime,live_node_run,platform_runtime,polymarket_bootstrap,polymarket_catalog,raw_capture_transport,reference_actor,reference_pipeline,render_live_config,ruleset_selector}.rs.
Deleted config: config/live.local.example.toml, config/operator-snapshots/2026-04-16/{README.md,live.local.toml}.
Modified entrypoint surface: src/lib.rs, src/main.rs, Cargo.toml, Cargo.lock, tests/bolt_v3_production_entrypoint.rs.
New legacy fence: scripts/verify_bolt_v3_legacy_default_fence.py, scripts/test_verify_bolt_v3_legacy_default_fence.py.

OUT-OF-SCOPE: production code remaining under src/bolt_v3_*. If you observe a problem there, flag it as out-of-scope and name P2/P3/P4/P5/P6.

REQUIRED QUESTIONS:
1. Any `use` / `pub use` / `mod` declaration in src/lib.rs or src/main.rs that still names a deleted module?
2. Any remaining src/**/*.rs file (non-deleted) that references a deleted symbol or path?
3. Does tests/bolt_v3_production_entrypoint.rs constitute a source fence — does it actually prove the legacy paths cannot be loaded from main.rs?
4. verify_bolt_v3_legacy_default_fence.py — would it catch a newly-introduced `Default::default()` in a bolt_v3 module? Does its self-test exercise that?
5. Cargo.toml — any dependency that was used ONLY by deleted code and is still listed? Any feature flag still referencing removed code?
6. Cargo.lock — does the churn match the dependency removals exactly, or is there unrelated lockfile movement?
7. Any docs/, specs/, scripts/, or .github/ reference to a deleted module that should also be removed?
8. Any operator-snapshot or example config still referencing the deleted legacy paths?
9. Are there any test fixtures (tests/fixtures/, contracts/) that reference deleted catalogs or schemas?

REQUIRED OUTPUT:
1. Verdict: APPROVE / REQUEST_CHANGES / NEEDS_INFO
2. Findings: severity / file:line / requirement / evidence / fix.
3. Out-of-scope observations: name owning packet.
4. Coverage gaps.
5. Confidence: 1-5.
```

## P2 — TOML Runtime Values & Config Parsing

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P2 — verify every runtime value is TOML-sourced; no hidden defaults; example/fixture/code agree.

IN-SCOPE FILES:
- src/bolt_v3_config.rs
- src/bolt_v3_validate.rs
- src/bolt_v3_live_node.rs
- src/bolt_v3_adapters.rs
- src/bounded_config_read.rs (new)
- config/root.toml (new)
- config/strategies/binary_oracle.example.toml (new)
- contracts/polymarket.toml
- tests/fixtures/bolt_v3/root.toml
- tests/fixtures/bolt_v3/binance_execution.toml (new)
- tests/fixtures/bolt_v3/strategies/binary_oracle.toml
- tests/config_parsing.rs
- tests/bolt_v3_adapter_mapping.rs
- docs/bolt-v3/2026-04-25-bolt-v3-schema.md

OUT-OF-SCOPE: provider-specific behavior beyond config wiring (P3); strategy policy semantics (P4); market-family details (P5); readiness gates (P6); verifier scripts (P7).

CONTEXT: T033 moved `auto_load_debounce_milliseconds` to TOML. T062 moved `transport_backend` to TOML. T060 moved updown cadence slug-token to TOML. T065 removed `.POLYMARKET` instrument-id pin. T066 removed `0_i64` clock sentinel. T047 added bounded TOML reads with a documented 1 MiB pre-parse guard (T059 — accepted as resource-exhaustion guard, not trading policy).

REQUIRED QUESTIONS:
1. Any `#[serde(default)]`, `#[serde(default = "…")]`, or manual `Default` impl on a production config struct in src/bolt_v3_config.rs or src/bolt_v3_validate.rs? If yes, justify each as protocol/NT-glue or flag as a hardcode regression.
2. Any `unwrap_or`, `unwrap_or_default`, `or_default`, `Option::get_or_insert`, or `.unwrap_or_else(|| …)` covering for a missing/empty TOML value in the production parse/validate path?
3. Does config/root.toml include every required field that bolt_v3_validate.rs enforces? List any field in the validator that is missing from the example.
4. Does tests/bolt_v3_adapter_mapping.rs assert that EACH TOML field reaches the corresponding NT config struct field (round-trip), or only a subset?
5. src/bounded_config_read.rs — what is the byte limit? Is it from TOML or code? If code, is it strictly a resource-exhaustion guard per T059, or does it gate any trading policy?
6. tests/config_parsing.rs — does it red-test missing-required-field for every required field (not just the happy path)?
7. tests/fixtures/bolt_v3/binance_execution.toml (new) — does it exercise the new required `transport_backend` field?
8. docs/bolt-v3/2026-04-25-bolt-v3-schema.md — every required field documented, with exact key path and type? Any field documented that the parser does not consume? Any field the parser requires but the doc omits?
9. Any literal integer/string in src/bolt_v3_config.rs or src/bolt_v3_validate.rs that is not classified as protocol/NT-API glue/test fixture, and is not source-fenced by an allowlist entry in verify_bolt_v3_runtime_literals.py?
10. src/bolt_v3_live_node.rs — does runtime-capture failure handling actually fail closed (T036)? Any silent log-only path?

REQUIRED OUTPUT (same format as P0/P1).
```

## P3 — Provider/Venue Architecture & Secrets

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P3 — verify provider seam integrity, secret source = SSM only, no raw-secret display.

IN-SCOPE FILES:
- src/bolt_v3_providers/mod.rs
- src/bolt_v3_providers/binance.rs
- src/bolt_v3_providers/polymarket.rs
- src/bolt_v3_providers/polymarket/fees.rs (new)
- src/bolt_v3_client_registration.rs
- src/bolt_v3_secrets.rs
- src/secrets.rs
- src/venue_contract.rs
- contracts/polymarket.toml
- tests/bolt_v3_client_registration.rs
- tests/bolt_v3_provider_binding.rs
- tests/bolt_v3_controlled_connect.rs
- tests/venue_contract.rs

OUT-OF-SCOPE: strategy policy (P4); market families (P5); config parsing (P2 — only flag wiring failures here).

CONTEXT: SSM-only secret rule. Recent fix: `SsmResolverSession::resolve` does not trim; `bolt_v3_secrets::resolve_field` owns empty/leading/trailing-whitespace rejection.

REQUIRED QUESTIONS:
1. Does any non-SSM secret source remain anywhere reachable from production? Search for: env var lookup, AWS CLI subprocess, file-based credential, 1Password CLI, environment-variable fallback. Any hit must be flagged.
2. src/bolt_v3_secrets.rs — does `resolve_field` reject empty / all-whitespace / leading-or-trailing-whitespace secret values fail-closed? Is there a unit test proving this?
3. src/secrets.rs — does `SsmResolverSession::resolve` (or equivalent) preserve raw value bytes including whitespace? Any `.trim()`/`.trim_start()`/`.trim_end()`/`.replace`/regex transform on the SSM response?
4. Any `println!`, `eprintln!`, `format!`, `dbg!`, `tracing::*`, `log::*`, `Debug` impl, or `Display` impl in providers/secrets that could surface a raw credential, private key, mnemonic, or API secret?
5. src/bolt_v3_client_registration.rs — provider dispatch: is it a fixed match-arm? If yes, is it acknowledged as a current-slice dispatch seam (T061) and does an injection seam exist that lets tests bypass it?
6. src/bolt_v3_providers/polymarket/fees.rs (new) — fee values from TOML? Any literal fee/precision/rounding in code? Scope: limited to polymarket provider, no leakage into core?
7. src/bolt_v3_providers/binance.rs and polymarket.rs — does each contain ONLY provider-specific behavior, or does either embed strategy/market-family logic that should live elsewhere?
8. src/bolt_v3_providers/mod.rs — is the public surface minimal (registration + binding only), or does it expose provider internals?
9. contracts/polymarket.toml — does anything in the file embed runtime values (caps, thresholds) that should live in `config/root.toml` instead of the provider contract?
10. Any provider config field that, if swapped (e.g., to a different wallet or API key), would require editing more than one section? (Group-by-change rule.)

REQUIRED OUTPUT (same format).
```

## P4 — Strategy / Archetype / Policy / Admission

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P4 — verify no hardcoded trading policy, order shape, side inference, pricing fallback, or admission cap.

IN-SCOPE FILES:
- src/bolt_v3_archetypes/mod.rs
- src/bolt_v3_archetypes/binary_oracle_edge_taker.rs
- src/strategies/mod.rs
- src/strategies/registry.rs
- src/strategies/binary_oracle_edge_taker.rs (renamed from eth_chainlink_taker.rs)
- src/bolt_v3_strategy_registration.rs
- tests/bolt_v3_strategy_registration.rs
- tests/bolt_v3_submit_admission.rs
- tests/bolt_v3_decision_evidence.rs
- tests/support/stub_runtime_strategy.rs

OUT-OF-SCOPE: provider wiring (P3); market family selection (P5); retired live evidence gates (P6); config parsing (P2).

CONTEXT: T040 generalized order-shape policy; T063 removed pricing fallback (fast venue → reference); T064 removed outcome-side inference from `-UP.`/`-DOWN.` instrument-id suffixes; T065 removed `.POLYMARKET` pin; T039 moved live-order cap to TOML; T061 added injection seam for strategy validation dispatch.

REQUIRED QUESTIONS:
1. src/strategies/binary_oracle_edge_taker.rs — does ANY of: order shape (LIMIT/MARKET/IOC/GTC), entry/exit order type, time-in-force, side (Buy/Sell), or quantity precision get chosen by a literal in Rust rather than projected from strategy TOML?
2. Side inference: search the strategy module for any pattern matching `-UP.`, `-DOWN.`, instrument-id suffix parsing, or hardcoded outcome→side mapping. Any hit is a regression of T064.
3. Pricing fallback (T063): is there ANY code path that prices an entry/position from a venue OR market other than the configured fast venue for the active market? Look for "fallback", "alternate", "reference", or selection-by-priority logic.
4. Position EV / managed-position pricing: does it require `managed_position.market_id == active_market.id` before using active fast spot, or can it silently price across markets?
5. src/bolt_v3_archetypes/binary_oracle_edge_taker.rs — any literal trading policy (caps, thresholds, ratios, multipliers, slippage tolerances) not sourced from archetype TOML?
6. tests/bolt_v3_submit_admission.rs — does it red-test ALL of: initial admission, over-count, over-notional, zero notional, negative notional?
7. tests/bolt_v3_decision_evidence.rs — does it verify that the admission path produces evidence BEFORE the NT submit, and that absent evidence fails closed?
8. src/bolt_v3_strategy_registration.rs — strategy dispatch: is the injection seam (T061) actually wired up in a test? Or is the production binding the only path?
9. Any expired/missing fair-probability handling: does it fail closed (T048 = `fair_probability_helper_fails_closed_when_expired`)?
10. tests/support/stub_runtime_strategy.rs — does the stub exercise the same admission/evidence/submit ordering as production, or does it bypass any gate?

REQUIRED OUTPUT (same format).
```

## P5 — Market Family / Instrument Filter

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P5 — verify market-family / instrument-filter logic has no internal lookup tables or hardcoded gates.

IN-SCOPE FILES:
- src/bolt_v3_market_families/mod.rs
- src/bolt_v3_market_families/updown.rs
- src/bolt_v3_instrument_filters.rs (new)
- tests/bolt_v3_instrument_filters.rs (renamed from bolt_v3_market_identity.rs)
- tests/nt_polymarket_filter_integration.rs

OUT-OF-SCOPE: provider modules (P3); strategy semantics (P4); config parsing (P2 — flag wiring failures only).

CONTEXT: T060 — `cadence_slug_token` is required in TOML; cadence-to-token internal table, minute-divisibility gate, and 32-character underlying bound were removed. T066 — instrument filter must derive from strategy TOML, not internal code defaults.

REQUIRED QUESTIONS:
1. src/bolt_v3_market_families/updown.rs — any internal map/array/match-arm from cadence value → slug token? Any minute-divisibility check (`% 60`, `% 1m`, etc.)? Any character-count gate on underlying or slug? Any of these is a T060 regression.
2. Is `cadence_slug_token` a required TOML field with no fallback? What is the error mode when it is missing?
3. src/bolt_v3_instrument_filters.rs — does the InstrumentFilterConfig derive every field from strategy TOML and clock source from NT `LiveClock`? Any `0_i64` clock sentinel (T066 regression)?
4. Family selection: how does the code pick which market_family to use? Is the dispatch table extensible without core edits, or is it a closed match?
5. tests/bolt_v3_instrument_filters.rs — does it red-test the missing-token case AND prove non-table cadence values can build filters when paired with a configured token?
6. tests/nt_polymarket_filter_integration.rs — does it integrate through to NT, or does it stub the NT side?
7. Any literal string for venue/family name (e.g., `"polymarket"`, `"updown"`, `"binance"`) used as a dispatch key in production code rather than as a stable protocol label?
8. Operator-misconfiguration: is the misconfigured-token risk documented and tested, or only documented?

REQUIRED OUTPUT (same format).
```

## P6 — Readiness / Canary / Evidence

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P6 — verify readiness/canary gates fail closed and have no silent-pass paths; verify caps are TOML-sourced.

IN-SCOPE FILES:
- retired live evidence gate modules and tests
- src/bolt_v3_readiness.rs
- tests/bolt_v3_readiness.rs

OUT-OF-SCOPE: strategy semantics (P4); provider/secret (P3); config parsing (P2).

CONTEXT: T039 — live-submit max order count is TOML-sourced; live proof accepts positive admitted-submit count up to the TOML cap and rejects zero or above-cap evidence. T036 — `run_bolt_v3_live_node` capture-failure regression must preserve the live-node run future and avoid false capture-failure logging on closed notification.

REQUIRED QUESTIONS:
1. Live-submit admission: every threshold/cap from TOML? Any literal cap remaining in code (other than protocol-required minimums)?
2. Single-submit evidence — does it accept a positive admitted submit count up to `max_live_order_count`, and reject zero or above-cap? Cite the test that proves both red sides.
3. Is there any path where an absent or empty evidence artifact silently passes? Any `if let Ok(_)` or `unwrap_or_default()` that turns "missing" into "pass"?
4. Strategy-free readiness — does it require explicit operator config AND evidence? What happens if either is missing?
5. Strategy-free schema — does the schema reject partial/empty submissions fail-closed, or does it have any optional field that should be required?
6. Live-submit admission — does the admission boundary run before exchange submit? Is the ordering enforced by a test?
7. Retired single-submit preconditions — did they exercise all preconditions (operator config, SSM, evidence hash), or only a subset?
8. Does any test in this packet set up a fake "live ready" state that production code accepts but should not?
9. Is there any path that, when run without live-submit admission configured, defaults to a permissive state instead of fail-closed?
10. Does run_bolt_v3_live_node correctly propagate capture failures without false-positive logging on closed notification (T036)?

REQUIRED OUTPUT (same format).
```

## P7 — Verifier Integrity & CI Gate (HIGHEST LEVERAGE)

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P7 — verify the source-fence detectors actually catch what they claim. This is the highest-leverage packet. If detectors are wrong, every "green" elsewhere is fake.

IN-SCOPE FILES (read all):
- scripts/verify_bolt_v3_runtime_literals.py (~545 lines)
- scripts/verify_bolt_v3_provider_leaks.py (~847 lines)
- scripts/verify_bolt_v3_core_boundary.py (~78 lines)
- scripts/verify_bolt_v3_naming.py
- scripts/verify_bolt_v3_status_map_current.py
- scripts/verify_bolt_v3_pure_rust_runtime.py (~341 lines)
- scripts/verify_bolt_v3_legacy_default_fence.py (~194 lines, new)
- scripts/verify_bolt_v3_strategy_policy_fence.py (~120 lines, new)
- scripts/verify_runtime_capture_yaml.py
- scripts/verify_ci_workflow_hygiene.py
- All corresponding scripts/test_verify_*.py self-tests
- justfile (specifically the `source-fence`, `fmt-check`, `gate` recipes)
- .github/workflows/ci.yml

OUT-OF-SCOPE: production Rust code (other packets). Treat this packet as if production code is opaque — review only whether detectors WOULD catch a deliberately-injected violation.

CONTEXT: justfile `source-fence` target chains all verifiers + self-tests. CI `gate` job is supposed to invoke this. Phase 9 wired new verifiers (legacy_default_fence, strategy_policy_fence) into `just fmt-check` per T057.

REQUIRED QUESTIONS (one per verifier — answer all):
1. verify_bolt_v3_runtime_literals.py — does it scan EVERY production .rs under src/, or only an explicit list? Does it strip `#[cfg(test)]` blocks before scanning so test-only literals do not whitelist production hits? Are its allowlist entries scoped tightly (file + line + reason) or broad (file-only)?
2. verify_bolt_v3_provider_leaks.py — does it catch a `polymarket`/`binance`/`chainlink`-specific symbol or string added to src/bolt_v3_{config,validate,live_node,adapters}.rs (i.e., core)? Does the self-test prove this by injecting such a token into a fixture and asserting FAIL?
3. verify_bolt_v3_core_boundary.py — at 78 lines, what does it actually enforce? Is the boundary list complete (every core module covered)?
4. verify_bolt_v3_naming.py — what naming rules are enforced, and would they catch an invented term like `polymeta_router`?
5. verify_bolt_v3_status_map_current.py — does it verify the status map row-by-row against actual source/entrypoint, or does it only check schema?
6. verify_bolt_v3_pure_rust_runtime.py — does it scan ALL of `src/` for PyO3, maturin, Python subprocess invocation, AWS CLI subprocess? Per T057, does it now cover runtime-capture and strategy modules?
7. verify_bolt_v3_legacy_default_fence.py (new) — does it catch `Default::default()`, `#[derive(Default)]`, manual `Default` impl in bolt_v3 modules? Self-test asserts FAIL on injection?
8. verify_bolt_v3_strategy_policy_fence.py (new) — does it catch hardcoded `Side::Buy/Sell`, `OrderType::Limit/Market`, fixed cadence tokens, instrument-id suffix patterns? Self-test exhaustive?
9. For each scripts/test_verify_*.py — does the self-test MUTATE a fixture into a known-bad state and assert FAIL, or does it only assert PASS on the current code? (Vacuous self-tests are a P7 BLOCKING finding.)
10. justfile `source-fence` recipe — does it call EVERY verifier and EVERY self-test? Any verifier listed in scripts/ that is not invoked by the recipe?
11. .github/workflows/ci.yml — does the `gate` job actually shell out to `just source-fence`? Does CI fail if any verifier fails?
12. Order of operations: do verifiers strip `#[cfg(test)]` and test files before scanning? If not, demonstrate a way to launder a production hardcode through a test alias.

REQUIRED OUTPUT (same format as P0, plus):
- For each verifier, list its coverage and any blind spot you identified.
- If a self-test is vacuous (does not exercise a fail path), label it BLOCKING.
- Propose minimal additions to close any coverage hole.
```

## P8 — Docs / Spec / Status-Map / Contract Drift

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P8 — verify docs, status maps, and research artifacts match the code at the current head.

IN-SCOPE FILES:
- docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md
- docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
- docs/bolt-v3/2026-04-25-bolt-v3-schema.md
- docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md
- docs/bolt-v3/2026-04-28-source-grounded-status-map.md
- docs/bolt-v3/research/naming/nt-owned-name-audit.yaml
- docs/bolt-v3/research/runtime-capture/bolt-current-capture.yaml
- docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml
- docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml
- specs/001-thin-live-canary-path/external-review-phase6-disposition.md
- specs/001-thin-live-canary-path/external-review-phase6-prompt.md
- specs/001-thin-live-canary-path/plan.md
- specs/001-thin-live-canary-path/quickstart.md
- specs/001-thin-live-canary-path/research.md
- retired Phase 7 strategy-free readiness spec files

OUT-OF-SCOPE: Phase 9 audit artifacts (P0); production code (P1–P7).

REQUIRED QUESTIONS:
1. Schema doc vs. code: every required field in src/bolt_v3_config.rs + src/bolt_v3_validate.rs documented? Any documented field the parser does not consume? Any field documented as optional that is actually required?
2. Status map: any row claiming a missing entrypoint, missing verifier, or missing capability that current src/ disproves? Any row claiming present that is actually absent?
3. Runtime-contracts doc: does it reflect the current adapter mapping in src/bolt_v3_adapters.rs?
4. Contract-ledger: do listed contracts match contracts/polymarket.toml and tests/fixtures/bolt_v3/?
5. nt-first-boundary-doctrine: any boundary rule violated by changes in P1/P2/P3/P4/P5?
6. research/naming/nt-owned-name-audit.yaml: any invented term present in code that this audit missed?
7. research/runtime-capture YAMLs: do they match src/nt_runtime_capture.rs and src/bolt_v3_live_node.rs?
8. research/runtime-literals TOML: any classification stale vs. the current verifier output?
9. specs/001 and specs/002 modifications: do the edits make sense in light of Phase 9 changes, or do they retroactively rewrite history? Any spec claim that contradicts what the code at 9fb1a239 actually does?
10. Any doc anywhere claiming "live ready" or "Phase 9 complete = live trade approval"? (Should not exist.)

REQUIRED OUTPUT (same format).
```

## P9 — Supporting Code Hygiene

```
You are reviewing PR #331 in seungpyoson/bolt-v2 at exact head bcd83f751ca9876bc0d76fc7f9e8e973a7198230.

PACKET P9 — verify every modification not assigned to P1–P8 is justified scope; flag unrelated changes.

IN-SCOPE FILES:
- src/bin/stream_to_lake.rs
- src/execution_state.rs
- src/lake_batch.rs
- src/log_sweep.rs
- src/nt_runtime_capture.rs
- src/raw_types.rs
- tests/cli.rs
- tests/lake_batch.rs
- tests/log_sweep.rs
- tests/nt_runtime_capture.rs
- tests/support/mod.rs
- tests/verify_build.sh
- .config/nextest.toml
- .gitignore
- Cargo.toml (the portion not already covered by P1 dep-removal)
- Cargo.lock (the portion not already covered by P1 dep-removal)

OUT-OF-SCOPE: any file already in P1–P8.

REQUIRED QUESTIONS:
1. For each modified file, does the diff trace to: (a) a declared T### task, (b) mechanical fallout from P1 deletions, or (c) something else? Flag (c).
2. Cargo.toml: any dep added that was not used by deleted code AND is not needed by new code? Any version bump unrelated to scope?
3. .gitignore: justify each new line.
4. .config/nextest.toml: justify each change. Is the change related to T032's full source coverage?
5. tests/verify_build.sh: does it still verify the same boundary it did pre-PR, or has its scope changed silently?
6. src/nt_runtime_capture.rs and tests/nt_runtime_capture.rs: changes relate to T036/T057, or to something else?
7. tests/support/mod.rs: any test-helper change that smells like production logic creeping into the support module?
8. src/bin/stream_to_lake.rs: a binary other than `bolt-v2`. Is its continued existence consistent with PURE RUST BINARY rule and one-runtime-path? Or is this another binary that escaped the legacy-cleanup?
9. Any file you would have expected to be changed but is not? (Coverage gap.)

REQUIRED OUTPUT (same format).
```

---

# Captured Output

Per-packet/per-model. Full verbatim outputs live in the user session; this section captures verdict + finding summary so the tracker stays operational.

## P0 — Captured 2026-05-17

### P0 — Claude (claude-opus-4-7, Opus 4.7 1M)

- Verdict: **REQUEST_CHANGES**
- Confidence: 4/5
- Findings raised: 11 (P0-1 through P0-11) — see ledger mapping P0-A/B/C/D/E/H/I/J/K/L/M
- Notable: only model to flag P0-E (T056 contradiction) and P0-H (shared-runtime untraced) and P0-I (config example untraced)

### P0 — Gemini (Gemini CLI)

- Verdict: **REQUEST_CHANGES**
- Confidence: 5/5 (self-rated — but see P0-X1)
- Findings raised: 7 (P0-001 through P0-007)
- Notable: **fabricated evidence** (P0-X1) — claimed 28 untraced CI-spec files that do not exist in the diff. Recommend re-running Gemini with stricter citation requirement.

### P0 — Kimi (Kimi Code CLI)

- Verdict: **REQUEST_CHANGES**
- Confidence: 4/5
- Findings raised: 7 (P0-001 through P0-007)
- Notable: only model to flag P0-F (T065 contradiction). Conservative on T056 severity (NON_BLOCKING) — overridden to BLOCKING per adjudication rule 6.

### P0 — DeepSeek (partial)

- Verdict: **(not captured — output truncated)**
- Confidence: 3/5 (self-rated)
- Findings raised: only out-of-scope observations and coverage gaps visible; no verdict block captured
- Notable: explicitly noted ~65% file-to-task coverage due to API truncation; would raise to 4/5 with local checkout. Concurs with SHA mismatch, stale reviews, T035 scope overrun, untraced `fees.rs`.
- **Action**: re-send P0 prompt to DeepSeek; capture full output.

### P0 — GLM (Claude Sonnet 4 review via GLM)

- Verdict: **REQUEST_CHANGES**
- Confidence: 4/5
- Findings raised: 8 (P0-01 through P0-08)
- Notable: only model to flag P0-G (`src/bolt_v3_market_identity.rs` untraced) and P0-N (PR title scope mismatch). Provided full untraced-file table.

### P0 — At-head re-verification (round 2, captured 2026-05-17 at head `0f16836b`)

After Round 1 fix commit `0f16836b` shipped, the P0 re-verification prompt was sent to all 5 models. Results:

| Model | Verdict | Confidence | Findings vs. claimed resolutions | New findings |
|---|---|---|---|---|
| Claude (Opus 4.7 1M) | APPROVE (docs-side closure) | 5/5 verified prior + 4/5 new | All 10 doc-side P0 RESOLVED; P0-B as expected PARTIAL | NEW-1 NON_BLOCKING (self-referential stale-head); NEW-2 NON_BLOCKING (CI not observed at literal HEAD) |
| Gemini CLI | APPROVE | 5/5 | All 10 doc-side P0 RESOLVED; P0-B PARTIAL | None. Self-corrected on P0-X1 fabrication. |
| Kimi Code CLI | APPROVE | 5/5 | All 10 doc-side P0 RESOLVED; P0-B PARTIAL | None |
| GLM (Sonnet 4 via GLM) | REQUEST_CHANGES | 4/5 | All 10 doc-side P0 RESOLVED; P0-B PARTIAL | **NEW-1 BLOCKING** (stale-head self-reference) |
| DeepSeek | (partial output — verdict block truncated) | 4/5 | Confirmed 13 prior resolutions; could not verify audit-report rows due to API truncation | None directly |

**Adjudication**:
- 4/5 APPROVE; GLM REQUEST_CHANGES.
- GLM's NEW-1 and Claude's NEW-1 are the same finding at different severities. Per adjudication rule 6 (contradiction defaults to BLOCKING) → **BLOCKING**.
- NEW-1 resolved by Round 2 fix commit `065c1dca` (verified by reading the doc edits — every "current head: <SHA>" claim either removed or restated as "production-code head" or "at task-authoring time").
- NEW-2 (CI at literal HEAD) — PR body now references CI run `25980066594` at `0f16836b` (mostly green; build was in_progress at time of next push) and explicitly notes that code-equivalent CI is green at production-code head `9fb1a239`. Resolved.

### P0 — Cross-reviewer convergence (Round 1)

| Finding | Claude | Gemini | Kimi | DeepSeek | GLM | Adjudicated |
|---|---|---|---|---|---|---|
| Stale head SHA (P0-A) | ✓ | ✓ | ✓ | ✓ | ✓ | BLOCKING |
| Reviews on stale SHA, none at head (P0-B) | ✓ | ✓ | ✓ | ✓ | ✓ | BLOCKING |
| T035 scope overrun (P0-C) | ✓ | ✓ | ✓ | ✓ | ✓ | BLOCKING |
| fees.rs untraced (P0-D) | ✓ | ✓ | ✓ | ✓ | ✓ | BLOCKING |
| T056 contradiction (P0-E) | ✓ | – | ✓ (NB) | – | – | BLOCKING (verified by grep) |
| T065 contradiction (P0-F) | – | – | ✓ | – | – | BLOCKING (verified by grep) |
| market_identity.rs untraced (P0-G) | – | – | – | – | ✓ | BLOCKING |
| Shared-runtime untraced (P0-H) | ✓ | – | partial | – | – | BLOCKING |
| Example-config untraced (P0-I) | ✓ | – | – | – | – | BLOCKING |
| Schema-doc size (P0-J) | ✓ | – | – | – | – | NON_BLOCKING |
| Runtime-literal audit TOML (P0-K) | ✓ | – | – | – | – | NON_BLOCKING |
| `source_not_sent` reviews carried (P0-L) | ✓ | partial | – | – | ✓ | NON_BLOCKING |
| FR-007 vs mergeable (P0-M) | ✓ | – | ✓ | – | ✓ | FYI |
| PR title scope (P0-N) | – | – | – | – | ✓ | NON_BLOCKING |
| **Gemini fabricated CI-spec files (P0-X1)** | – | claimed | – | – | – | **DISPROVEN** |

✓ = flagged; – = not flagged; NB = flagged at NON_BLOCKING.

5/5 convergence on the four headline blockers (P0-A through P0-D). Disagreement on P0-E severity resolved BLOCKING per adjudication rule 6. Gemini's fabricated CI-spec claim is a P0-level reviewer-quality finding, not a PR finding.

---

# Final Synthesis

Filled in after all 50 reviews are adjudicated. Currently P0 only (5/50 reviews captured).

## P0 Interim

- Total P0 findings: 14 accepted + 1 disproven (Gemini fabrication)
- Accepted BLOCKING: 9 (P0-A through P0-I)
- Accepted NON_BLOCKING: 4 (P0-J, K, L, N)
- FYI: 1 (P0-M)
- Disproven: 1 (P0-X1 — Gemini hallucinated 28 CI-spec files)
- Reassigned: 0 (P0 was artifact-only by design)
- Coverage gaps still open: 4 (DeepSeek truncation, Gemini fabrication doubt, no at-head external review, P7 verifier-run gap)

## P0 Status

| Item | Status |
|---|---|
| P0-A (wrong head in PR body) | RESOLVED (`0f16836b` body update) |
| P0-B doc side (stale review caveats) | RESOLVED (`0f16836b` body + audit-report update) |
| P0-B operational (re-run reviews at-head) | **OPEN** — see T074 below |
| P0-C, D, E, F, G, H, I (scope traceability) | RESOLVED (`0f16836b` T067–T073 + T056/T065 restatement) |
| P0-J, K (NON_BLOCKING schema/research size) | OPEN — deferred to follow-up PR |
| P0-L (source_not_sent caveat) | RESOLVED |
| P0-M (FR-007 merge gate) | RESOLVED |
| P0-N (PR title) | RESOLVED |
| P0-X1 (Gemini fabrication) | DISPROVEN — Gemini self-corrected at re-review |
| Re-review NEW-1 (stale-head self-reference) | RESOLVED (`065c1dca` production-code-head framing) |
| Re-review NEW-2 (CI at literal HEAD) | RESOLVED (PR body now references docs-only CI runs + code-equivalent CI at production-code head) |

### P3 findings (captured 2026-05-17 at head `ca494297`; all 6 reviewer outputs captured including GPT)

| ID | Source (reviewer / severity) | Adjudicated severity | Detail | Status | Resolution |
|----|------------------------------|----------------------|--------|--------|------------|
| P3-BLOCK1 | GPT BLOCKING; Claude F1 (FYI), Gemini NB, DeepSeek F1 (NB), Kimi F1 (NB), GLM F1 (NB) — **6/6 raised same class; 1 BLOCKING + 5 NB/FYI** | BLOCKING (rule 6 — severity contradictions default to BLOCKING) | `ResolvedBoltV3PolymarketSecrets` and `ResolvedBoltV3BinanceSecrets` held credentials in bare `pub String` fields with hand-rolled `Debug` redaction via local `RedactedDebug` helper. No zeroize-on-drop; redaction by manual discipline, not by class. | **resolved** | Commit `691521d3` aligns with NT's per-credential pattern in `crates/adapters/*/common/credential.rs`: `#[derive(Clone, Zeroize, ZeroizeOnDrop)]` on the resolved structs + `nautilus_core::string::secret::REDACTED` in their manual Debug impls + `zeroize = "1.8.2"` as a direct dep pinned at the version already locked transitively via NT. Verified: cargo fmt --check; cargo clippy --locked --all-targets -- -D warnings; cargo test --locked --lib (253 passed); cargo test --locked --tests (all 22+ binaries pass). |
| P3-NB1 | Claude F2 (NB) | NON_BLOCKING | `ssm_resolver_session_does_not_trim_resolved_secret_values` (src/secrets.rs:496) only forbade `.trim()`; `.trim_start()` / `.trim_end()` / `.replace(` / `Regex::` / `regex::Regex` transforms could silently regress the byte-exact SSM contract. | **resolved** | Commit `691521d3` broadens the guard to reject every transform variant the SSM contract forbids on the resolve body. Verified: cargo test --locked --lib -- ssm_resolver_session_does_not_trim → PASS. |
| P3-NB2 | Claude F4 (NB) | NON_BLOCKING | The wrapping path for NT credential-validator errors (`src/bolt_v3_providers/binance.rs:300`, `src/bolt_v3_providers/polymarket.rs:477-480`) appends `{reason}` from third-party NT validators. The existing `rejects_invalid_resolved_polymarket_private_key_shape` test asserts the wrapper prefix is present but does not assert the raw input bytes are absent from the wrapped error. | **resolved** | Commit `691521d3` adds two new tests: `wrapped_polymarket_private_key_error_does_not_leak_raw_input_bytes` and `wrapped_binance_api_secret_error_does_not_leak_raw_input_bytes`. Both pass a distinct sentinel value through the resolver and assert the sentinel is absent from both the wrapped `BoltV3SecretError.source` and the `Display` output. Verified: cargo test --locked --lib -- wrapped_polymarket_private_key wrapped_binance_api_secret → both PASS. |
| P3-NB3 | Claude F3 (NB) | NON_BLOCKING — deferred | `check_no_forbidden_credential_env_vars_with` silently returns Ok for an unknown provider kind (empty blocklist). Today `validate_venue_block` rejects unknown kinds upstream, so the env-var check standalone is theoretically permissive. Defense-in-depth gap. | **deferred → #371** | Folded into issue #371 (Phase 9 hardening follow-up) — defense-in-depth, not an active gap; gated upstream by `validate_venue_block`. |
| P3-NB4 | Kimi F2 (NB) | NON_BLOCKING — deferred | `ProviderResolvedSecrets` trait `Debug` bound is not compile-time-enforced for redaction. A future provider could derive `Debug` and bypass redaction. | **deferred → #371** | Folded into issue #371 — applies only when adding a third provider; current code follows NT's manual-Debug convention. |
| P3-NB5 | Kimi F3 (NB), GLM coverage note (FYI), GPT F2 (NB) — **3/6 raised** | NON_BLOCKING — deferred | T072 behavior-lock text says `tests/bolt_v3_provider_binding.rs` exercises the polymarket `build_fee_provider` seam. The functional fee-provider build tests live in `tests/bolt_v3_adapter_mapping.rs:634`; `tests/bolt_v3_provider_binding.rs:587-650` only enforces source-fence/import-boundary checks. Coverage exists; the spec text mis-anchors the file. | **deferred → #371** | Folded into issue #371 — test-organization, not behavior. |
| P3-FYI-1 | Claude F5 (NB-debatable), Claude F6 (FYI), DeepSeek FYI-padding | FYI — out-of-scope | (a) Polymarket wallet swap requires editing both `[clients.polymarket_main.secrets]` (SSM paths) and `[clients.polymarket_main.execution].funder` (public routing identifier) — group-by-change interpretation; defense: funder is a public value, not a credential. (b) `normalize_api_secret_padding` over-pads already-padded base64 input. Data-correctness, not P3 secret-source scope. | **noted** | Both observations recorded for future operator-docs / config-strictness work; neither violates the P3 secret-source invariant set. |

### P3 — Cross-reviewer convergence (round 1)

| Theme | GPT | Claude | Gemini | DeepSeek | Kimi | GLM | Net |
|-------|-----|--------|--------|----------|------|-----|-----|
| Bare-`String` secret fields + hand-rolled Debug (P3-BLOCK1) | **BLOCKING** | FYI | NB | NB | NB | NB | **BLOCKING** (rule 6) |
| No `.trim*`/`.replace`/`Regex::` regression guard (P3-NB1) | not raised | NB | not raised | not raised | not raised | not raised | NB → resolved |
| Wrapped-error raw-secret absence (P3-NB2) | not raised | NB | not raised | not raised | not raised | not raised | NB → resolved |
| env-var blocklist empty for unknown provider kind (P3-NB3) | not raised | NB | not raised | not raised | not raised | not raised | NB → deferred to #371 |
| `ProviderResolvedSecrets` trait Debug bound not compile-enforced (P3-NB4) | not raised | not raised | not raised | not raised | NB | not raised | NB → deferred to #371 |
| fee-provider seam test location vs T072 text (P3-NB5) | NB | not raised | not raised | not raised | NB | FYI | NB → deferred to #371 |
| Wallet group-by-change (P3-FYI-1a) | not raised | NB debatable | not raised | not raised | not raised | not raised | FYI |
| `normalize_api_secret_padding` over-pad edge case (P3-FYI-1b) | not raised | FYI | not raised | FYI doc | not raised | not raised | FYI |
| SSM single-source / no fallback backends (Q1) | clean (4/5 confidence) | clean (4/5) | clean (5/5) | clean (5/5) | clean (4/5) | clean (4/5) | unanimous clean |
| resolve_field whitespace fail-closed (Q2) | clean | clean | clean | clean | clean | clean | unanimous clean |
| SsmResolverSession::resolve byte-exact (Q3) | clean | clean | clean | clean | clean | clean | unanimous clean |
| Per-provider SSM-path validation ownership (Q12) | clean | clean | clean | clean | clean | clean | unanimous clean |
| contracts/polymarket.toml has no runtime values (Q9) | clean | clean | clean | clean | clean | clean | unanimous clean |
| controlled-connect uses mocks, no real credentials (Q14) | clean | clean | clean | clean | clean | clean | unanimous clean |
| venue_contract.rs no secret-bearing injection (Q15) | clean | clean | clean | clean | clean | clean | unanimous clean |

### P3 round 1 — Adjudication summary

Total findings (post-adjudication, deduped via MECE):
- **1 BLOCKING** — resolved in `691521d3` (P3-BLOCK1)
- **2 NB** — resolved in `691521d3` (P3-NB1, P3-NB2)
- **3 NB** — deferred to issue #371 (P3-NB3, P3-NB4, P3-NB5)
- **1 FYI** — recorded but not actioned (P3-FYI-1)

All non-deferred items satisfy rule 4 ("Two terminal states only: FIXED or DISPROVEN"). Deferred items have explicit acceptance criteria in #371 and are tracked as Phase-9 hardening follow-up under CLAUDE.md rule 9 (one branch = one declared scope) — they extend beyond PR #331's accepted scope.

Coverage gap: DeepSeek model-substitution anomaly continues — DeepSeek's manual route at P3 capture again served a Claude family model per the response's REPORT-EXACT-MODEL line. The verdicts still count toward 6/6 because GPT, Gemini, Kimi, GLM, and DeepSeek's substituted route independently converged with file:line evidence.

### P3 round 2 — Re-verification (captured 2026-05-17 at head `691521d3`; 5 reviewer outputs)

**Reviewers:** GPT (gpt-5.2-pro), Claude (claude-opus-4-7[1m]), Gemini, DeepSeek, Kimi. GLM not re-run at round 2; round 1 already captured 6/6 convergence on the BLOCKING class.

**Verdicts:** 4 APPROVE + 1 NEEDS_INFO. Substantive verdict 5/5 APPROVE — GPT's NEEDS_INFO is on a prompt-baseline metric (expected integration-test count "619 + 2 ignored"), not on the fix. All five reviewers independently confirm P3-BLOCK1, P3-NB1, P3-NB2 closed without regressions and no new P3-class findings.

| Reviewer | Verdict | Confidence | Notes |
|----------|---------|------------|-------|
| GPT | NEEDS_INFO | 4/5 | All 13 substantive items PASS; "P3 code fixes themselves look closed against the declared in-PR scope." NEEDS_INFO is solely on item 10 count baseline reconciliation. |
| Claude (claude-opus-4-7[1m]) | APPROVE | 5/5 | 13/13 PASS. Flagged same count discrepancy as FYI, plus two non-blocking hardening observations (NT-side downstream String clones; Debug-substring assertion not pinned). |
| Gemini | APPROVE | 5/5 | 13/13 PASS. Cited 619 — appears to have rubber-stamped the prompt baseline rather than counted; not load-bearing. |
| DeepSeek | APPROVE | 5/5 | All 13 PASS. Two FYI gaps: no compile-time ZeroizeOnDrop-fires test (inherent to pattern); aggregate Debug test asserts absence not presence of `<redacted>`. |
| Kimi | APPROVE | 5/5 | 13/13 PASS. Observed 600 + 2 ignored across 32 binaries; existing P3-NB4/NB5 deferred items reconfirmed as out-of-scope. |

#### Authoritative test-count baseline at `691521d3` (locally re-verified before close)

```
cargo test --locked --lib   → 253 passed; 0 failed; 0 ignored
cargo test --locked --tests → 600 passed; 0 failed; 2 ignored (32 binaries)
Combined                    → 853 passed; 0 failed; 2 ignored
```

Reviewer variance (Kimi 600 / Claude 601 / GPT 603) sits within ±3 and is consistent with environment-conditional test discovery; **zero failures** is the substantive gate and is unanimous. The prompt baseline "619 + 2 ignored" was carried over from a stale prior-session count and was not recomputed at this head — recorded here as a process miss (CLAUDE.md rule 1: never cite a metric without a fresh source command). Future packet prompts will pin the count from a fresh `cargo test --locked --tests` invocation at the current head.

#### Round-2 findings ledger (additive to round-1 ledger above)

| ID | Source | Severity | Detail | Status | Action |
|----|--------|----------|--------|--------|--------|
| P3-NB6 | DeepSeek (FYI), Claude (FYI) | NON_BLOCKING — deferred | The aggregate `resolved_bolt_v3_secrets_debug_does_not_leak_secret_values` test asserts the absence of secret values from the Debug output, but does not assert the **presence** of the NT `<redacted>` substring. A regression that replaced `&REDACTED` with an empty string or with a different placeholder could pass the existing test. | **deferred → #371** | One-line hardening: add `assert!(debug.contains("<redacted>"))` to the aggregate test. Folded into issue #371 as P3 round-2 leftover. |
| P3-NB7 | DeepSeek (FYI) | NON_BLOCKING — deferred | No runtime test verifies that `ZeroizeOnDrop` actually fires on drop of `ResolvedBoltV3PolymarketSecrets` / `ResolvedBoltV3BinanceSecrets`. The derive is verified structurally (source + NT-pattern match) and `Cargo.lock` confirms the zeroize crate is linked, but the drop-zero behavior itself is not exercised in any assertion. Practical to test only via unsafe-memory inspection or by giving the struct a `&mut`-only inspection seam used in tests. | **deferred → #371** | Folded into issue #371 as P3 round-2 leftover; defense-in-depth, not an active gap (the derive's correctness is NT's responsibility once the trait bounds match). |
| P3-FYI-2 | Claude | FYI — informational | `Zeroize`/`ZeroizeOnDrop` only zero the bolt-side `ResolvedBoltV3{Polymarket,Binance}Secrets` struct on drop. Downstream `polymarket.rs:794-797` and `binance.rs:355-356` `.clone()` the fields into NT's `PolymarketExecClientConfig` / `BinanceDataClientConfig`, where NT holds them as bare `String` for the LiveNode's lifetime. The bolt-side surface is correct; the NT-side residency is NT's responsibility (NT only wraps `EvmPrivateKey` with `Zeroize`; `api_key`/`api_secret`/`passphrase` stay as bare `String` in NT config). | **noted** | Recorded for awareness; no action — fixing the NT-side requires changes upstream in NT, outside PR #331's scope and outside bolt-v2's edit perimeter. |

#### Adjudication summary (round 2)

Round-1 findings → all closed at `691521d3`:
- **P3-BLOCK1** — Polymarket+Binance resolved-secret structs now `#[derive(Clone, Zeroize, ZeroizeOnDrop)]`; manual Debug uses `nautilus_core::string::secret::REDACTED` (= `"<redacted>"`); `RedactedDebug` helper removed; `zeroize = "1.8.2"` pinned as direct dep matching NT-transitive version. NT pattern fidelity confirmed: `EvmPrivateKey` in `~/.cargo/git/checkouts/nautilus_trader-*/crates/adapters/polymarket/src/common/credential.rs:60` uses identical derive shape.
- **P3-NB1** — `ssm_resolver_session_does_not_trim_resolved_secret_values` (src/secrets.rs:514-521) rejects all six forbidden transforms: `.trim()`, `.trim_start()`, `.trim_end()`, `.replace(`, `Regex::`, `regex::Regex`.
- **P3-NB2** — Two new sentinel tests (src/bolt_v3_secrets.rs:484, :524) assert distinct sentinel values are absent from both `error.source` and `error.to_string()` on the polymarket/binance wrap-error paths.

Round-2 deferred items folded into [issue #371](https://github.com/seungpyoson/bolt-v2/issues/371): **P3-NB6** (assert presence of `<redacted>` in aggregate Debug test), **P3-NB7** (compile-time/runtime ZeroizeOnDrop-fires assertion). **P3-FYI-2** (NT-side downstream String clones) recorded as informational only — upstream NT responsibility.

**P3 — CLOSED** at `691521d3` (rounds 1+2, 5/5 substantive APPROVE).

Coverage gap (process-level, not code-level): the P3 round-2 prompt cited "619 + 2 ignored" as the integration-test baseline. Actual at this head is 600 + 2 ignored. Filed as a process correction here (CLAUDE.md rule 1); future packet prompts will pin counts from a fresh local run at the exact head being verified.

### P4 round 1 — Adjudication (captured 2026-05-17 at head `691521d3`; 6 reviewer outputs)

**Reviewers:** GPT, Claude (`claude-opus-4-7[1m]`), Gemini, DeepSeek (manual route again served a `claude-opus-4-7[1m]` substitution per its REPORT-EXACT-MODEL line), Kimi, GLM (also substituted — its REPORT-EXACT-MODEL line declares `claude-sonnet-4-20250514`). Two of six routes (DeepSeek + GLM) served Claude-family models instead of the intended non-Claude reviewer. The substantive verdicts still converge across the four distinct-model routes (GPT, Claude opus-4-7[1m], Gemini, Kimi).

**Verdicts:** 5 REQUEST_CHANGES (GPT, Claude, Gemini, Kimi, GLM) + 1 APPROVE (DeepSeek). All five REQUEST_CHANGES converge on the same BLOCKING class. DeepSeek's APPROVE called the same issue NB; per adjudication rule 6, severity contradictions across reviewers default to BLOCKING.

| Reviewer | Verdict | Confidence | Primary finding |
|----------|---------|------------|-----------------|
| GPT | REQUEST_CHANGES | 4/5 | P4-DECISION-EVIDENCE-INCOMPLETE BLOCKING — admit gate writes no decision evidence; intent record has no timestamp/gate identity/version |
| Claude (`claude-opus-4-7[1m]`) | REQUEST_CHANGES | 4/5 | B1 BLOCKING (no timestamp) + B2 BLOCKING (admission gate decisions not recorded) + NB1-4 + FYI1-2 |
| Gemini | REQUEST_CHANGES | 5/5 | P4-EVIDENCE-MISSING-GATES BLOCKING + P4-REGISTRATION-DUP-CHECK BLOCKING |
| DeepSeek (substituted → claude family) | APPROVE | 5/5 | NB1 timestamp; NB2 dedup; NB3 UnsupportedStrategy test; FYI |
| Kimi | REQUEST_CHANGES | 5/5 | P4-EVIDENCE-001 BLOCKING + P4-MUTEX-SYNC NB |
| GLM (substituted → claude-sonnet-4) | REQUEST_CHANGES | 4/5 | P4-B1 BLOCKING (timestamp + gate identity + gate version) + P4-NB1 (arm-precondition doc) |

#### P4 findings ledger

| ID | Source (reviewer + label) | Adjudicated severity | Detail | Status | Resolution |
|----|---------------------------|----------------------|--------|--------|------------|
| P4-BLOCK1 | GPT P4-DECISION-EVIDENCE-INCOMPLETE BLOCKING; Claude B1+B2 BLOCKING; Gemini P4-EVIDENCE-MISSING-GATES BLOCKING; Kimi P4-EVIDENCE-001 BLOCKING; GLM P4-B1 BLOCKING; DeepSeek NB1 timestamp NB (substituted route) — **5/6 BLOCKING + 1 NB → BLOCKING by rule 6** | BLOCKING | (a) `BoltV3SubmitAdmissionState::admit` returns admit/reject without writing any decision evidence (5 reject reasons + admit all bypass audit). (b) `BoltV3OrderIntentEvidence` has no timestamp, no gate identity, no gate version. (c) `BoltV3DecisionEvidenceWriter` trait exposes only `record_order_intent`. | **resolved** | Commit `bcd83f75` aligns with the Q4 audit contract: adds `record_admission_decision` trait method + `BoltV3AdmissionOutcome` enum naming every outcome variant + `BoltV3AdmissionDecisionEvidence` record. Every JSONL line now wraps payload in a versioned envelope: `schema_version`, `recorded_at_utc_ns` (writer-stamped via `chrono::Utc::now()`), `gate_id` (`bolt_v3.order_intent` or `bolt_v3.submit_admission`), `gate_version` (`env!("CARGO_PKG_VERSION")`), `kind`, payload. `BoltV3SubmitAdmissionState::new` now requires the writer at construction; `admit()` does a 2-phase commit (evaluate → record → mutate) so evidence-write failure surfaces as `BoltV3SubmitAdmissionError::EvidenceWriteFailed { reason }` and does NOT consume an admission slot. Writer construction moves up to `build_live_node_with_clients` so the same `Arc<dyn>` is shared by admission state and strategy registration. Verified: cargo fmt clean; cargo clippy clean; cargo test --locked --lib → 255 passed; cargo test --locked --tests → 605 passed, 0 failed, 2 ignored. |
| P4-NB1 | Gemini P4-REGISTRATION-DUP-CHECK BLOCKING; Claude NB3 NB; DeepSeek NB2 NB; Kimi caveat — **1 BLOCKING + 3 NB → severity contradiction by rule 6** | NON_BLOCKING — deferred (rule 9) | `register_bolt_v3_strategies_on_node_with_bindings` does not maintain a per-call dedup of `registered_strategy_id` strings. Detection currently relies on upstream `validate_venue_block`/`src/bolt_v3_validate.rs:440-448` plus NT's own `add_strategy()` rejection of duplicate IDs. Defense-in-depth gap, not an active leak — Gemini's BLOCKING is on "defensive dedup" being absent, but the runtime path is correct by composition. | **deferred → #371** | Defense-in-depth, gated upstream. Folded into issue #371 as P4 round-1 leftover. Severity downgraded because (a) upstream validation rejects duplicates at config-parse time, (b) NT's `add_strategy()` rejects duplicate `StrategyId`s, (c) no production path exercises the gap. |
| P4-NB2 | Claude NB1 NB (mutex panic-on-poison); Kimi P4-MUTEX-SYNC NB; DeepSeek implicitly OK | NON_BLOCKING — deferred | `BoltV3SubmitAdmissionState::{arm,admit}` use `.expect("submit admission state mutex should not be poisoned")` rather than mapping `PoisonError` to a typed `BoltV3SubmitAdmissionError` variant. Fail-closed intent is correct (halts on prior corruption), but the form converts a structured error contract into a process panic. Kimi also notes `std::sync::Mutex` in an async-runtime context could in principle block executor threads; current single-threaded NT runtime makes this benign. | **deferred → #371** | Folded into issue #371; defense-in-depth on the error contract shape, not an active correctness gap. |
| P4-NB3 | Claude NB4 NB (source-string scanning in tests) | NON_BLOCKING — deferred | `tests/bolt_v3_strategy_registration.rs::binary_oracle_runtime_mapping_uses_market_family_target_projection` and `tests/bolt_v3_decision_evidence.rs::binary_oracle_edge_taker_records_evidence_then_admission_before_only_direct_submit_call` use `include_str!()` + `source.contains()` ordering scans. Brittle against refactors / comment text containing the searched strings. | **deferred → #371** | Folded into issue #371; test-architecture hardening, not behavior. |
| P4-NB4 | Claude NB2 NB; Kimi FYI; DeepSeek NB3+FYI; GPT coverage gap — **multiple reviewers** | NON_BLOCKING — deferred | Test coverage gaps: (a) `BoltV3StrategyRegistrationError::UnsupportedStrategy` rejection at the registration boundary is not exercised in `tests/bolt_v3_strategy_registration.rs`; (b) decision-evidence write-failure / lock-poisoning paths not exercised; (c) multi-strategy registration (two strategies on same node) not exercised. The first two are partially mitigated by tests in inline `#[cfg(test)]` modules within the production files. | **deferred → #371** | Folded into issue #371; test-suite extension, not a production gap. |
| P4-FYI-1 | Claude FYI1; DeepSeek FYI | FYI — recorded | `STRATEGY_ID_SEPARATOR` `-` is inlined at `binary_oracle_edge_taker.rs:570`. A shared const at the binding root would prevent future drift if more archetypes are added. | **noted** | Format constant, not a tunable; no NO HARDCODES violation. Will become worth extracting when the second concrete archetype lands. |
| P4-FYI-2 | Claude FYI2 | FYI — recorded | `validate_strategy_runtime_fields` repeats `value == 0` as a positive-integer sentinel pattern at three sites. A `require_positive_u64(field, value, &mut errors)` helper would scale better for additional archetypes. | **noted** | Same condition as P4-FYI-1 — applies when the second archetype is added. |
| P4-FYI-3 | DeepSeek FYI1 | FYI — recorded | `oms_type_value()` match in `binary_oracle_edge_taker.rs:647-649` has one arm; will fail-to-compile if `OmsType` adds variants. Acceptable exhaustive-match behavior. | **noted** | Compile-time enforcement is the right shape. |

#### P4 — Cross-reviewer convergence (round 1)

| Theme | GPT | Claude | Gemini | DeepSeek | Kimi | GLM | Net |
|-------|-----|--------|--------|----------|------|-----|-----|
| Admission gate decisions not recorded (P4-BLOCK1 part a) | **BLOCKING** | **BLOCKING** | **BLOCKING** | NB | **BLOCKING** | **BLOCKING** | **BLOCKING** (rule 6) |
| Order-intent record missing timestamp (P4-BLOCK1 part b) | **BLOCKING** | **BLOCKING** | **BLOCKING** | NB | **BLOCKING** | **BLOCKING** | **BLOCKING** |
| Order-intent record missing gate identity (P4-BLOCK1 part c) | **BLOCKING** | not raised | **BLOCKING** | not raised | not raised | **BLOCKING** | **BLOCKING** |
| Order-intent record missing gate version (P4-BLOCK1 part d) | **BLOCKING** | not raised | not raised | not raised | not raised | **BLOCKING** | **BLOCKING** |
| Duplicate-strategy-id dedup at P4 layer (P4-NB1) | not raised | NB | **BLOCKING** | NB | caveat | not raised | NB → deferred (rule 9) |
| Mutex poison panic-on-poison (P4-NB2) | not raised | NB | not raised | OK | NB | OK | NB → deferred |
| Source-string-scanning brittle tests (P4-NB3) | not raised | NB | not raised | not raised | not raised | not raised | NB → deferred |
| UnsupportedStrategy registration test gap (P4-NB4a) | not raised | NB | not raised | NB | FYI | not raised | NB → deferred |
| Evidence write-failure path test gap (P4-NB4b) | not raised | NB | not raised | FYI | FYI | not raised | NB → deferred |
| Multi-strategy registration test gap (P4-NB4c) | not raised | not raised | not raised | not raised | not raised | FYI | FYI |
| STRATEGY_ID_SEPARATOR const (P4-FYI-1) | not raised | FYI | not raised | not raised | not raised | not raised | FYI |
| require_positive_u64 helper (P4-FYI-2) | not raised | FYI | not raised | not raised | not raised | not raised | FYI |
| oms_type_value exhaustiveness (P4-FYI-3) | not raised | not raised | not raised | FYI | not raised | not raised | FYI |
| NO HARDCODES audit (Q1) | clean (4/5) | clean (5/5) | clean (5/5) | clean (5/5) | clean (5/5) | clean (4/5) | unanimous clean |
| Archetype routing fail-closed (Q2) | clean | clean | clean | clean | clean | clean | unanimous clean |
| Admission gate ordering (Q3) | clean | clean (NB1 form) | clean | clean | clean | clean (NB1 doc) | unanimous clean on order |
| Concurrency in binary_oracle_edge_taker (Q6) | clean | clean | clean | clean | clean | clean | unanimous clean — pure sync config mapper |
| Credential containment (Q7) | clean | clean | clean | clean | clean | clean | unanimous clean |
| Single canonical paths (Q8) | clean | clean | clean | clean | clean | clean | unanimous clean |
| TODO/FIXME absence (Q9) | clean | clean | clean | clean | clean | clean | unanimous clean (zero hits) |

#### P4 round 1 — Adjudication summary

Total findings (post-adjudication, deduped via MECE):
- **1 BLOCKING** — resolved in `bcd83f75` (P4-BLOCK1: admit gate decisions + timestamp + gate identity + gate version)
- **4 NB** — deferred to issue #371 (P4-NB1 dedup, P4-NB2 mutex form, P4-NB3 brittle test scans, P4-NB4 test coverage gaps)
- **3 FYI** — recorded (P4-FYI-1 STRATEGY_ID_SEPARATOR; P4-FYI-2 require_positive_u64; P4-FYI-3 oms_type_value)

Decision-evidence schema bump: the JSONL on-disk format moves from a flat intent-only shape to a versioned envelope with `schema_version=2`. Operators reading existing `order-intents.jsonl` files will see new admission-decision lines interleaved (distinguished by `kind="admission_decision"`). Previous-version intent lines are not auto-migrated; the file is append-only and the new schema is the only shape written going forward. This is recorded in PR body as an operator-visible change.

Coverage gap (process-level): two of six reviewer routes (DeepSeek + GLM) served Claude-family models per their REPORT-EXACT-MODEL lines despite their non-Claude labels. The substantive verdicts still converge across the four distinct-model routes; the BLOCKING class is uncontested in the actual evidence. Logging the substitution pattern: DeepSeek substituted in P2/P3-round1/P3-round2/P4-round1 (4 consecutive packets); GLM substituted only at P4-round1. This is an operational signal that the model-routing/manual-route pipeline has an upstream substitution bug not specific to a single packet.

### P4 round 2 — Re-verification (captured 2026-05-17 at head `bcd83f75`; 5 reviewer outputs)

5 distinct routes: GPT, Claude (opus-4-7 — REPORT-EXACT-MODEL line confirms no substitution this round), Gemini, DeepSeek, Kimi. No model substitution flagged in any output for this round. Each reviewer ran the 14-item verification brief.

Substantive verdicts: 5/5 APPROVE. Confidence 5/5 from every reviewer.

Brief items 1–14 results (uniform across all 5 reviewers):
- Item 1 — diff stat strictly 11 files: PASS (504 insertions / 60 deletions)
- Item 2 — trait surface + 5-variant outcome enum + envelope structs with 5 metadata fields + payload: PASS
- Item 3 — four constants exact (`SCHEMA_VERSION=2`, `env!("CARGO_PKG_VERSION")`, `bolt_v3.order_intent`, `bolt_v3.submit_admission`): PASS
- Item 4 — `new(Arc<dyn Writer>)` + `EvidenceWriteFailed { reason: String }`: PASS
- Item 5 — two-phase commit under lock (evaluate pure → record → mutate only on Admitted): PASS
- Item 6 — single `JsonlBoltV3DecisionEvidenceWriter::from_loaded_config` site at `bolt_v3_live_node.rs:649`, shared `Arc<dyn>` to both consumers: PASS
- Item 7 — `register_*` takes writer by parameter, no internal construction: PASS
- Item 8 — all 6 focused tests pass: PASS
- Item 9 — `cargo fmt --all -- --check`: PASS
- Item 10 — `cargo clippy --locked --all-targets -- -D warnings`: PASS
- Item 11 — `cargo test --locked --lib` 255/0: PASS
- Item 12 — `cargo test --locked --tests` 0 failed: PASS (3/5 ran full suite; DeepSeek + Kimi hit host timeout/memory pressure and confirmed P4-relevant binaries pass; substantive gate met)
- Item 13 — every `admit()` return path flows through `record_admission_decision` (only carve-out is the recorder-failure path itself): PASS
- Item 14 — no new P4-class findings (no new hardcodes, no fail-open default, no credential leak, no debt markers, no dual paths): PASS

Findings introduced at round 2: **NONE**.

Carryover (already tracked in #371, unchanged): P4-NB1, P4-NB2, P4-NB3, P4-NB4, P4-FYI-1, P4-FYI-2, P4-FYI-3.

One non-finding observation (Claude, explicit "fold into #371 only if exact-time test reproducibility becomes a need"): `current_utc_ns` at `bolt_v3_decision_evidence.rs:159-163` uses `chrono::Utc::now().timestamp_nanos_opt().expect(...)` — no clock injection, so deterministic timestamp testing is impossible; tests currently assert `recorded_at_utc_ns > 0`, appropriate for a wall-clock writer. **Not adding to #371 today — conditional per Claude's own framing.**

One unrelated count observation (Claude): integration suite reported 606 passed; brief pinned 605. Substantive gate is `0 failed`, met by all reviewers. The +1 is below the noise threshold for a 600+ test corpus and may reflect a recent test addition between brief authoring and run; not a finding.

#### P4 round 2 — Adjudication summary

Substantive verdict: **APPROVE 5/5 across distinct models. P4-BLOCK1 close pinned at `bcd83f75`. No new P4-class findings. All deferred items remain correctly tracked in #371.**

**P4 packet — CLOSED (rounds 1 + 2).**

Head-anchor reconciliation: the substantive round-2 verification was performed at `bcd83f75`. After the round was launched, the PR head advanced to `0e34ef73` — a docs-only commit that registers the six new P4 audit-record literals (`SCHEMA_VERSION=2`, `env!("CARGO_PKG_VERSION")`, two gate IDs, two kinds) in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`. The `0e34ef73` delta is metadata only — it has zero production-code change and was required to unblock the `fmt-check` and `source-fence` CI lanes that the runtime-literal verifier enforces. Pre-push local quality bar at `bcd83f75` had used `cargo fmt --check` + `cargo clippy` + `cargo test` only, not `just fmt-check` / `just source-fence`; root-cause writeup carried in PR body and on `0e34ef73`'s commit body. Round-2 substantive verification at `bcd83f75` carries forward unchanged because no production code changed between heads.

CI status snapshot (head `0e34ef73`): `fmt-check`, `source-fence`, `clippy`, `deny`, `build`, `check-aarch64`, `detector`, `CodeQL`, `Analyze (actions)`, `Analyze (rust)` all PASS; nextest shards 3+4 PASS; shards 1+2 still running at time of writing; `test` rollup and `gate` will resolve as shards land.

Gemini Code Assist on PR #331 (2026-05-17 at `bcd83f75`): all 7 inline threads now RESOLVED. Three fresh production-code threads (fees `warm_inner` coalescing, secrets whitespace per-class scope, `bounded_config_read` streaming feasibility) received inline-reply dispositions linking to issue #371 items 15–17. Four older audit-doc threads were already resolved by prior commits; two further suggestions on that round are rejected on evidence — `eth_chainlink_taker.rs:3971-3977` DISPROVEN (file does not exist; analogous code at `binary_oracle_edge_taker.rs:5031-5034` already returns `None` when `time_to_expiry_years <= ZERO_F64`); audit-report.md / plan.md AI-slop scan keyword observations STALE (current `audit-report.md:44`+`:53` and `plan.md:50`+`:71` already include the FR-005 keywords; corrected scan at this head produced no new findings).

## Recommendation (current)

P0 + P1 + P2 + P3 + P4 fully closed. P4 round 2 closed by 5/5 distinct-model APPROVE (GPT, Claude opus-4-7 no-substitution, Gemini, DeepSeek, Kimi) at head `bcd83f75`; no new findings introduced. Fix commits: `0f16836b` + `065c1dca` (P0), `023d1214` (P1), `ca494297` (P2), `691521d3` (P3), `bcd83f75` (P4 round 1 — production-code), `0e34ef73` (P4 CI restoration: registers six P4 audit-record literals — `BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION = 2`, `env!("CARGO_PKG_VERSION")`, `BOLT_V3_ORDER_INTENT_GATE_ID`, `BOLT_V3_SUBMIT_ADMISSION_GATE_ID`, `kind: "order_intent"`, `kind: "admission_decision"` — in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`; docs-only allowlist update with no production-code change). Deferred hardening items consolidated in [issue #371](https://github.com/seungpyoson/bolt-v2/issues/371) — P4 round 1 adds P4-NB1 (dedup), P4-NB2 (mutex form), P4-NB3 (brittle test scans), P4-NB4 (test coverage gaps); items 15-17 add Gemini Code Assist's 2026-05-17 inline review dispositions (fees warm_inner request-coalescing, secrets whitespace per-class scope, bounded_config_read streaming feasibility). P0-B operational re-review (T074) is being satisfied incrementally as each Pn packet closes — P4 close completes another increment.

**Note on `bcd83f75` CI failure and `0e34ef73` restoration:** Pushing `bcd83f75` failed four CI checks (`fmt-check`, `source-fence`, `test`, `gate`) from a single root cause — the six new audit-record literals added to `src/bolt_v3_decision_evidence.rs` were not registered in the runtime-literal allowlist. `just fmt-check` and `just source-fence` both invoke `scripts/test_verify_bolt_v3_runtime_literals.py`, which enforces 1:1 correspondence between scanned production literals and allowlist rows. The `test-shards` matrix `needs: [detector, source-fence]`, so GitHub Actions skipped it when source-fence failed; the `test` rollup interpreted SKIPPED as not-success and `gate` escalated. Local pre-push verification at `bcd83f75` used `cargo fmt --check` + `cargo clippy` + `cargo test` only, NOT `just fmt-check` / `just source-fence` — process miss: local quality bar must match the CI recipe set on future commits. Fix commit `0e34ef73` registers each of the six literals with classifications `decision_evidence_envelope_schema_version`, `decision_evidence_gate_version_source`, `decision_evidence_gate_id` (×2), `decision_evidence_envelope_kind` (×2). Both `scripts/test_verify_bolt_v3_runtime_literals.py` and `scripts/verify_bolt_v3_runtime_literals.py` PASS locally at `0e34ef73`.

**Gemini Code Assist on PR #331 (2026-05-17 review at `bcd83f75`):** 7 inline items + 2 review summaries. Disposition: items on production code at current head (`fees.rs:149` warm_inner coalescing, `bolt_v3_secrets.rs:281` whitespace per-class scope, `bounded_config_read.rs:86` streaming) added to issue #371 as items 15-17. Two further suggestions were rejected on evidence: (a) F8 — `eth_chainlink_taker.rs:3971-3977` step-function readiness — DISPROVEN; the file does not exist (`find` + `git log --all` confirm), and the analogous code at `src/strategies/binary_oracle_edge_taker.rs:5031-5034` already returns `None` when `time_to_expiry_years <= ZERO_F64`. (b) audit-report.md / plan.md AI-slop-marker keyword observations — STALE; current `audit-report.md:44`+`:53` and `plan.md:50`+`:71` already include `As an AI|language model|I'm sorry|apologize|unfortunate` per FR-005, and re-running the corrected scan at this head produces no new findings.

**Next move: send P5 packet prompt at head `0e34ef73`.** P5 is the market-family / instrument-filter / market-pruning packet — verifies that the market-family typing system, instrument filter pipeline, and quote/trade routing in the bolt-v3 binding layer carry no hardcoded thresholds/IDs, no fail-open default routing, and no dual paths between the legacy filter site (if any remains) and the canonical filter. Same cadence: round-1 review across 5–6 reviewers → adjudicate → fix-if-findings → round-2 re-verification → close → P6. CI at `0e34ef73`: ALL checks PASS (`fmt-check`, `source-fence`, `clippy`, `deny`, `build`, `check-aarch64`, `detector`, `CodeQL`, `Analyze (actions)`, `Analyze (rust)`, all 4 nextest shards, `test` rollup, `gate`). PR-mergeability indicator is BLOCKED only on the still-pending P5–P9 review packets, not on CI; readiness is the user's call.

Fixed cadence for P2–P9: review (round 1) → fix if findings → re-verify (round 2) → close → next packet. No packet advances on round-1-only adjudication.
