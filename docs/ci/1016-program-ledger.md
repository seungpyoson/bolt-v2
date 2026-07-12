# Issue #1016 Program Ledger

## Authority and use

This is the current-only operational ledger at authoritative main `9f3b13f4c6ae937be69cfb9c44fae409d268ef30`. Historical ledger copies, packets, branch heads, and receipts are commit-qualified evidence only. They do not become current without revalidation from fresh main.

This ledger records documentation state; it does not authorize implementation, GitHub control-plane changes, or deletion of an existing control.

## Program A — accepted cleanup wave

| PR | Owning issue | Final head | Merge/main | Repository delta | Python outcome | State |
| --- | --- | --- | --- | ---: | ---: | --- |
| #1364 | #1318 | `91c4bd8e…` | `3c40bb10…` | +0/-54 | 50 lines deleted | `MERGED` |
| #1365 | #1215 | `22bff389…` | `f25cd42c…` | +1/-6 | Net Python reduction | `MERGED` |
| #1368 | #1215 | `b66869b8…` | `5a87c509…` | +3/-7 | Net Python reduction | `MERGED` |
| #1369 | #1215 | `b901592d…` | `9f3b13f4…` | +0/-59 | 59 lines deleted | `MERGED` |

Combined: 9 files, +4/-126 repository lines, and 118 net Python lines removed. These Program-A issue-owned deletions already landed before any precursor. They define the current planning boundary but are distinct from the legacy #1016 deletion: the precursor will fix that exact deletion manifest, and the legacy central-verifier bytes are deleted only by atomic activation. Stale candidate rows are not revived; any historical packet requires revalidation from fresh `9f3b13f4…` or its future authoritative successor.

## Program B — #1016 central verifier

| Order | Node | State | Resolution condition |
| ---: | --- | --- | --- |
| 1 | Issue-body and atomic-ruling reconciliation | `BLOCKED` | Reviewed issue authority distinguishes already-landed Program-A deletions, precursor-fixed legacy deletion manifest, freeze, and actual legacy deletion at atomic activation |
| 2 | Corrected dormant-base and two-context trust design | `BLOCKED` | Owner review and external adversarial review accept or further revise the correction |
| 3 | External App/control-plane and temporary-lock authorization | `BLOCKED` | Separate approved design covers principals/quorum, digest-bound approval, attestation, rollback-resistant state, key lifecycle, audit retention, installation ownership, Mergify Freeze/Merge Protections operations, retry/terminal classification, terminal-failure policy, and budget; disposable live proof and explicit owner acceptance of the rehearsal-derived successful-ceremony target, retry budget, pre-precursor abort threshold, and post-precursor unbounded terminal-tail ordinary-merge outage risk are mandatory before reliance |
| 4 | Exact-SHA regeneration | `BLOCKED` | Reviewed outputs exist for rule dispositions, callers, corpus, timing, peak RSS, cost, and amplification, each bound to command, SHA, and digest |
| 5 | Prepared drafts and exact-number admission-lock PR | `BLOCKED` | Before lock merge: open/reserve precursor draft and make it review-ready/green; reserve activation draft and prepare exact allowed scope as far as possible; complete control-plane/live canaries; review abort/restore; obtain owner approval of the rehearsal-derived successful-ceremony target, retry budget, and pre-precursor abort threshold plus separate acceptance of post-precursor no recovery and an unbounded terminal-tail ordinary-merge outage. Then land one reviewed temporary `.mergify.yml` lock under legacy authority with exactly one precursor-only queue, batch size one, one parallel check, injection disabled, four explicit legacy checks, native review, and no alternate route. The ordinary/unrelated-merge outage starts at lock merge; only precursor and activation ceremony PRs may merge until successful cleanup |
| 6 | Temporary-control proof and final pre-precursor state | `BLOCKED` | Prove this repository/API's `exempt` support, Mergify injection behavior, exact-number admission, no hidden routes, self-change reset, proof invalidation, mixed batches, merge-time Freeze re-evaluation, exclusions, dequeue/no running batch, wrong publisher, native/direct blocking, Freeze under exempt, identity, bounded retry classification/exhaustion, successful-ceremony timing and a pre-precursor abort threshold, latency, and API/quorum/audit. Enable temporary Merge Protections/Freeze; set final gate-only-replacement ruleset and Mergify 10562 `exempt`; add reserved activation as inert second exclusion; terminally re-query all state |
| 7 | Complete dormant precursor | `BLOCKED` | One precursor contains exact dormant replacement bytes and manifest and atomically replaces the admission lock with final hotfix/default mappings. The temporary lock, four legacy checks, and native review judge it; trusted emits nothing; all other paths remain frozen |
| 8 | Promotion, tombstone, and closed canary | `BLOCKED` | After precursor, no enforcement mutation occurs. Promote exact bytes, tombstone bootstrap, irreversibly disable issuance/acceptance, and run an internal, non-publishing clean/falsifying canary against final state. Enumerated infrastructure/observation noise may retry with fresh nonce only under wholly unchanged immutable state and within the pre-approved budget. Failed, intermediate, infrastructure-noise, canary, reused, converted, or partial results cannot publish merge-visible authority. Merits/identity failure or exhaustion is terminal, leaves Freeze active with no recovery PR, and may make the ordinary-merge outage unbounded |
| 9 | Atomic #1016 activation and legacy deletion | `BLOCKED` | After successful canary, queue reserved activation alone as the literal first subsequent covered change. A fresh retry uses a fresh nonce and the same immutable activation/base/configuration tuple; emit a new trusted result only on its exact regenerated proof head after independently completing the entire terminal validation, with no prior-result reuse or conversion. Atomically delete legacy covered semantics and prove expected protected main |
| 10 | Remove temporary controls and enter steady state | `BLOCKED` | After exact main and terminal proof, delete Freeze and disable/remove temporary Merge Protections reporting or binding; the successful-path ordinary/unrelated-merge outage ends only then. Retain final App-qualified trusted ruleset authority and Mergify `exempt` unless separately redesigned. Trusted is emitted for each exact Mergify proof head judged by active protected-base semantics, remains absent on ordinary PR heads, and Vn judges Vn+1 staging |

The future deterministic 113-file planning inventory and machine-readable subsystem DAG are required deliverables, but their digests do not yet exist and are not asserted here.

## Later program nodes

Shared GitHub transport, provenance, merge governance, Rust verification, clean-merged artifacts, storage, and AI review are `UNASSIGNED`. They may be represented in a future reviewed DAG, but none has implementation authority until it has a named issue and reviewed exact file, cutover, deletion, dependency, and evidence set.

The shared GitHub transport node, when authorized, is one transport-only all-caller migration slice. It may not absorb domain verdicts or become a broad domain rewrite.

## Current decision boundary

Only Program-A-before-precursor and atomic legacy deletion ordering are accepted. The temporary admission lock, outage/retry budgets, temporary Mergify Merge Protections/Freeze ceremony, pre-precursor final ruleset and `exempt` state, promotion/disablement/canary/activation hinge, no-recovery risk acceptance, dormant implementation, two external-App contexts, steady-state protected-base authority, Mergify/App-qualified authority binding, and control-plane model remain proposed corrections pending separate approval, disposable live proof, owner review, and external review. GitHub Cloud documents `exempt` as a repository-rules bypass mode, but this repository/API version and Mergify injection behavior remain live-proof blockers; no mutation is claimed. Production implementation is blocked and not ready.
