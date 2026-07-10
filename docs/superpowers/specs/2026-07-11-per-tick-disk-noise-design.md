# Per-Tick Disk-Write Noise Design

## Goal

Eliminate redundant disk appends reachable from quote, book, timer, and index-price handlers without changing any evidence schema or suppressing action-, recovery-, reconciliation-, reservation-, or settlement-keyed records. This work is related to #1275 and #1179 and is required before the 24-hour soak kill/restart acceptance leg.

## Root Cause

`BinaryOracleEdgeTaker::try_submit_entry_order` has two structurally distinct `strategy_input_snapshot` append paths:

- `blocked_entry_strategy_input_evidence_snapshot_at` builds a diagnostic snapshot with an empty `client_order_id` whenever entry pricing is blocked on realized-volatility readiness. `on_book_deltas` calls the entry evaluation at market-data cadence, and this path appends and syncs a multi-kilobyte record on every evaluation.
- `entry_strategy_input_evidence_snapshot_at` builds a submit-linked snapshot with a non-empty `client_order_id` at the entry-submit chokepoint. Recovery and shadow-PnL correlate this record with `order_intent` and `admission_decision` by `client_order_id`.

The blocked form has no machine consumer: `read_latest_entry_decision_evidence_chain` and `shadow_pnl::read_admitted_entry_chains` can only use submit-linked snapshots. It remains useful as the full durable realized-volatility diagnostic trail, so this slice retains it as forensic-only state observation, flags it as a drop candidate for an owner decision, and reduces it to state transitions.

## Chosen Design

Add a strategy-local `BlockedStrategyInputDedupeKey` and a single last-key field to `BinaryOracleEdgeTaker`. A dedicated blocked-path recording helper will build the blocked snapshot, derive its semantic state key, and append only when the key changes. The submit-linked call site remains direct and unchanged, so writer-level policy cannot suppress recovery-critical evidence.

The key contains:

- market and selection identity/outcome;
- entry gate and pricing blocker categories;
- fast-venue/reference availability and incoherence state;
- realized-volatility surface and blocker categories;
- per-source identity, enabled/quorum participation, status, block reason, and last rejection category; and
- unknown-source rejection categories.

The key excludes prices, timestamps, ages, sample counts, rejection counts, coverage values, volatility values, and every other field that can vary on an otherwise identical tick. When the evaluation leaves the blocked-RV path, the stored key is cleared; returning to the same blocked state therefore emits a new transition. The key is in memory only and resets on restart, so one duplicate per key per process lifetime is accepted, matching #1351's terminal-evidence ruling.

## Append-Path Census

The enumeration unit is an append path. `strategy_input_snapshot` therefore has two rows. “No semantic reader” means generic readers may validate and skip the envelope, but no machine consumer uses the payload.

