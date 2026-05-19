# Phase 7 External Review Disposition

**Branch**: `017-bolt-v3-phase7-no-submit-readiness-fresh`
**Initial plan head reviewed**: `28da07386d81469fa7cb3f1b0ecfef625a4e0e88`
**Revised plan head reviewed**: `9945334fef2571cef5653d4ac6130457b03da939`
**Base**: `d6f55774c32b71a242dcf78b8292a7f9e537afab`

## Required Review Results

| Reviewer | Job | Source | Verdict | Blocking Findings |
| --- | --- | --- | --- | --- |
| Claude | `09eae7d5-4c2a-4173-afaa-dc09909c54cd` | sent | APPROVE | None |
| DeepSeek | `job_8f63d873-78c3-46ad-bb10-688a8fdb9145` | sent | APPROVE | None |
| GLM | `job_95c36378-9bcc-4462-93e2-126619ddebf0` | sent | APPROVE | None |

## Accepted Non-blocking Findings

- Operator harness source fence should be explicit. Accepted: T007 now fences both readiness module and operator harness.
- Controlled connect/disconnect timeout should be explicit. Accepted: spec edge cases now include controlled-connect, reference-readiness, and disconnect timeout.
- External-review disposition file should define accepted fixes, disprovals, and deferrals. Accepted: T004 wording updated and this disposition file created.
- `approval_id_hash` algorithm should be pinned. Accepted: data model now requires full lowercase SHA-256 hex and forbids raw approval id in report output.
- `reference_readiness` stage needs clearer pass/fail contract. Accepted: contract now defines required reference readiness and fail-closed cases.
- Add explicit reference-readiness failure, report byte-cap, and double-failure cleanup test coverage. Accepted: T014 now covers these cases.
- Focused clippy should happen before late PR readiness. Accepted: T019a added after local Phase 7 tests.

## Revised Plan Review Results

| Reviewer | Job | Source | Verdict | Blocking Findings |
| --- | --- | --- | --- | --- |
| Claude | `ef74f70a-4738-48b5-a654-5181e9cae44a` | sent | APPROVE | None |
| DeepSeek | `job_edbe9919-21db-4dbe-9bfd-7bea74ef3a56` | sent | APPROVE | None |
| GLM | `job_83a6f76c-9ff5-410d-89cb-9e2360b47983` | sent | APPROVE | None |

## Revised Plan Accepted Non-blocking Findings

- Chainlink/freshness scope was over-broad for a no-run cache-presence check. Accepted: contract and quickstart now distinguish configured reference-instrument reachability from Phase 8 feed/source freshness.
- Cache population may be asynchronous after `LiveNode::start()`. Accepted: plan, research, and T046 now require bounded cache recheck using existing live-node timeout config with no new hardcoded poll interval.
- Strategy `on_start()` side effects must be explicit. Accepted: plan, research, and T048 now require a strategy `on_start`/submit-admission audit; source fences are not claimed as runtime-subscription fences.
- Stop must run even if reference-cache inspection fails or startup partially fails. Accepted: plan, research, T043, T044, and T045 now require stop recording across those paths.
- Real operator path can read existing account state through startup reconciliation. Accepted: quickstart now requires an empty or segregated approved account before any ignored real harness run.
- Source-fence coverage must include the new helper direction. Accepted: T048 keeps source fences in the implementation correction gate.

## Non-blocking Deferrals

- Full cargo test and full clippy remain PR-readiness gates because Phase 7 code is not written yet. They remain tracked by T037.
- Real SSM/venue no-submit readiness remains approval-gated and will not run during default implementation. It remains tracked by T025 and the quickstart approval command.

## Phase 8 Blocked State

Phase 8 live action remains blocked until a real no-submit report exists, live-canary gate accepts that report, the `binary_oracle_edge_taker` strategy-input safety audit approves the actual feed/venue/market/math/economics inputs, and the user explicitly approves the exact head SHA plus live command.

## Implementation Discovery

Current main exposes controlled bolt-v3 NT connect/disconnect without entering the runner loop, but connect success alone cannot satisfy `reference_readiness`. The revised Phase 7 plan uses bounded NT `LiveNode::start()` / required reference-instrument cache inspection / `LiveNode::stop()` without `LiveNode::run()`. This keeps lifecycle and cache ownership in NT and avoids a direct provider-read dual path.

## Decision

Implementation may begin after this disposition because Claude, DeepSeek, and GLM all approved the Phase 7 plan with no blocking findings, and all actionable non-blocking plan findings above are accepted into the plan before runtime code.
