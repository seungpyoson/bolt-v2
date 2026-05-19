# Phase 6 External Review Disposition

Date: 2026-05-13
Head reviewed: `main` / `origin/main` at `a5c60f2b6a4fe67fc80cf9d234f1512af09bec03`

## Scope

Reviewed plan artifacts and current main code surfaces for Phase 6 submit admission. No implementation branch was created.

## Results

| Provider | Job | Result | Disposition |
| --- | --- | --- | --- |
| Claude | `b61291ad-ee13-48c3-bdb5-f0cbd56ed42b` | `REQUEST_CHANGES` | Accepted. Plan/docs updated. |
| Gemini | `7d5a7063-4f36-4857-bd9c-eeb816832188` | `REQUEST_CHANGES` | Accepted. Plan/docs updated. |
| DeepSeek | `job_997753f3-fea5-4063-ad54-bec5345dd325` | `REQUEST_CHANGES` | Accepted. Plan/docs updated. |
| GLM | `job_9ec8dc4e-56c7-468a-9f00-7897647157d9` | `APPROVE` with non-blocking findings | Accepted non-blocking clarifications. Plan/docs updated. |
| Grok | `job_3ca149cd-925c-4a02-b14e-3f597063bb9f` | `REQUEST_CHANGES` | Accepted. Plan/docs updated. |
| Kimi | `411bcfaa-2336-4e89-a317-e9785678fc1e` | Failed audit: `review_not_completed`, `missing_verdict` | Skipped after user approval. Retry was stopped for runtime. |

## Second-pass Review

| Provider | Job | Result | Disposition |
| --- | --- | --- | --- |
| Claude | `e9a7bcd9-8274-4121-a093-d5ec1ad35641` | `APPROVE` | Plan ready for TDD. Full scope included `src/strategies/binary_oracle_edge_taker.rs`. |
| DeepSeek | `job_a7e36397-2c73-45e1-b0c0-7ab9a1604339` | `APPROVE` | Plan ready for TDD. Direct API packet excluded `src/strategies/binary_oracle_edge_taker.rs` due provider file-size cap. |
| GLM | `job_bff84adf-a9a9-4058-a83e-e98bc3cac5c4` | `APPROVE` | Plan ready for TDD. Direct API packet excluded `src/strategies/binary_oracle_edge_taker.rs` due provider file-size cap. |

## PR #324 Implementation Review

| Provider | Head | Result | Disposition |
| --- | --- | --- | --- |
| Claude | `96acf2eca164d20e29317a6d584a250f2dc865e0` | `APPROVE` | No blocking findings. |
| GPT | `96acf2eca164d20e29317a6d584a250f2dc865e0` | `REQUEST_CHANGES` | Partially accepted. Hardened runtime/gate public API so submit admission cannot be manually pre-armed from a constructed report or reached through a raw `LiveNode` deref bypass. Research-doc scope finding explicitly user-waived because the doc is non-runtime evidence and avoiding a separate docs PR is preferred. |

## Accepted Fixes

- Specified one shared admission-state carrier across build, strategy registration, and runner arming.
- Specified `BoltV3LiveNodeRuntime` as the planned carrier for `LiveNode` plus `Arc<BoltV3SubmitAdmissionState>`.
- Specified `run_bolt_v3_live_node` arms from the existing validated `BoltV3LiveCanaryGateReport` before runtime capture is wired.
- Added `NotArmed`, `AlreadyArmed`, count-cap, notional-cap, and non-positive-notional error contract.
- Specified one internal mutex for gate report, armed flag, and count mutation.
- Defined stale-arm as any arm attempt after one successful arm.
- Defined global budget semantics across all registered strategies.
- Defined entry, exit, and replace-submit candidates as consuming budget; plain cancel is excluded.
- Added cap-equality admission behavior.
- Added strategy-owned notional constraint and no core provider/market hardcoding.
- Strengthened source-fence scope to all strategy and archetype submit call sites.
- Clarified decision evidence is intent evidence, not NT submit proof.
- Clarified restart resets Phase 6 in-memory admission budget; NT restart reconciliation remains Phase 8 scope.
- Clarified the admission `Arc` outlives runner exit for later evidence inspection.

## Recommendation

Close PR #316 and PR #317 as stale/superseded after user approval. Start Phase 6 implementation only from a fresh branch off current `main`, using TDD one public behavior at a time.
