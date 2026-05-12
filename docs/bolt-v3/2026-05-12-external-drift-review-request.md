# Bolt-v3 External Drift Review Request

Date: 2026-05-12
Branch: `codex/bolt-v3-slice-3-10-completion-audit`
Head: `98d56ec13db9d48b5962df101ecc9f72c4115c6c`

## Review Goal

Decide whether current bolt-v3 work is still the shortest safe path to a tiny live trade through NautilusTrader, or whether it has drifted into verifier/doc/test bloat.

This is not a request to approve production trading. This is a drift review.

## Hard Constraints

- No hardcoded runtime values in core.
- No dual production paths.
- No Python production runtime.
- SSM is only secret source.
- NautilusTrader owns trading runtime mechanics.
- Bolt-v3 owns config, activation, strategy construction, evidence, risk policy, and gates.
- Do not treat local tests as live readiness.
- Do not revive PR #300 as evidence.

## Current Evidence To Review

- Current audit: `docs/bolt-v3/2026-05-12-slice-3-10-completion-audit-refresh.md`
- Tracker: `docs/bolt-v3/2026-05-10-bolt-v3-follow-up-tracker.md`
- Production ledger: `docs/bolt-v3/2026-05-10-production-readiness-evidence-ledger.md`
- Review summary: `docs/bolt-v3/2026-05-10-production-readiness-review-summary.md`

## Known Current Status

- F3-F8 mostly have local proof.
- F9 has local mock lifecycle plus local real-adapter HTTP submit/cancel proof.
- F10 has local mock restart/reconciliation proof.
- F12 scale is only partial.
- F13 hardcode work is partial and risks diminishing returns.
- No approved authenticated venue canary has run.
- No real fill/user-channel proof exists.
- No real adapter reconciliation proof exists.

## Questions For Reviewer

1. Is current branch still NT-thin, or is bolt-v3 rebuilding too much around NT?
2. Which current artifacts are necessary for immediate tiny-live-trade verification?
3. Which artifacts are bloat or should stop now?
4. Is F13 hardcode verifier expansion still valuable, or should it freeze?
5. What is the shortest safe next slice to reach a real capped live trade?
6. What must block live submit no matter what?
7. Does any current design create hidden hardcodes, policy hardcodes, or dual paths?
8. Does any current naming or abstraction drift away from NT conventions?

## Expected Review Output

Use this format:

```text
Verdict: ACCEPT / ACCEPT WITH CHANGES / REJECT

Findings

F1 - Severity P0/P1/P2/P3
File/line:
Problem:
Why it matters:
Concrete fix:

Recommended next slice:

Stop doing:

Live-submit blockers:
```

## Reviewer Bias Requested

Be hostile to bloat. Assume local verifier work is suspect unless it clearly reduces live-trading risk or prevents hardcode relapse.

Be hostile to fake readiness. NT support, local mocks, and source fences are not live proof.

Be specific. Every claim should point to code, tests, docs, NT source, or explicit missing evidence.
