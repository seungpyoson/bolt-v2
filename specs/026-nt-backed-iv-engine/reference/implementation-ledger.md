# IV Engine Implementation Ledger

**Feature**: `specs/026-nt-backed-iv-engine/`
**Scope**: NT-backed IV engine only. FV, RV, market-maker behavior, broad sidecar collectors, and strategy-specific IV logic are out of scope.

## Phase 1 Setup

| Task | Evidence | Status |
|---|---|---|
| `T001` | `reference/repository-truth.md` records fetch/prune, branch/head/main/merge-base, PR state, changed files, and diffstat. | Complete |
| `T002` | `reference/overlap-ledger.md` refreshed open PR and issue overlap at head `f994ae15198502aee9227aea5e813d12b8d5bf92`; no open PRs and no fully ported issues found. | Complete |
| `T003` | `reference/evidence-ledger.md` records current-main evidence and missing/implemented status by requirement group. | Complete |
| `T004` | `reference/nt-evidence.md` records the pinned NT git revision from `Cargo.toml` and `Cargo.lock`. | Complete |
| `T005` | `src/bolt_v3_iv/mod.rs` establishes the Rust module boundary without runtime behavior. | Complete |
| `T006` | `src/lib.rs` exports `bolt_v3_iv`. | Complete |
| `T007` | `tests/fixtures/bolt_v3_iv/README.md` establishes the fixture directory. | Complete |
| `T008` | `tests/fixtures/bolt_v3_iv/evidence.md` records expected fixture inventory. | Complete |
| `T009` | This ledger records implementation progress. | Complete |

## Verification Log

| Command | Outcome | Notes |
|---|---|---|
| `git diff --check` | PASS | No whitespace errors. |
| `cargo fmt --check` | PASS | Formatter check exited 0. |
| `cargo test --locked bolt_v3_iv` | PASS | Compiled the crate graph and ran filtered test binaries; no matching tests exist yet for the Phase 1 boundary, and all filtered binaries exited 0. |

`just source-fence`, clippy, full IV test targets, and CI are not complete for the full feature. They remain tracked by polish tasks `T123` and `T124` after implementation tests exist.

## TDD Boundary

Phase 1 adds documentation, module boundaries, and fixture inventory only. Runtime behavior starts in Phase 2 and later user-story tasks, where each production behavior must have RED/GREEN evidence before being marked complete.

## Phase 2 Foundational Types

| Task | Evidence | Status |
|---|---|---|
| `T010` | `src/bolt_v3_iv/mod.rs` declares `audit`, `authz`, `bounds`, `error`, `health`, `provenance`, `selector`, `time`, and `types`. | Complete |
| `T011` | `src/bolt_v3_iv/types.rs` defines source/product/basis/convention enums. | Complete |
| `T012` | `src/bolt_v3_iv/error.rs` defines `IvRejectReason` and the required-reason list. | Complete |
| `T013` | `src/bolt_v3_iv/time.rs` defines `UnixNanos`. | Complete |
| `T014` | `src/bolt_v3_iv/bounds.rs` defines `IvNumericBounds`, `IvBoundUnit`, and `IvConventionBounds`. | Complete |
| `T015` | `src/bolt_v3_iv/provenance.rs` defines `IvPolicyDecision`. | Complete |
| `T016` | `src/bolt_v3_iv/provenance.rs` defines `IvProvenance` and typed helper identity. | Complete |
| `T017` | `src/bolt_v3_iv/selector.rs` defines the source/query selector union and product mapping. | Complete |
| `T018` | `src/bolt_v3_iv/authz.rs` defines `IvSelectorAuthorization`. | Complete |
| `T019` | `src/bolt_v3_iv/audit.rs` defines `IvAuditPolicy`, `IvAuditHandleId`, raw product kinds, and retention marker. | Complete |
| `T020` | `src/bolt_v3_iv/health.rs` defines source-health states, transitions, and current-query eligibility. | Complete |
| `T021` | `tests/bolt_v3_iv_support.rs` provides shared IV fixture builders. | Complete |
| `T022` | `tests/bolt_v3_iv_source_fence.rs` provides the IV source-fence entrypoint placeholder. | Complete |
| `T023` | `justfile` wires `--test bolt_v3_iv_source_fence` into the managed `source-fence` cargo test invocation. | Complete |
| `T024` | This section records the foundational RED/GREEN evidence. | Complete |

