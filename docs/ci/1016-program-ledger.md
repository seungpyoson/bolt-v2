# Issue #1016 Program Ledger

## Authority and use

This is the current-only operational ledger at authoritative main `9f3b13f4c6ae937be69cfb9c44fae409d268ef30`. Historical ledger copies, packets, branch heads, and receipts are commit-qualified evidence only. They do not become current without revalidation from fresh main.

This ledger records documentation state; it does not authorize implementation, GitHub control-plane changes, or deletion of an existing control.

## Program A — accepted cleanup wave

| Issue | Final head | Merge/main | Repository delta | Python outcome | State |
| --- | --- | --- | ---: | ---: | --- |
| #1364 | `91c4bd8e…` | `3c40bb10…` | +0/-54 | 50 lines deleted | `MERGED` |
| #1365 | `22bff389…` | `f25cd42c…` | +1/-6 | Net Python reduction | `MERGED` |
| #1368 | `b66869b8…` | `5a87c509…` | +3/-7 | Net Python reduction | `MERGED` |
| #1369 | `b901592d…` | `9f3b13f4…` | +0/-59 | 59 lines deleted | `MERGED` |

Combined: 9 files, +4/-126 repository lines, and 118 net Python lines removed. These Program-A issue-owned deletions already landed before any precursor. They define the current planning boundary but are distinct from the legacy #1016 deletion: the precursor will fix that exact deletion manifest, and the legacy central-verifier bytes are deleted only by atomic activation. Stale candidate rows are not revived; any historical packet requires revalidation from fresh `9f3b13f4…` or its future authoritative successor.

## Program B — #1016 central verifier

| Order | Node | State | Resolution condition |
| ---: | --- | --- | --- |
| 1 | Issue-body and atomic-ruling reconciliation | `BLOCKED` | Reviewed issue authority distinguishes already-landed Program-A deletions, precursor-fixed legacy deletion manifest, freeze, and actual legacy deletion at atomic activation |
| 2 | Corrected dormant-base and two-context trust design | `BLOCKED` | Owner review and external adversarial review accept or further revise the correction |
| 3 | External App/control-plane authorization and budget | `BLOCKED` | Separate approved design covers principals/quorum, digest-bound approval, attestation, rollback-resistant state, key lifecycle, audit retention, installation ownership, and budget |
| 4 | Exact-SHA regeneration | `BLOCKED` | Reviewed outputs exist for rule dispositions, callers, corpus, timing, peak RSS, cost, and amplification, each bound to command, SHA, and digest |
| 5 | Complete dormant precursor | `BLOCKED` | Exact replacement bytes and pending-activation manifest land on protected base under the separately authorized bootstrap exception |
| 6 | Protected-base canary and freeze | `BLOCKED` | Exact reviewed bytes are verified at protected `policy_base_sha` and promoted; the external monotonic tombstone is then written and later bootstrap issuance and acceptance are irreversibly disabled; a closed canary passes against that post-tombstone, post-disable state; only then is freeze evidence complete. Canary failure remains fail-closed and blocked, cannot reopen or reissue the bootstrap exception, and requires a separately governed control-plane recovery path |
| 7 | Atomic #1016 activation and legacy deletion | `BLOCKED` | An isolated exact-manifest head is judged by base-owned staged semantics and contains no extra semantic or activation changes |

The future deterministic 113-file planning inventory and machine-readable subsystem DAG are required deliverables, but their digests do not yet exist and are not asserted here.

## Later program nodes

Shared GitHub transport, provenance, merge governance, Rust verification, clean-merged artifacts, storage, and AI review are `UNASSIGNED`. They may be represented in a future reviewed DAG, but none has implementation authority until it has a named issue and reviewed exact file, cutover, deletion, dependency, and evidence set.

The shared GitHub transport node, when authorized, is one transport-only all-caller migration slice. It may not absorb domain verdicts or become a broad domain rewrite.

## Current decision boundary

The atomic sequence is the only accepted implementation-order decision recorded here. The dormant implementation, two external-App contexts, Mergify/App-qualified authority binding, and control-plane model remain proposed corrections pending owner plus external review. Production implementation is not ready.
