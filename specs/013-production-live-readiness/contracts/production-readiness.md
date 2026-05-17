# Contract: Production Live Readiness

## Claim Contract

1. Every readiness claim must name one level: tiny-canary ready, staged-live ready, or production-live ready.
2. Tiny-canary readiness permits only one explicitly approved capped canary attempt.
3. Staged-live readiness permits only operator-supervised repeated runs inside configured stage bounds.
4. Production-live readiness permits production-grade claims only for the named venue, market family, strategy, host, binary, root TOML, and approval scope.
5. If evidence supports a narrower level, the narrower level is the only allowed claim.

## Promotion Contract

Promotion from tiny-canary to staged-live requires:

- completed canary evidence for NT submit and venue order state
- strategy cancel evidence when order remains open
- restart reconciliation evidence
- post-run hygiene evidence
- order-lifecycle tests/tooling
- restart-reconciliation tests/tooling
- single-runner protection tests/tooling
- approval replay-resistance tests/tooling
- monitoring/alerting proof
- deploy provenance proof for each run

Promotion from staged-live to production-live requires:

- completed staged-run acceptance evidence
- no open status-map blocker in rows 34-48 unless explicitly waived
- exercised operator runbooks for repeated-live operation, abort, restart recovery, and post-run hygiene
- alert routing proof
- deploy provenance tied to reviewed commit, built binary, host, root TOML, SSM manifest, approval artifact, NT pin, and CI run
- explicit operator approval naming production scope

## Invalid Evidence

Evidence is invalid if it prints raw secrets, private keys, raw approval ids, account balances, or unredacted credential material.

## Non-Goals

- No live submit.
- No production deployment.
- No alternate secret source.
- No Bolt-owned order lifecycle or reconciliation implementation in this slice.