## Phase 2 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_foundation` | RED | Initial run was blocked by the Rust verification disk-pressure guard. After removing the generated managed target cache, the RED run failed with `E0432` unresolved imports for missing `bolt_v3_iv` modules. |
| `cargo test --locked --test bolt_v3_iv_foundation` | GREEN | 4 tests passed after adding foundational modules. |
| `cargo test --locked --test bolt_v3_iv_source_fence` | GREEN | 1 test passed for the IV source-fence entrypoint placeholder. |
| `cargo fmt --check` | GREEN | Final formatter check exited 0 after applying `cargo fmt`. |
| `git diff --check` | GREEN | No whitespace errors after Phase 2 edits. |

`just source-fence`, clippy, full IV user-story targets, and CI remain open for the broader feature and are still tracked by `T123` and `T124`.

## Phase 3 User Story 1 Capability Inventory

| Task | Evidence | Status |
|---|---|---|
| `T025` | `tests/bolt_v3_iv_capability.rs` asserts Cargo metadata plus `Cargo.lock` resolve to the pinned NT checkout and a single NT revision. | Complete |
| `T026` | `tests/bolt_v3_iv_capability.rs` asserts seed-family discovery covers model data, data actor subscriptions, data-engine publications, msgbus topics, option-chain manager, greeks helper, adapter, and custom-data surfaces. | Complete |
| `T027` | `tests/bolt_v3_iv_capability.rs` asserts the whole-checkout sweep includes every FR-054 term. | Complete |
| `T028` | `tests/bolt_v3_iv_capability.rs` asserts unclassified candidates are rejected before fixture-backed ledger validation passes. | Complete |
| `T029` | RED evidence below records the missing capability module failure. | Complete |
| `T030` | `src/bolt_v3_iv/capability.rs` implements the NT Cargo metadata and lockfile resolver. | Complete |
| `T031` | `src/bolt_v3_iv/capability.rs` implements seed-family scanning for the required NT capability families. | Complete |
| `T032` | `src/bolt_v3_iv/capability.rs` implements recursive public-symbol candidate discovery across Rust source files. | Complete |
| `T033` | `src/bolt_v3_iv/capability.rs` implements explicit candidate classification and unclassified-candidate rejection. | Complete |
| `T034` | `src/bolt_v3_iv/capability.rs` implements the capability ledger TOML fixture loader. | Complete |
| `T035` | `tests/fixtures/bolt_v3_iv/capability-ledger.toml` records supported fixture classifications. | Complete |
| `T036` | GREEN evidence below records the passing focused US1 test target. | Complete |

## Phase 3 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_capability` | RED | Failed with `E0432` unresolved import because `bolt_v2::bolt_v3_iv::capability` did not exist. |
| `cargo test --locked --test bolt_v3_iv_capability` | GREEN | 4 tests passed after adding the capability resolver, scanners, explicit ledger model, and TOML fixture. |

NT-first decisions for US1: the Cargo resolver derives NT evidence from `cargo metadata` and `Cargo.lock`; scanners operate against source files from the resolved checkout shape; no FV/RV, strategy-specific, venue-specific, or sidecar collection behavior was introduced.

## Phase 4 User Story 2 Subscription Planning

