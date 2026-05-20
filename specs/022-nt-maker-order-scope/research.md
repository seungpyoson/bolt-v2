# Research: NT-Matched Maker Order Scope

## Decision: Use pinned NautilusTrader Polymarket adapter as maker-order source of truth

**Evidence**:

- `Cargo.toml:22-35` pins NautilusTrader crates to `7c2aafb30fb143069c915a3f2057bb12174405f6`.
- `Cargo.lock:4746-4748` confirms `nautilus-polymarket` is loaded from the same rev.
- Local source path: `/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb`.

**Rationale**: Bolt-v3 constitution says NT owns adapter behavior and venue wire translation.

**Alternatives considered**: Local maker adapter logic. Rejected because it duplicates NT behavior.

## Decision: Maker order means post-only limit order, not a separate order type

**Evidence**:

- NT maps `TimeInForce::Gtc` to Polymarket `GTC`, `Gtd` to `GTD`, `Fok` to `FOK`, and `Ioc` to `FAK` in `crates/adapters/polymarket/src/common/enums.rs:294-304`.
- NT market-order mapping only allows `Fok` and `Ioc` in `crates/adapters/polymarket/src/common/enums.rs:309-315`.
- NT validates limit orders and rejects `post_only` unless TIF is `Gtc` or `Gtd` in `crates/adapters/polymarket/src/execution/order_builder.rs:164-198`.
- NT carries `request.post_only` into `SignedLimitOrderSubmission.post_only` in `crates/adapters/polymarket/src/execution/submitter.rs:327-359`.
- NT posts the signed order with `submission.post_only` in `crates/adapters/polymarket/src/execution/submitter.rs:362-380`.
- NT `PostOrderBody` serializes with `#[serde(rename_all = "camelCase")]` and includes `post_only` only when true in `crates/adapters/polymarket/src/http/clob.rs:62-70`.
- NT `ClobHttpClient::post_order` builds `PostOrderBody { post_only }` and sends it to `/order` in `crates/adapters/polymarket/src/http/clob.rs:362-376`.
- NT query serialization test proves true serializes as `postOnly` and false is omitted in `crates/adapters/polymarket/src/http/query.rs:529-547`.

**Rationale**: Polymarket maker behavior is expressed by `post_only=true` on an NT limit order with supported TIF.

**Alternatives considered**: Separate local `maker` order type. Rejected because NT exposes canonical order type plus post-only flag.

## Decision: GTD is supported by NT but bolt-v3 expiry policy is not yet approved

**Evidence**:

- NT accepts `TimeInForce::Gtd` in the generic Polymarket limit TIF mapping at `common/enums.rs:299-300`.
- NT test helper creates GTD limit orders with `expire_time` in `execution/order_builder.rs:383-404`.
- NT submitter maps request `expire_time` to order expiration in `execution/submitter.rs:331-357`.
- NT converts `UnixNanos` expiry to seconds string or `"0"` in `execution/submitter.rs:414-418`.

**Rationale**: If bolt exposes GTD, it must provide NT `expire_time`; otherwise the order cannot represent a valid GTD order. NT evidence proves the required field and pass-through behavior, but does not say bolt-v3 should derive expiry from `post_only_requote_interval_ms`. Reusing that cadence would be local policy, not NT-derived behavior, so GTD remains blocked until an explicit TOML-owned expiry field or equivalent config contract is approved.

**Alternatives considered**: Reuse `post_only_requote_interval_ms` as GTD expiry interval. Rejected because no cited NT evidence establishes that coupling.

## Bolt config-to-submit evidence

