# Research: PR #331 Phase 9 Current-Head Audit

## Method

Current source of truth for PR #331 claims is live PR metadata plus checked-out source, not old branch memory. Exact head is verified at review time with `gh pr view 331 --json headRefOid,baseRefOid,mergeStateStatus,state`. Committed P9 docs intentionally avoid hardcoding their own future commit SHA.

Commands used for the current P9 sync:

- `git status --short --branch`
- `gh pr view 331 --json number,title,state,isDraft,headRefName,headRefOid,baseRefName,baseRefOid,mergeStateStatus,updatedAt,url`
- `gh pr view 392 --json number,title,state,isDraft,headRefName,headRefOid,baseRefName,baseRefOid,mergeStateStatus,updatedAt,url`
- `gh pr view 331 --comments --json comments`
- `sed -n '84,108p' specs/001-thin-live-canary-path/tasks.md`
- `sed -n '70,120p' docs/bolt-v3/2026-05-18-production-readiness-contract.md`
- `rg -n "implemented|partial|missing|tiny live|production live|dry-run|shadow|deploy trust|panic" docs/bolt-v3/2026-04-28-source-grounded-status-map.md`
- stale-reference and debt-marker scans from `quickstart.md`

## Decisions

### Decision 1: P9 Cannot Certify Live Readiness

P7 and P8 source-review gates are closed for PR #331, but live evidence remains unrun. `specs/001-thin-live-canary-path/tasks.md` leaves T038 unchecked for the real SSM/venue no-submit run and T046 unchecked for the tiny-capital canary run.

### Decision 2: P9 Source Review Can Proceed After Artifact Sync

P9 is reviewing the audit artifacts and source-backed claim boundaries. It can close PR #331 source-review obligations only after current artifacts are clean, pushed, CI is green, and six external reviewers return no unresolved blockers. This does not close no-submit live readiness or tiny-canary readiness.

### Decision 3: No Active Operator Config Is Present

The checkout has tracked root/strategy example TOML files, but no active operator root TOML is present. Tracked examples are not approval evidence and are not a substitute for a configured, checksummed operator run.

### Decision 4: Self-Referential SHA Is Avoided

If committed docs hardcode the commit SHA they are part of, the act of updating them changes the SHA and makes the artifact stale. The correct control surface is: committed docs require runtime exact-head injection; PR comments record exact-head review evidence after push.

### Decision 5: PR #392 Remains Downstream

PR #392 is open and separate. It aligns bolt-v3 TOML vocabulary with NautilusTrader vocabulary and depends on PR #331 landing first. P9 may document that dependency, but must not implement PR #392 scope.

### Decision 6: Production Readiness Contract Blocks Broad Claims

`docs/bolt-v3/2026-05-18-production-readiness-contract.md` requires order lifecycle, restart reconciliation, single-runner, approval replay-resistance, monitoring/alerting, and deploy provenance proof before staged or production live claims.

### Decision 7: Status Map Still Contains Live-Readiness Gaps

The source-grounded status map marks current source coverage implemented for several architecture/verifier surfaces, but rows for activated-scope evidence, catalog round-trip, NT readiness, Chainlink anchor, order lifecycle, reconciliation, observability, dry-run, shadow, deploy trust, panic/service policy, CLOB V2 readiness, tiny live canary, production live trading, cost/fee facts, and broad discovery activation remain missing or partial.

## Alternatives Rejected

- Treat P7/P8 source reviews as live readiness: rejected because T038 and T046 are still unrun.
- Store final external review disposition only in a committed file: rejected because adding exact-head review results would create a new head and stale the review evidence. PR comments are the exact-head evidence surface.
- Continue PR #392 work inside PR #331: rejected because PR #392 has a separate declared scope and must remain downstream.