| Task | Evidence | Status |
|---|---|---|
| `T037` | `tests/bolt_v3_iv_subscription.rs` asserts option-greeks sources map to NT option-greeks subscribe operations with configured client, selector, params, and generation. | Complete |
| `T038` | `tests/bolt_v3_iv_subscription.rs` asserts option-chain sources map to NT option-chain subscribe operations with configured series and strike-range selector. | Complete |
| `T039` | `tests/bolt_v3_iv_subscription.rs` asserts aggregate-greeks sources map to NT greeks-topic subscribe operations. | Complete |
| `T040` | `tests/bolt_v3_iv_subscription.rs` asserts custom implied-volatility sources map to ledger-compatible NT custom-data subscribe operations. | Complete |
| `T041` | `tests/bolt_v3_iv_subscription.rs` asserts reload unsubscribes stale generations, subscribes the new generation, and emits source-removal operations for deleted sources. | Complete |
| `T042` | RED evidence below records the missing subscription/runtime module failure. | Complete |
| `T043` | `src/bolt_v3_iv/subscription.rs` defines `IvSubscriptionPlan`, profile/source subscription config, lifecycle, runtime operation, and NT source-kind enums. | Complete |
| `T044` | `src/bolt_v3_iv/subscription.rs` maps option-greeks sources to subscribe/unsubscribe option-greeks operations. | Complete |
| `T045` | `src/bolt_v3_iv/subscription.rs` maps option-chain sources to subscribe/unsubscribe option-chain operations. | Complete |
| `T046` | `src/bolt_v3_iv/subscription.rs` maps aggregate-greeks sources to subscribe/unsubscribe aggregate-greeks topic operations. | Complete |
| `T047` | `src/bolt_v3_iv/subscription.rs` maps custom implied-volatility sources to subscribe/unsubscribe custom-data operations. | Complete |
| `T048` | `src/bolt_v3_iv/subscription.rs` implements start, stop, reload, unsubscribe, and source-removal planning. | Complete |
| `T049` | `src/bolt_v3_iv/runtime.rs` defines the runtime binding adapter trait and plan outcome/source-health bridge. | Complete |
| `T050` | GREEN evidence below records the passing focused US2 test target. | Complete |

## Phase 4 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_subscription` | RED | Failed with unresolved imports because `bolt_v2::bolt_v3_iv::runtime` and `bolt_v2::bolt_v3_iv::subscription` did not exist. |
| `cargo test --locked --test bolt_v3_iv_subscription` | GREEN | 5 tests passed after adding typed subscription planning, reload/source-removal planning, and runtime binding adapter outcomes. |

NT runtime mapping decisions for US2: subscription plans carry only configured profile/source/client/selector/params/generation values; source kind to NT operation mapping is explicit and typed; no strategy, venue, market, asset, instrument, cadence, source ID, timeout, or policy runtime value is introduced in core logic.

## Phase 5 User Story 3 Raw Preservation And Indexed Products

| Task | Evidence | Status |
|---|---|---|
| `T051` | `tests/bolt_v3_iv_ingest.rs` asserts option-greeks raw payload preservation and mark/bid/ask IV plus greeks indexing. | Complete |
| `T052` | `tests/bolt_v3_iv_ingest.rs` asserts option-chain slices build retained smiles and surface views without interpolation. | Complete |
| `T053` | `tests/bolt_v3_iv_ingest.rs` asserts aggregate-greeks raw events index to aggregate-greeks products. | Complete |
| `T054` | `tests/bolt_v3_iv_ingest.rs` asserts custom implied-volatility events index as custom IV evidence. | Complete |
| `T055` | `tests/bolt_v3_iv_store.rs` asserts raw payload access is audit/replay/test-only and strategy role access is denied. | Complete |
| `T056` | `tests/bolt_v3_iv_store.rs` asserts indexed product provenance is complete and incomplete provenance rejects. | Complete |
| `T057` | RED evidence below records the missing ingest/store/raw-access/provenance validation failure. | Complete |
| `T058` | `src/bolt_v3_iv/ingest.rs` implements `IvRawEvent` preservation and raw provenance creation. | Complete |
| `T059` | `src/bolt_v3_iv/store.rs` indexes `IvPoint` and `IvGreeksPoint` from option-greeks payloads. | Complete |
| `T060` | `src/bolt_v3_iv/store.rs` constructs retained `IvSmile` views from option-chain slices. | Complete |
| `T061` | `src/bolt_v3_iv/store.rs` constructs `IvSurface` views from retained smiles. | Complete |
| `T062` | `src/bolt_v3_iv/store.rs` indexes `IvAggregateGreeks` products. | Complete |
| `T063` | `src/bolt_v3_iv/store.rs` indexes custom `IvEvidence` products. | Complete |
| `T064` | `src/bolt_v3_iv/raw_access.rs` enforces audit/replay/test-only raw event access. | Complete |
| `T065` | `src/bolt_v3_iv/provenance.rs` builds raw-event provenance and validates required provenance fields. | Complete |
| `T066` | GREEN evidence below records the passing focused US3 test targets. | Complete |

