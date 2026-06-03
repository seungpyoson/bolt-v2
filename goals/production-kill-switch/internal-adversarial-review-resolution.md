# Internal Adversarial Review Resolution

Review source: `goals/production-kill-switch/internal-adversarial-review.md`

Resolution status: addressed in design artifacts; not an external-model approval.

## Resolution Map

1. HIGH: flatten orders can be blocked by ordinary submit-admission caps.
   - Added a distinct proof-bound `KillSwitchForcedReduction` admission/action class.
   - Required forced reductions to bind halt id, trigger evidence hash, position proof, route proof, and config policy hash.
   - Required tests for ordinary cap exhaustion and over-normal-cap forced reductions.
   - Updated `design.md`, `issue-draft.md`, `facts.md`, `research.md`, `plan.md`, and `review-packet.md`.

2. HIGH: cancel/reconciliation scope omitted inflight, pending-cancel, emulated, and algorithm orders.
   - Replaced "open orders" proof with "outstanding order risk".
   - Required coverage for open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal order risk.
   - Added cancel and reconciliation tests for each category.
   - Updated `design.md`, `issue-draft.md`, `facts.md`, `research.md`, `plan.md`, and `review-packet.md`.

3. MEDIUM: state-machine table omitted durable-write failure and reset authorization.
   - Added `Halting -> FailedManualIntervention` on durable write/append/rename/fsync failure.
   - Added `state_store_healthy`, `state_write_succeeded`, `operator_authorized`, and `manual_reset_evidence_valid` to the exhaustive transition dimensions.
   - Added durable-store failure and reset-authorization tests.
   - Updated `design.md`, `issue-draft.md`, and `plan.md`.

4. MEDIUM: reconciliation had a cache-only escape hatch.
   - Split proof streams into mandatory and optional config-owned streams.
   - Required fail-closed behavior for missing/stale/contradictory mandatory streams.
   - Required stronger cache/portfolio query evidence when an optional stream is absent.
   - Updated `design.md`, `issue-draft.md`, `facts.md`, `plan.md`, and `review-packet.md`.

5. MEDIUM: operator reset lacked authorization and tamper-evidence requirements.
   - Required authorization source, operator identity binding, and append-only or hash-chained reset evidence.
   - Added negative tests for unauthorized, stale, or tampered reset evidence.
   - Updated `design.md`, `issue-draft.md`, `facts.md`, `plan.md`, and `review-packet.md`.

## Packet Update

The selected external-review packet changed after this resolution pass. `goals/production-kill-switch/packet-manifest.md` now records the updated 15-file packet hashes and the latest source-not-sent DeepSeek/GLM preflight totals: 299,176 bytes and 6,122 lines.