| Append path / record kind | Tick-handler reachability | Named reader or action consumer | Bucket | Disposition and structural evidence |
| --- | --- | --- | --- | --- |
| `strategy_input_snapshot` — blocked RV | Book; no quote/timer/index-price append | No machine reader; retained for full RV forensic diagnostics | 2 STATE-OBSERVATION | State-key at the distinct `blocked_entry_strategy_input_evidence_snapshot_at` call site; forensic-only drop candidate pending owner decision |
| `strategy_input_snapshot` — submit linked | Book entry submit; no quote/timer/index-price append | `read_latest_entry_decision_evidence_chain`; `shadow_pnl::read_admitted_entry_chains` | 1 EVENT-KEYED | Untouched direct append from `entry_strategy_input_evidence_snapshot_at`; non-empty `client_order_id` structurally distinguishes it |
| `order_intent` | Quote/book/timer exit submit; book entry submit; no index-price append | Recovery/shadow-PnL chain and real submit action | 1 EVENT-KEYED | Completeness untouched |
| `admission_decision` | Quote/book/timer exit admission; book entry admission; no index-price append | Recovery/shadow-PnL chain and real admission action | 1 EVENT-KEYED | Completeness untouched |
| `basket_admission_decision` | None of the four current strategy handlers; basket execution boundary only | Real basket admission action; no semantic file reader | 1 EVENT-KEYED | Completeness untouched; reader status disclosed |
| `capital_admission_rebuild` | Startup only | Startup rebuild lifecycle audit; generic readers validate it | 1 EVENT-KEYED | Completeness untouched |
| `submit_reservation_metadata` | Quote/book/timer submit paths; no index-price append | `read_submit_reservation_recovery_evidence` | 1 EVENT-KEYED | Reservation completeness untouched |
| `submit_reservation_fill` | Order-fill lifecycle event, not a four-source tick | `read_submit_reservation_recovery_evidence` | 1 EVENT-KEYED | Reservation completeness untouched |
| `entry_skip` | Book entry evaluation; no quote/timer/index-price append | No semantic file reader | 3 ALREADY-DEDUPED | `EntrySkipDedupeKey` contains blocker/market/liveness state only; no time or price |
| `exit_decision` | Quote/book/timer exit evaluation; no index-price append | No semantic file reader | 3 ALREADY-DEDUPED | `ExitDecisionDedupeKey` contains market/position/reason/outcome state only; no time or price |
| `exit_evaluation` | Quote/book/timer exit evaluation; no index-price append | `read_exit_evaluation_evidence` | 3 ALREADY-DEDUPED | `ExitOutcomeKey` contains decision/block/RV-gate outcome only; no time or price; actual submit remains unconditional |
| `loss_governor_halt` | Admission reached from quote/book/timer submit paths; no index-price append | `read_loss_governor_halt_evidence`; real halt episode | 1 EVENT-KEYED | Power-of-two episode samples carry changing retry count/elapsed information; completeness policy unchanged |
| `order_reject` | Admission/reject events reached from quote/book/timer and order lifecycle; no index-price append | `read_order_reject_evidence`; real rejection episode | 1 EVENT-KEYED | Power-of-two episode samples and lifecycle rejects carry real action information; unchanged |
| `order_lifecycle` | Timer reconciliation and order lifecycle callbacks; settlement terminal paths may follow timer/index-price state | Real lifecycle/reconciliation transitions; no semantic file reader | 1 EVENT-KEYED | Completeness untouched; reader status disclosed |
| `requote_throttle` | No append from the maker's current trade/timer handlers; reachable from explicit maker runtime quote routing | No semantic file reader | 3 ALREADY-DEDUPED | `RequoteThrottleDedupeKey` contains family/leg/action/reason/bound only; no time or price |
| `settlement` | Index-price resolution; startup/timer recovery replay | Settlement recovery readers (`read_settlement_*`) | 1 EVENT-KEYED | Settlement-key completeness untouched |
| `settlement_booking_error` | Index-price settlement and quote/book/timer exit/settlement checks | Settlement booking-error recovery readers | 1 EVENT-KEYED | Booking-error completeness untouched |
| `venue_truth_capture_failure` | Live-node periodic venue-truth poll, not a strategy handler | No semantic file reader; admission state is mutated directly rather than recovered from this record | 4 NO NAMED READER | Keep unchanged and flag for owner decision; do not silently drop |
| `venue_truth_divergence` | Live-node periodic venue-truth poll, not a strategy handler | Real durable halt action; kill-switch store is recovery authority; no semantic evidence-file reader | 1 EVENT-KEYED | Completeness untouched; reader status disclosed |

### Reproducible negative sweep

The non-evidence appender claim is established by searching all strategy and live-node Rust modules for direct append/file-write APIs:

```bash
rg -n 'write_all|sync_data|OpenOptions|append_jsonl|File::create|BufWriter|csv::Writer|serde_json::to_writer' \
  src/strategies src/bolt_v3_live_node.rs src/bolt_v3_live_node --glob '*.rs'
```

The result is empty. Handler reachability is independently enumerated with:

```bash
rg -n 'impl DataActor|fn on_quote|fn on_book_deltas|fn on_time_event|fn on_index_price' \
  src/strategies src/bolt_v3_live_node.rs src/bolt_v3_live_node --glob '*.rs'
```

Call sites for every evidence kind are enumerated from the complete `BoltV3DecisionEvidenceWriter` trait method list and cross-checked against all `record_*` calls under `src/`.

## Testing

- Add a blocked-snapshot differential that fails on current code: N evaluations at different tick times with identical semantic state produce one `StrategyInput` event; changing the RV blocker/status produces the second.
- Preserve a submit-linked differential proving separate client-order-linked snapshots are never suppressed.
- Audit existing bucket-3 keys structurally and with differentials: entry-skip, exit-decision, exit-outcome, and requote-throttle. Each key must contain no per-tick timestamp or price.
- Run the full unfiltered exact-head suite remotely, with no scope filters, plus `just fmt-check`, the runtime literal self-test/audit if strings change, and the repository's static source-fence gates.

## Scope Boundaries

- No evidence schema changes.
- No changes to capture, rotation, journald, uploader, or recovery byte limits.
- No writer-level dedupe.
- No suppression or sampling changes to event-keyed records.
- No local compile-heavy Rust verification; exact-head Rust proof follows the repository's remote-first workflow.