## Phase 5 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_ingest --test bolt_v3_iv_store` | RED | Failed with unresolved imports because `ingest`, `store`, `raw_access`, and `validate_iv_provenance` did not exist. |
| `cargo test --locked --test bolt_v3_iv_ingest --test bolt_v3_iv_store` | GREEN | 6 tests passed after adding raw preservation, indexed products, audit-only raw reads, and provenance validation. |

Raw-boundary decisions for US3: raw payloads stay in `IvRawEvent` and are reachable only through `raw_access`; strategy-role raw access rejects; strategy-safe products carry provenance references to raw event IDs but do not expose raw payload values.

## Phase 6 User Story 4 NT Helper Derivation

| Task | Evidence | Status |
|---|---|---|
| `T067` | `tests/bolt_v3_iv_derive.rs` asserts helper policy selection chooses the configured NT helper symbol. | Complete |
| `T068` | `tests/bolt_v3_iv_derive.rs` asserts complete inputs derive IV through the NT helper and record helper provenance. | Complete |
| `T069` | `tests/bolt_v3_iv_derive.rs` asserts every required derived-input field rejects when missing. | Complete |
| `T070` | `tests/bolt_v3_iv_derive.rs` asserts stale/skewed input timestamps reject before helper invocation. | Complete |
| `T071` | `tests/bolt_v3_iv_derive.rs` asserts expired operator-configured rate/carry inputs reject before helper invocation. | Complete |
| `T072` | `tests/bolt_v3_iv_derive.rs` asserts helper output outside configured IV bounds rejects. | Complete |
| `T073` | RED evidence below records the missing derive module failure. | Complete |
| `T074` | `src/bolt_v3_iv/derive.rs` implements `IvHelperPolicy` and typed NT helper symbol selection. | Complete |
| `T075` | `src/bolt_v3_iv/derive.rs` resolves required derived input fields from the typed input set. | Complete |
| `T076` | `src/bolt_v3_iv/derive.rs` validates missing, non-finite, non-positive, skewed, and expired inputs before helper invocation. | Complete |
| `T077` | `src/bolt_v3_iv/derive.rs` invokes `nautilus_model::data::imply_vol_and_greeks` from the pinned NT dependency. | Complete |
| `T078` | `src/bolt_v3_iv/derive.rs` validates helper IV output against configured numeric/convention bounds. | Complete |
| `T079` | `src/bolt_v3_iv/derive.rs` attaches `IvHelperIdentity` and `IvPolicyDecision::Helper` to derived-output provenance. | Complete |
| `T080` | GREEN evidence below records the passing focused US4 test target. | Complete |

## Phase 6 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_derive` | RED | Failed with unresolved import because `bolt_v2::bolt_v3_iv::derive` did not exist. |
| `cargo test --locked --test bolt_v3_iv_derive` | GREEN | 6 tests passed after adding helper policy selection, complete-input derivation, fail-closed input validation, output-bound validation, and helper provenance. |

