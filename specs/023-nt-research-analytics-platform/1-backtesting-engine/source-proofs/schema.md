# SourceProofReport — schema (BTE-015)

The thin contract every market-data source must satisfy **before** it can back a
backtest. A source is not selectable from prose, a venue example, or a vendor
homepage — only from an *accepted* `SourceProofReport`. One report per
(fixture, source-binding); the populated reports live alongside this file
([`binary-option.md`](./binary-option.md), [`perps-spot.md`](./perps-spot.md)).

This schema stores **proof and pointers, not payload** — no raw ticks, no
catalog, no backtest results live in a report.

## Ordering rule (official/free first)

Candidates are recorded in this order, and a paid/vendor candidate is only
admitted **after** the official/free gap that forces it is written down:

1. The venue's own official API / archive (free or included).
2. Free public archives / community mirrors.
3. Paid vendors / aggregators — admitted only with a recorded reason ("official
   source lacks historical L2", etc.).

## Fields

> The authoritative field set and the `DataFidelityClass` enum are owned by
> [`reference/data-model.md`](../../reference/data-model.md) (`SourceProofReport`,
> `DataFidelityClass`) and [`reference/contracts.md`](../../reference/contracts.md)
> (Source Proof Contract). This table is the BTE-local working view and must not
> diverge from them.

| Field | Meaning |
|-------|---------|
| `source_id` | Stable id for this (fixture, source) binding. |
| `fixture` | `binary option` \| `perps/spot` (the BTE-003 fixture this serves). |
| `venues_covered` | Venues/markets the source actually provides. |
| `candidate_tier` | `official_free` \| `free_public` \| `paid_vendor` (drives ordering above). |
| `data_classes` | What exists: L2 incremental book / L3 MBO / depth snapshots @cadence / trades / quotes / OHLCV. |
| `fidelity_class` | One of the classes below — the **highest** the source can actually support. |
| `l2_evidence` | Required when claiming `L2_REPLAY`: the specific historical product (incremental deltas, or sufficient-cadence snapshots) **plus one of** — a dated coverage URL, a decompressed sample, or a recorded operator attestation (who validated it, which product, when). A claim with none of these is rejected. |
| `forward_capture_status` | `not_needed` \| `required` \| `in_progress` — `required` when no usable history exists and L2 must be captured going forward. |
| `history_depth` | Start date + retention/freshness (update lag). |
| `time_semantics` | Event-time, availability-time, and capture-time (not just start/freshness). |
| `nt_data_class_mapping` | The NT class each stream maps to: `OrderBookDelta` / `OrderBookDepth10` / `TradeTick` / `QuoteTick` / `Bar`. |
| `cross_market_ref` | When the signal needs it (e.g. kimchi-premium): the reference/FX source + a no-future-leak note. |
| `license` | Commercial-use determination + the **verbatim** clause and its source URL. `not_confirmed` is a valid, blocking value. |
| `cost_model` / `cost_estimate` | Pricing model + a concrete number **with units** and a fetched URL, or `contact_sales` / `not_public`. Never an invented number. |
| `fidelity_class` ⇒ `forbidden_claims` | The claim limits this fidelity forces (see table). |
| `sample_pointer` | Pointer (URL/path) to a representative sample — not the sample itself. |
| `sample_hash` | Content/manifest hash of that sample (pairs with `sample_pointer`). |
| `artifact_root_uri` | The single TOML-owned S3 `artifact_root` URI under which this report's artifacts/samples live. |
| `required_checks` | Pass/fail for: schema, sample, license, time-coverage, fidelity, NT-mapping, forbidden-claims. **All must pass** for acceptance. |
| `source_proof_version` | Monotonic version. |
| `supersedes_source_proof_id` | Prior report this replaces, if any. |
| `selection_status` | `candidate` \| `shortlisted` \| `accepted` \| `rejected`. |
| `accepted_by` / `accepted_at` / `acceptance_mode` | Set **only** on accepted reports; accepted records are immutable. |

## Fidelity classes and forbidden claims (claim limits)

| `fidelity_class` | Data | NT mapping | Forbidden claims |
|------------------|------|-----------|------------------|
| `L2_REPLAY` | tick-level incremental order book | `OrderBookDelta` | — (execution-quality fills permitted *if* the venue's own deltas, not snapshot-reconstructed) |
| `DEPTH_SNAPSHOT_REPLAY` | periodic depth snapshots (e.g. 0.5s, 1-min) | `OrderBookDepth10` / snapshot-flagged deltas | no tick-accurate queue position; intra-snapshot fills are approximations |
| `TRADE_BAR_REPLAY` | trades and/or OHLCV bars only | `TradeTick` / `Bar` | no order-book or fill realism; signal/trade-through only |
| `SIGNAL_ONLY` | reference series (e.g. FX rate) | custom / `Bar` | not a tradable instrument; reference/feature input only |

A report claiming a fidelity class it cannot evidence is **rejected** by
`required_checks`. A "snapshot-derived" incremental feed (snapshots repackaged as
deltas) is `DEPTH_SNAPSHOT_REPLAY`, **not** `L2_REPLAY`.

## Acceptance authority

Acceptance is the Backtesting Engine / source-proof authority's call (BTE-019).
`normal` runs default to the latest accepted proof; non-latest pins are allowed
only for non-`normal` `run_purpose` with an allowed reason code. Accepted records
are immutable; a correction is a new version that `supersedes` the old.
