# Internal Adversarial Review: Production Kill Switch Design

Verdict: `REQUEST_CHANGES`

Reviewed at repo HEAD `6ba549dcf15823484b5546f4eb314371479d7cc0`, against the current `goals/production-kill-switch` package and selected source evidence.

## Findings

### HIGH: Flatten orders can still be blocked by ordinary submit-admission caps

The design says the halt latch blocks entries/replaces while allowing explicit risk-reducing exits and kill-switch action commands (`goals/production-kill-switch/design.md:31-35`) and says flatten submits go through a proven NT path (`goals/production-kill-switch/design.md:147-150`). The current submit-admission path still applies max-notional and order-count caps to all admitted submits: notional is checked before the loss-governor exemption (`src/bolt_v3_submit_admission.rs:170-180`), and the live-order count cap is checked after it (`src/bolt_v3_submit_admission.rs:194-199`). The existing loss-governor spec also explicitly leaves risk-reducing exits under existing count/lifecycle caps (`specs/505-nt-loss-governor/spec.md:45`, `specs/505-nt-loss-governor/spec.md:65`, `specs/505-nt-loss-governor/spec.md:79`).

Risk: a kill switch triggered because the account is already at risk can deadlock before flattening if the forced exit exceeds the normal per-order cap or if the ordinary live-order count cap is exhausted.

Required change: the issue/design must require a distinct kill-switch forced-reduction admission class, or an explicit config-owned exemption from ordinary notional/order-count caps for verified flatten orders. Tests must cover cap-exhausted and over-normal-cap flatten attempts.

### HIGH: Cancel/reconciliation scope omits inflight, pending-cancel, emulated, and algorithm orders

The cancel design enumerates "open orders" only (`goals/production-kill-switch/design.md:130`), and flat proof requires no open orders (`goals/production-kill-switch/design.md:160`). The research already notes NT exposes pending-cancel/inflight helpers (`goals/production-kill-switch/research.md:68`). NT's own strategy cancel-all path treats open, emulated, inflight, and exec-algorithm orders as separate risk surfaces before routing cancellation (`nautilus_trader/crates/trading/src/strategy/mod.rs:757-806`, `nautilus_trader/crates/trading/src/strategy/mod.rs:839-872`).

Risk: the design can cancel visible open orders and then claim flat while an accepted-but-inflight, pending-cancel, emulated, or algorithm-managed order can still fill after the halt.

Required change: the issue/design must require cancel and reconciliation over open, inflight, pending-cancel, emulated, and algorithm/contingent order surfaces, with explicit NT cache/helper names and race tests for each category.

### MEDIUM: State-machine table does not cover durable-write failure or reset authorization

`Halting` is defined as "durable evidence write in progress" (`goals/production-kill-switch/design.md:65-67`), but the transition list only defines `Halting -> Halted` on persisted evidence (`goals/production-kill-switch/design.md:75-76`). The required exhaustive table omits `state_write_succeeded`, `state_write_failed`, `manual_reset_evidence_valid`, and operator authorization dimensions (`goals/production-kill-switch/design.md:86-89`).

Risk: implementation can satisfy the stated transition table without proving runtime behavior for failed halt persistence or invalid/unauthorized reset evidence.

Required change: add explicit fail-closed transitions for durable-store write/read failures, and add reset authorization/evidence dimensions to the exhaustive state-machine tests.

### MEDIUM: Reconciliation has a cache-only escape hatch that conflicts with fail-closed proof

Flat proof allows captured event evidence to be unavailable if the proof records why it is unavailable (`goals/production-kill-switch/design.md:160-164`), while the next line says missing proof keeps the system halted or moves it to manual intervention (`goals/production-kill-switch/design.md:165`). The research says captured events are evidence and missing proof keeps the system halted (`goals/production-kill-switch/research.md:94-95`).

Risk: implementers can treat unavailable event streams as documented-but-acceptable and claim flat from cache alone, even when event loss is the thing that should keep the system fail-closed.

Required change: define which proof streams are mandatory by config. If an event stream is mandatory, absence must fail closed; if optional, the design must specify the stronger cache freshness and query evidence that substitutes for it.

### MEDIUM: Operator reset path lacks an authorization/tamper-evidence requirement

The design names manual kill/reset commands and a redacted event-log viewer (`goals/production-kill-switch/design.md:55-59`) and configures manual trigger/reset paths plus redaction policy (`goals/production-kill-switch/design.md:186-187`), but it does not require authorization source, operator identity validation, append-only/tamper-evident reset evidence, or tests that unauthorized reset cannot return to `Armed`.

Risk: manual reset is the one path that re-enables new risk after a halt. Treating "evidence path/hash" as sufficient leaves the most dangerous operator action under-specified.

Required change: require operator authorization, identity binding, append-only or hash-chained reset evidence, and negative tests for unauthorized or stale reset evidence.

## Recommendation

Do not treat the current design as internally approved for issue creation. The next revision should tighten the issue draft and design around forced-flatten admission, full outstanding-order reconciliation, durable-store failure transitions, proof-stream mandatory/optional semantics, and operator reset authorization.