Helper NT-source decisions for US4: the wrapper calls `nautilus_model::data::imply_vol_and_greeks`; test fixtures compute the expected price/vol from `nautilus_model::data::black_scholes_greeks`; no strategy-local or non-NT IV helper path was introduced.

## Phase 7 User Story 5 Config And Lifecycle

| Task | Evidence | Status |
|---|---|---|
| `T081` | `tests/bolt_v3_iv_config.rs` asserts full IV TOML profile parsing into typed Rust config. | Complete |
| `T082` | `tests/bolt_v3_iv_config.rs` asserts unknown IV schema versions reject before subscription planning. | Complete |
| `T083` | `tests/bolt_v3_iv_config.rs` asserts selector/source mismatches reject with a field-specific diagnostic. | Complete |
| `T084` | `tests/bolt_v3_iv_config.rs` asserts zero memory bounds and empty source selectors reject. | Complete |
| `T085` | `tests/bolt_v3_iv_policy.rs` asserts projection, interpolation/extrapolation, fallback, and quorum policy behavior. | Complete |
| `T086` | `tests/bolt_v3_iv_policy.rs` asserts policy outputs carry typed `IvPolicyDecision` variants. | Complete |
| `T087` | `tests/bolt_v3_iv_live_integration.rs` asserts source-health transitions and retention eviction keep current views bounded. | Complete |
| `T088` | RED evidence below records missing config/policy/retention/root IV wiring. | Complete |
| `T089` | `src/bolt_v3_iv/config.rs` implements IV TOML schema parsing for `IvRootConfig`, `IvProfile`, and `IvSourceConfig`. | Complete |
| `T090` | `src/bolt_v3_iv/config.rs` validates supported IV schema version. | Complete |
| `T091` | `src/bolt_v3_iv/config.rs` validates selector/source/product shape and reuses subscription planning validation. | Complete |
| `T092` | `src/bolt_v3_iv/bounds.rs` implements reusable numeric/convention bound acceptance; `config.rs` validates positive memory bounds. | Complete |
| `T093` | `src/bolt_v3_iv/policy.rs` implements projection policy skew rejection and typed decision output. | Complete |
| `T094` | `src/bolt_v3_iv/policy.rs` implements interpolation and configured extrapolation rejection. | Complete |
| `T095` | `src/bolt_v3_iv/policy.rs` implements ordered fallback policy. | Complete |
| `T096` | `src/bolt_v3_iv/policy.rs` implements quorum policy. | Complete |
| `T097` | `src/bolt_v3_iv/health.rs` state machine is exercised by `tests/bolt_v3_iv_live_integration.rs`. | Complete |
| `T098` | `src/bolt_v3_iv/store.rs` implements retention eviction for raw and indexed IV products. | Complete |
| `T099` | `src/bolt_v3_config.rs` accepts optional `[iv]` root config and `src/bolt_v3_validate.rs` validates it through root validation. | Complete |
| `T100` | GREEN evidence below records the passing US5 test targets. | Complete |

## Phase 7 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_config --test bolt_v3_iv_policy --test bolt_v3_iv_live_integration` | RED | Failed with unresolved imports for `config`, `policy`, missing `IvRetentionPolicy`/`enforce_retention`, and missing root `iv` field. |
| `cargo test --locked --test bolt_v3_iv_config --test bolt_v3_iv_policy --test bolt_v3_iv_live_integration` | GREEN | 9 tests passed after adding typed IV TOML config, policy functions, retention eviction, and root config integration. |

Config group-by-change decisions for US5: one `[iv]` root block owns IV profiles; each `IvProfile` owns its sources, strategy IDs, selector authorization, enabled products, memory bounds, and source selectors; changing a source ID, selector fingerprint, or strategy authorization stays inside that profile block.

## Phase 8 User Story 6 Strategy Query API