- Config load parses TOML and validates root/strategy configs in `src/bolt_v3_config.rs:359-413`.
- `OrderParams` uses NT `OrderType` and `TimeInForce` fields at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:113-118`.
- Archetype validation converts raw parameters into `ParametersBlock` and validates entry/exit order combinations at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:121-145`.
- Runtime mapping reads `parameters_block` and inserts entry/exit order config into the raw strategy table at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:278-290` and `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:430-455`.
- Runtime mapping serializes order fields with NT enum lower-case names and booleans at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:662-682`.
- Current committed entry validation supports taker `Fok` and maker `Gtc/post_only=true` at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:819-849`.
- Current committed exit validation supports taker market `Ioc` and maker limit `Gtc/post_only=true` at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:856-888`.
- Strategy order construction passes configured order fields into NT `order_factory().limit(...)` and `order_factory().market(...)` at `src/strategies/binary_oracle_edge_taker.rs:3464-3510` and `src/strategies/binary_oracle_edge_taker.rs:4812-4868`.
- Entry and exit submit paths build configured NT orders, then submit through `submit_order_with_decision_evidence` at `src/strategies/binary_oracle_edge_taker.rs:3520-3587` and `src/strategies/binary_oracle_edge_taker.rs:3680-3801`.
- Submit wrapper records decision evidence, checks submit admission, and calls NT `self.submit_order(order, None, Some(client_id), None)` at `src/strategies/binary_oracle_edge_taker.rs:3450-3462`.

## Decision: Bolt evidence gap remains adapter HTTP payload coverage

**Evidence**:

- Bolt can prove config validation, raw runtime mapping, NT order-object construction, and existing `self.submit_order(...)` wrapper.
- NT dependency tests prove adapter HTTP serialization of `postOnly`.
- No bolt test currently drives a bolt-built strategy order through pinned NT Polymarket execution client and captures the real `/order` payload.

**Rationale**: This gap must be explicit. Static path proof is not live smoke proof.

**Alternatives considered**: Claim end-to-end live readiness from unit tests. Rejected.

## Current Branch Evidence

- Worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/maker-order-proof`
- Branch: `codex/maker-order-proof`
- Current committed head before this Speckit control work: `97cbf828423578e09a604bf31bdaa91ec3573df3`
- Commit `97cbf828423578e09a604bf31bdaa91ec3573df3` is `feat: enable config-driven maker orders`, touching `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`, `src/strategies/binary_oracle_edge_taker.rs`, `tests/config_parsing.rs`, `tests/bolt_v3_strategy_registration.rs`, docs, config, and fixtures. It is a committed candidate implementation, not a clean pre-implementation base.
- Current dirty inventory after removing unapproved GTD edits: `M AGENTS.md` and `?? specs/022-nt-maker-order-scope/`.
- Current production/test/schema dirty diff is empty for `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`, `src/strategies/binary_oracle_edge_taker.rs`, `tests/config_parsing.rs`, `tests/bolt_v3_strategy_registration.rs`, and `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`.
- No-mistakes daemon: running, but active run observed for a different branch, so not evidence for this branch.
- PR #388 status observed via `gh pr view 388`: open, trade-readiness scope, not merged, and not maker-order implementation proof.

## Pre-Implementation Review Finding Resolution

- Internal adversarial review requested changes because T004/T005 were checked before this file contained full bolt line references. Resolution: added the bolt config-to-submit evidence section above.
- Internal adversarial review found GTD expiry policy was not NT-derived. Resolution: GTD remains recognized as NT-supported but blocked for bolt-v3 until explicit TOML expiry config is designed and approved.
- Internal adversarial review found the pre-implementation gate was compromised by dirty GTD code/test/doc edits. Resolution: unapproved dirty GTD edits were removed; remaining production/test/doc dirty diff is empty as of `git diff -- src/bolt_v3_archetypes/binary_oracle_edge_taker.rs src/strategies/binary_oracle_edge_taker.rs tests/config_parsing.rs tests/bolt_v3_strategy_registration.rs docs/bolt-v3/2026-04-25-bolt-v3-schema.md`.
- DeepSeek job `job_0a9dbdbd-ef4f-4852-b0f3-cbd639e7ca38` returned REQUEST_CHANGES. Findings: missing bolt path mapping, missing dirty-file inventory, quickstart missing from review scope, GTD expiry edge case underspecified, and possible ambiguity around post-only/TIF rejection. Resolution so far: bolt path mapping added, dirty inventory added, GTD blocked unless explicit expiry contract is approved, and current contract remains GTC-only.
- GLM job `job_5579578e-5e46-473d-9c17-52269dedff80` returned REQUEST_CHANGES. Findings overlapped with DeepSeek and Gemini: GTD expiry underspecified and bolt path mapping missing. Resolution so far: GTD blocked and bolt path mapping added.
- Gemini job `c2b437fc-41d4-41d2-beb9-8b3d27960e1f` returned REQUEST_CHANGES. Findings: missing TOML config for GTD expiry and speculative reuse of `post_only_requote_interval_ms`. Resolution: GTD blocked and that reuse rejected.
- Kimi job `6df6c1e5-744d-43ff-a0f0-c50bd1fa5774` returned REQUEST_CHANGES. Findings: commit `97cbf828423578e09a604bf31bdaa91ec3573df3` is already an implementation commit, pre-implementation gates were not followed before that commit, T005 was missing in the reviewed snapshot, and no red-test evidence exists for that commit. Resolution so far: current docs now identify `97cbf828423578e09a604bf31bdaa91ec3573df3` as candidate implementation, not process proof. Remaining open issue: decide whether the candidate commit can be accepted after audit or must be replayed from a clean base to satisfy TDD evidence.
- Claude job `1318ca11-a6f3-4e4a-868e-e6f83a689040` failed with `review_not_completed` / `permission_blocked`; no verdict recorded.
- These initial external reviews were run against an older artifact snapshot. Tasks T007-T012 remain open until review is rerun against the current Speckit artifacts.
- Current-snapshot internal adversarial review returned REQUEST_CHANGES. Findings: T004 overclaimed HTTP `postOnly` serialization without enough line evidence, and T013 had an unresolved replay-vs-accept decision.
- T013 resolution: commit `97cbf828423578e09a604bf31bdaa91ec3573df3` must not be treated as TDD/process proof. The candidate implementation may be accepted only after replay proof in a clean `origin/main` worktree shows tests fail before the production diff and pass after the production diff, or after an explicit user waiver. Without replay proof or waiver, completion cannot be claimed.
- Final exact-current internal review approved T007; it verified the Speckit pointer, HTTP `postOnly` evidence, T002 disposition, T013 replay decision, and GTC-only/GTD-blocked scope.
- Final exact-current Claude job `5d53d875-25ab-4315-b9fa-64ffba4141ac` approved T008. It reviewed the 7-file Speckit scope and found no blocking findings; it noted independent git/source verification was partly NOT REVIEWED due permission limits.
- Final exact-current Gemini job `8cd48da2-cd37-4bb0-8043-f86bcfdcd51f` approved T009 with no blocking or non-blocking findings.
- Final exact-current Kimi job `9489c55a-f4eb-4bf5-a66f-4d9b2632acd8` approved T010. It verified `HEAD` `97cbf828423578e09a604bf31bdaa91ec3573df3`, dirty inventory, empty production/test/schema diff, and spot-checked cited NT and bolt line evidence.
- Final exact-current DeepSeek job `job_e0c193a6-9911-4143-940a-c352f5ed715d` approved T011 with no blocking findings.
- Final exact-current GLM job `job_3fa4238a-22d1-43b2-8731-640bd1579797` approved T012 with no blocking findings. Non-blocking concerns were portability of local paths, future mitigation for the adapter HTTP payload coverage gap, and eventual explanation or exclusion of `AGENTS.md`.
- T013 is resolved for pre-implementation gating: all current pre-implementation findings are either resolved or explicitly documented, and Phase 4 remains blocked until clean replay proof records red-before/green-after evidence.

## TDD Replay Evidence

- T014 clean replay worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/maker-order-replay`.
- T014 base SHA: `831368756bf5a7f8398944502dcce5fcc7c7952d` (`origin/main` at replay creation).
- T014 initial replay worktree status: clean.
- Candidate diff from `origin/main..97cbf828423578e09a604bf31bdaa91ec3573df3` includes unrelated workflow files; replay must apply only the approved maker-order files, not stale workflow changes.
- T015 red evidence: `CARGO_TARGET_DIR=/tmp/bolt-v2-maker-replay-red /Users/spson/.cargo/bin/cargo test post_only_gtc -- --nocapture` failed on clean replay after tests only. `bolt_v3_archetype_accepts_post_only_gtc_entry_order` and `bolt_v3_archetype_accepts_post_only_gtc_exit_order` failed because validation still allowed only taker entry FOK and taker exit IOC.
- T016 base-green evidence: the same red run showed `binary_oracle_runtime_mapping_preserves_post_only_gtc_entry_order` and `binary_oracle_runtime_mapping_preserves_post_only_gtc_exit_order` already passed before production changes, so no runtime-mapping production diff was required.
- T017 red evidence: `CARGO_TARGET_DIR=/tmp/bolt-v2-maker-replay-red /Users/spson/.cargo/bin/cargo test post_only_entry_submission_price_uses_passive_book_price -- --nocapture` failed before production changes with `left: Some(0.41), right: Some(0.4)`.
- T017 red evidence: `CARGO_TARGET_DIR=/tmp/bolt-v2-maker-replay-red /Users/spson/.cargo/bin/cargo test post_only_exit_submission_price_uses_passive_book_price -- --nocapture` failed before production changes with `left: Some(0.44), right: Some(0.45)`.
- T018/T019 production replay applied only the minimal maker files: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` and `src/strategies/binary_oracle_edge_taker.rs`. It did not add `post_only_requote_interval_ms`, pending timestamps, or cancel/resubmit requote logic.
- T020 schema replay updated only the documented valid `binary_oracle_edge_taker` order combinations to include GTC post-only maker entry and exit. It did not add a runtime requote field.
- T021 green evidence: `CARGO_TARGET_DIR=/tmp/bolt-v2-maker-replay-green /Users/spson/.cargo/bin/cargo test post_only -- --nocapture` passed on the clean replay diff. Passing targeted tests included 2 strategy unit tests, 2 runtime-mapping tests, and 3 config/NT-source evidence tests.
- T022 cleanup evidence: scoped ai-slop pass over changed files found the earlier speculative requote surface removed from the clean branch. `rg -n "post_only_requote|submitted_at_ms|cancel_stale_post_only|requote" src tests config docs` returned no matches. No additional cleanup edits were required beyond replacing the stale quickstart filter with the actual NT serialization test filter.
- T023 full verification evidence: static NT evidence checks exited 0; `/Users/spson/.cargo/bin/cargo fmt -- --check` exited 0; `git diff --check` exited 0; `CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test` exited 0, including 250 lib tests, all integration tests, and doc tests.
- T030/T031/T032 push evidence: clean branch `codex/maker-order-proof-clean` was created from `origin/main` replay worktree and pushed to `origin/codex/maker-order-proof-clean` at commit `c625ff4e` (`feat: enable NT-matched maker orders`).
- Post-implementation external audit gate remains open: `AGENTS.md` forbids external red-team review while work is uncommitted/unpushed and before the exact PR head CI is green. No PR head CI was created or verified in this step, so T024-T029 are deliberately left open.
- PR #434 CI evidence: first exact-head CI run for `fd4fbfc6cbde21fdc07bf8607ded44ced3b51e30` failed `source-fence` because `tests/config_parsing.rs` used forbidden bolt naming token `post_only` while constructing NT `PostOrderParams`. Fix keeps bolt test naming on `is_post_only` and verifies pinned NT source declares camelCase `postOnly`.
- Local fix verification: `just source-fence` exited 0 after the naming fix; `CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test polymarket_post_order_params_declares_camel_case_is_post_only_flag -- --nocapture` exited 0; `/Users/spson/.cargo/bin/cargo fmt -- --check` exited 0; `git diff --check` exited 0.
- T038 hermetic NT evidence fix: `tests/config_parsing.rs` no longer includes `$HOME/.cargo/git/checkouts/.../query.rs`. It includes committed fixture `tests/fixtures/nt_polymarket_query_post_order_params_7c2aafb.txt`, which records NT revision `7c2aafb30fb143069c915a3f2057bb12174405f6`, source path `crates/adapters/polymarket/src/http/query.rs`, source SHA-256 `c81bc63f9bfabff4c1dc7a3fcff33ee7c9f8c119e80e629a94afc59590238ed0`, and extracted lines for `PostOrderParams` plus `postOnly` serialization tests.
- T039 local verification after T038: `cargo fmt -- --check` exited 0; `git diff --check` exited 0; `cargo test polymarket_post_order_params_declares_camel_case_is_post_only_flag -- --nocapture` exited 0. First `just source-fence` attempt failed only on sandbox denial to `/Users/spson/.cache/rust-verification/bolt-v2/cache.lock`; escalated rerun exited 0, including naming, runtime-capture YAML, `bolt_v3_controlled_connect` (11 passed), and `bolt_v3_production_entrypoint` (5 passed).
- T037 Claude post-implementation branch-diff job `4116413c-6ab4-49cb-abd7-a0579c260a94` returned APPROVE. Non-blocking concerns were the brittle NT source include, missing direct order-object assertion, maker entry sizing label semantics, and passive maker exit not guaranteeing immediate flattening; these are resolved by T038, T045, existing price evidence, and T046 documentation.
- T040 DeepSeek source-excerpt follow-up job `job_5b84c8e7-7211-4571-b5de-8768b5a281d1` returned APPROVE. It closed the strategy-packet scope gaps for archetype validation, configured order construction, `is_post_only`/TIF pass-through to NT limit orders, single submit wrapper, maker passive prices, taker unchanged, and GTD blocked. Remaining gap was live NT adapter/canary scope, already documented as outside this unit/static proof.
- T041 GLM source-excerpt follow-up job `job_2b959994-47d0-4904-929b-41ada342e7cd` returned APPROVE. It closed the order construction, submit path, archetype validation, maker price, taker-unchanged, and GTD-blocked gaps. Non-blocking recommendation for a direct order-object construction test is resolved by T045.
- T043 Gemini source-excerpt follow-up job `321266b9-c6a9-4e36-b8cc-5e1e56441e37` returned APPROVE against head `52ab7719f6b0770cf3ca45b7c82f5cd8fe843f8d`. It found no blocking findings and no scope gaps; its only concern was passive GTC maker orders may rest unfilled, resolved by T046 documentation.
- T042 Kimi source-excerpt follow-up job `8345bbdc-6396-4803-a6d1-7a45219ae20c` returned APPROVE after 925267 ms. Non-blocking concerns: short-side maker orders are not tested because the contract is long-side only; market post-only runtime rejection has no direct unit test but is defense-in-depth behind archetype validation; the hermetic NT fixture must be updated if the pinned NT revision changes.
- T044 GTD-negative validation proof: `cargo test bolt_v3_archetype_rejects_gtd_time_in_force_until_expiry_policy_exists -- --nocapture` exited 0. The test proves `gtd` parses via NT `TimeInForce` but is rejected by the `binary_oracle_edge_taker` archetype until a TOML-owned expiry policy exists.
- T045 direct order-object proof: `cargo test post_only_maker_order_objects_preserve_nt_limit_gtc_fields -- --nocapture` exited 0 after registering the test strategy with NT core. The test asserts maker entry and exit build as `OrderAny::Limit` with `OrderType::Limit`, `TimeInForce::Gtc`, `is_post_only=true`, `expire_time=None`, and expected side/price.
- T046 forced-flat/passive-fill documentation: `specs/022-nt-maker-order-scope/contracts/maker-order-config.md` and `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` now state maker exit is passive at the touch and does not guarantee immediate flattening; operators requiring immediate flattening must configure taker exit until a separate TOML-owned forced-exit override exists.
- T047 audit finding disposition: all blocking findings are resolved. Documented residuals are accepted scope boundaries: short-side maker is outside the long-side contract; live NT adapter HTTP/canary execution remains outside unit/static proof; market post-only runtime rejection is defense-in-depth behind validation; GTD remains blocked until explicit TOML expiry policy; fixture staleness is controlled by the pinned NT revision and SHA-256 assertion.
- T048 local verification before commit: `cargo fmt -- --check` exited 0; `git diff --check` exited 0; `cargo test bolt_v3_archetype_rejects_gtd_time_in_force_until_expiry_policy_exists -- --nocapture` exited 0; `cargo test post_only_maker_order_objects_preserve_nt_limit_gtc_fields -- --nocapture` exited 0; escalated `just source-fence` exited 0 after sandbox denial on the shared cache lock; `cargo test` exited 0, including 251 lib tests, all integration tests, and doc tests.