| Task | Evidence | Status |
|---|---|---|
| `T101` | `tests/bolt_v3_iv_query.rs` asserts profile-wide strategy handles can query an indexed IV point and receive provenance. | Complete |
| `T102` | `tests/bolt_v3_iv_query.rs` asserts selector-scoped authorization requires matching source ID and selector fingerprint. | Complete |
| `T103` | `tests/bolt_v3_iv_query.rs` asserts strategy query handles reject raw payload requests. | Complete |
| `T104` | `tests/bolt_v3_iv_live_integration.rs` asserts root IV config builds strategy query handle registrations and live IV lifecycle plans. | Complete |
| `T105` | `tests/bolt_v3_iv_source_fence.rs` rejects strategy-local NT IV subscription calls. | Complete |
| `T106` | `tests/bolt_v3_iv_source_fence.rs` rejects strategy-local NT helper IV derivation calls. | Complete |
| `T107` | `tests/bolt_v3_iv_source_fence.rs` rejects raw audit reader and raw payload DTO imports in strategy source. | Complete |
| `T108` | `tests/bolt_v3_iv_source_fence.rs` rejects hardcoded IV runtime values in IV core source and scans current IV core files. | Complete |
| `T109` | RED evidence below records the missing query/config/registration/lifecycle failures. | Complete |
| `T110` | `src/bolt_v3_iv/query.rs` defines `IvQuery`, `IvProductQuery`, `IvRawPayloadQuery`, `IvQueryProduct`, `IvProjectedScalarIv`, `IvQueryError`, and `IvQueryHandle`. | Complete |
| `T111` | `src/bolt_v3_iv/authz.rs` authorizes profile-wide strategy product/source requests. | Complete |
| `T112` | `src/bolt_v3_iv/authz.rs` authorizes selector-scoped requests by product kind, source ID, and selector fingerprint. | Complete |
| `T113` | `src/bolt_v3_iv/query.rs` routes strategy-safe product queries through indexed store products, configured projection policies, and engine-owned derived input/helper state. | Complete |
| `T114` | `src/bolt_v3_iv/query.rs` rejects raw payload requests and raw event dereference on strategy handles. | Complete |
| `T115` | `src/bolt_v3_strategy_registration.rs` builds an IV query handle registry from root IV config and injects it into `StrategyRegistrationContext`. | Complete |
| `T116` | `src/bolt_v3_live_node.rs` derives IV start/stop lifecycle plans from root IV profiles. | Complete |
| `T117` | `tests/bolt_v3_iv_source_fence.rs` implements strategy and IV-core source-fence checks. | Complete |
| `T118` | GREEN evidence below records the passing US6 test targets. | Complete |

## Phase 8 RED/GREEN Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_query --test bolt_v3_iv_config --test bolt_v3_iv_live_integration --test bolt_v3_iv_source_fence` | RED | Failed with missing `bolt_v3_iv::query`, missing profile-owned `strategy_ids`/`selector_authorization`, missing `strategy_authorizations()`, and missing live/strategy registration functions. |
| `cargo test --locked --test bolt_v3_iv_query --test bolt_v3_iv_config --test bolt_v3_iv_live_integration --test bolt_v3_iv_source_fence` | GREEN | 19 tests passed after adding config-owned strategy authorization, query handles, raw rejection, strategy-registration IV handle registry, live IV lifecycle planning, and IV source-fence checks. |
| `cargo test --locked --test bolt_v3_iv_query` | RED | Projection/derived query gap reproduced: missing `with_projection_policies`, missing `with_helper_policies`, missing `IvQueryProduct::ProjectedScalarIv`, and missing `IvQueryProduct::DerivedIv`. |
| `cargo test --locked --test bolt_v3_iv_query` | GREEN | 5 tests passed after adding projected scalar routing, derived IV routing through engine-owned helper inputs, and boxed derived query products. |

Strategy-boundary decisions for US6: strategies receive IV access only through `IvQueryHandle`; raw payload dereference remains rejected on strategy handles; strategy registration builds handles from profile-owned TOML authorization; projected scalar and derived IV queries require engine-owned policy/input state; source-fence allows public query API imports while rejecting direct NT IV subscription calls, NT helper derivation, raw audit readers, raw IV DTO imports, and IV-core placeholder runtime hardcodes.

## Phase 9 Polish And Verification

| Task | Evidence | Status |
|---|---|---|
| `T119` | `quickstart.md` was updated to the implemented `[iv]` TOML schema. | Complete |
| `T120` | `contracts/iv-engine-api.md` was updated with final public query, registration, and lifecycle type names. | Complete |
| `T121` | This section records RED/GREEN and verification outcomes through the latest local head, including the post-review custom-data raw-preservation and fail-closed runtime/config fixes. | Complete |
| `T122` | `overlap-ledger.md` was refreshed at head `ebd1a09d790ee6b242a9de49189bcfb7e361dd6e`; no open overlap PRs/issues were found to close. | Complete |
| `T123` | Focused IV and config test commands below passed after the final query-route, post-review raw-preservation, and fail-closed runtime/config changes. | Complete |
| `T124` | Formatter, clippy, binary clippy, and source-fence gates below passed after the final query-route, post-review raw-preservation, and fail-closed runtime/config changes. | Complete |
| `T125` | `internal-review.md` records the internal adversarial review. | Complete |
| `T126` | `external-review.md` records PR #611 review status: CodeQL/Gemini threads replied to and resolved, no unresolved review threads found, and CodeQL green. | Complete |
| `T127` | `external-review.md` records that PR review comments were resolved and GitHub CI was rerun green on the reviewed PR head; final head status must be confirmed after the final push. | Complete |
| `T128` | `final-summary.md` records base/PR context, NT APIs used, GitHub CI verification, review status, and residual risk. | Complete |

## Phase 9 RED/GREEN And Gate Evidence

| Command | Outcome | Notes |
|---|---|---|
| `cargo test --locked --test bolt_v3_iv_query` | RED | Failed before the final query-route fix with missing projection/helper handle methods and missing projected/derived product variants. |
| `cargo test --locked --test bolt_v3_iv_query` | GREEN | 5 tests passed: profile-wide point query, selector-scoped auth, raw rejection, projected scalar IV, and derived IV. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_custom_data_ingest_preserves_original_json_in_raw_payloads` | RED | Failed before the raw-preservation fix because aggregate/custom-IV raw payload variants did not expose preserved NT custom-data JSON. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_custom_data_ingest_preserves_original_json_in_raw_payloads` | GREEN | Passed after runtime custom-data ingest stored serialized NT custom-data JSON on aggregate/custom-IV raw payload variants. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_unaccepted_convention` | RED | Failed before the convention fix because runtime source config did not carry or enforce `accepted_conventions`. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_unaccepted_convention` | GREEN | Passed after NT greeks convention names were checked against the configured source convention set and source health recorded `UnsupportedConvention`. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_missing_iv_basis` | RED | Failed before the missing-basis fix because greeks with no mark/bid/ask IV were accepted as successful no-op indexing. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_missing_iv_basis` | GREEN | Passed after store indexing returned `MissingIvBasis`, preserved raw evidence, and runtime recorded typed source health. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_engine_enforces_retention_after_failed_indexing_ingest` | RED | Failed before the retention fix because runtime state exposed no raw-event count evidence and failed store indexing skipped retention enforcement. |
| `cargo test --locked --test bolt_v3_iv_live_integration runtime_engine_enforces_retention_after_failed_indexing_ingest` | GREEN | Passed after runtime enforced profile retention after both successful and failed store ingestion. |
| `cargo test --locked --test bolt_v3_iv_store raw_payload_access_rejects_audit_as_of_before_event_receipt` | RED | Failed before the audit fix because `saturating_sub` made audit requests before receipt look age-zero. |
| `cargo test --locked --test bolt_v3_iv_store raw_payload_access_rejects_audit_as_of_before_event_receipt` | GREEN | Passed after raw audit access rejected `as_of_ns < received_ts_ns` with `RetentionMiss`. |
| `cargo test --locked --test bolt_v3_iv_config duplicate_custom_data_nt_topics_reject_before_runtime_binding` | RED | Failed before the config fix because duplicate aggregate/custom-IV custom-data topics loaded successfully. |
| `cargo test --locked --test bolt_v3_iv_config duplicate_custom_data_nt_topics_reject_before_runtime_binding` | GREEN | Passed after effective NT topic uniqueness validation covered aggregate greeks and custom implied-volatility custom data. |
| `cargo test --locked --test bolt_v3_iv_subscription runtime_engine_reload_updates_configured_source_generations` | RED | Failed before the reload fix because `IvRuntimeEngine` had no root-reload API. |
| `cargo test --locked --test bolt_v3_iv_subscription runtime_engine_reload_updates_configured_source_generations` | GREEN | Passed after `apply_iv_root_reload` refreshed runtime sources, policies, retention, derived inputs, and current generations while preserving query states. |
| `cargo test --locked --test bolt_v3_iv_subscription` | PASS | 15 subscription tests passed after adding coverage for reloaded-generation stale handles and removed-profile handle invalidation. |
| `cargo test --locked --test bolt_v3_iv_live_integration --test bolt_v3_iv_subscription --test bolt_v3_iv_config --test bolt_v3_iv_store` | PASS | 49 affected config/live/store/subscription tests passed after the fail-closed runtime/config fixes. |
| `cargo test --locked --test bolt_v3_iv_live_integration --test bolt_v3_iv_ingest` | PASS | 17 live integration tests and 4 ingest tests passed after the custom-data raw-preservation and fail-closed runtime/config fixes. |
| `cargo test --locked bolt_v3_iv` | PASS | Filtered command compiled the crate graph and all matching test binaries; all filtered binaries exited 0. |
| `cargo test --locked --test bolt_v3_iv_capability --test bolt_v3_iv_config --test bolt_v3_iv_live_integration --test bolt_v3_iv_subscription --test bolt_v3_iv_ingest --test bolt_v3_iv_store --test bolt_v3_iv_query --test bolt_v3_iv_policy --test bolt_v3_iv_derive --test bolt_v3_iv_source_fence --test config_parsing` | PASS | 83 IV tests and 191 `config_parsing` tests passed after the post-review raw-preservation and fail-closed runtime/config fixes. |
| `cargo fmt --check` | PASS | Formatter check exited 0 after the final edits. |
| `cargo clippy --locked --lib -- -D warnings` | PASS | Clippy initially rejected a large `IvQueryProduct::DerivedIv` enum variant; after boxing the derived output, clippy exited 0. |
| `cargo clippy --locked --bin bolt-v2 -- -D warnings` | PASS | Binary clippy exited 0 for the live-node wiring path. |
| `just source-fence` | PASS | Runtime literal audit, provider leak, core boundary, naming, dependency, schema-current, pure-Rust, legacy default, strategy policy, runtime-capture, controlled-connect, production-entrypoint, and IV source-fence checks passed. |
| `cargo test --locked` | PASS | Full local test suite exited 0 after internal review fixes. |
| PR #611 GitHub CI | PASS | Exact reviewed head `23004a14a1987215fb440bed6a3128c20591db3a` was green before this evidence update; passing checks included CI gate/test/nextest shards/clippy/deny/build/source-fence/fmt-check, CodeQL, actionlint, and Backtester CI. |

Phase 9 source-fence remediation: production `Default` derives and `unwrap_or_default` calls in IV code were replaced with explicit constructors or explicit fallback values. The runtime literal audit was extended for the IV module surface, and the schema-current verifier now accepts the active Speckit pointer for `specs/026-nt-backed-iv-engine/` alongside the existing order-intent pointer policy.
