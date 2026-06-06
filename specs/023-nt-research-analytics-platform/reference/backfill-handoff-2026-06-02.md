# Backfill Handoff - 2026-06-02

This is the handoff entry point for the one-off backfill state. It separates
uploaded/manifest-backed evidence from venue-complete coverage and records what
is actually wired today.

PR context: PR #582 preserves this handoff as the reference-only replacement for
PR #578. It does not add or bless the one-off scripts as maintained tooling.

## Naming

- Report PMXT as `Polymarket (PMXT source)` in user-facing tables.
- Do not report PMXT as a separate venue.
- Preserve `PMXT`, `pmxt`, `r2v2.pmxt.dev`, and source binding names in raw
  provenance fields, manifests, source proofs, and S3 lineage. Those fields
  identify the upstream source, not the venue name.

## Acceptance Terms

- `Accepted` means a completed manifest ties uploaded payloads to source,
  scope, hashes, bytes, and errors.
- `Accepted` does not mean full venue coverage.
- `Accepted` does not mean canonical normalized tables exist.
- `Accepted` does not mean the data is ready as NautilusTrader backtesting
  input.
- Raw S3 objects without completed manifests are physically present but not
  accepted for coverage accounting.
- Zero-payload manifests prove source-resolution outcome only; they do not add
  saved data.

## Current Wiring State

The current data is wired as S3 staging plus manifests. It is not fully wired as
the canonical backfill table contract and it is not a complete NautilusTrader
backtesting catalog.

| Layer | Wired now? | Handoff rule |
|---|---|---|
| Raw S3 staging | Yes, for completed venue/source runs | Treat as staged evidence only. |
| Completed ingest manifests | Yes, but schemas differ by venue/script | Use manifests for counts, bytes, hashes, errors, and gaps. |
| Source provenance | Partial | PMXT/Polymarket and archive flows carry strong source binding; some venue scripts encode provenance in custom manifests. |
| Machine-enforced source-proof acceptance | Partial/manual | Current acceptance is documented from manifests and status files, not enforced by one acceptance service. |
| Coverage ledger | Human-readable only | This file and the detailed status log are the ledger; there is no single machine-enforced coverage database yet. |
| Canonical normalized tables | No | Do not assume `trades`, `funding_rates`, `order_book_snapshots_fixed_depth`, options tables, or prediction-market metadata tables are populated uniformly. |
| NautilusTrader catalog/backtesting input | No | Do not run research/backtests as if this is a complete NT catalog until normalized writes and source-proof gates are implemented. |

## Per-Venue Wiring State

| Venue | Current wiring | Do not claim |
|---|---|---|
| Polymarket (PMXT source) | Best-aligned staged evidence: source binding is `polymarket-parquet-archive-index`, PMXT parquet objects are staged to S3, and manifests track source objects, bytes, errors, and offsets. | Do not claim full 93-day hourly coverage; do not report PMXT as a separate venue. |
| Binance | Raw public archive staging with final manifest for current seven-token scope. | Do not claim canonical normalized tables or NT-ready catalog. |
| OKX | Raw daily historical-download payloads plus source-proof uploads and per-run manifests. | Do not claim April 5 onward coverage or canonical table writes. |
| Hyperliquid core | Raw archive/API staging with manifest, completed objects, and explicit source gaps. | Do not call it complete; manifest records 799 gaps. |
| Hyperliquid HIP-3 | Targeted staged metadata/funding-style tranche. | Do not claim broad HIP-3 historical replay coverage. |
| Hyperliquid HIP-4 | Targeted staged prediction-market/outcome tranche. | Do not claim one-year outcome L2/trade replay or seven-token coverage. |
| Bybit | Archive tick-trade staging with accepted manifests. | Do not claim REST, delivery, historical volatility, or complete venue coverage. |
| Deribit | Partial raw staging with manifest and many errors. | Do not claim complete Deribit coverage or complete requested base coverage. |

## Required Next Wiring Work

Before using this data as production-grade backtesting input, complete these
steps:

1. Build a machine-readable coverage ledger from manifests with expected units,
   completed units, failed units, gap reasons, and source-proof ids.
2. Recover or rerun physically uploaded objects that are not covered by
   completed manifests, starting with Polymarket (PMXT source).
3. Normalize staged raw data into the table families in
   `backfill-table-contract.md`.
4. Enforce source-proof acceptance before canonical S3 writes.
5. Generate instrument-universe snapshots for the requested window, including
   instruments active at any point in the window.
6. Build or export the NautilusTrader-compatible catalog only after normalized
   rows, instrument metadata, source proofs, and gap policies are accepted.

## Current Venue Coverage

| Venue | Accepted data | Coverage state | Coverage basis |
|---|---:|---|---|
| Polymarket (PMXT source) | 748 objects / 286,821,012,302 bytes | Partial | 748 of 1,148 planned PMXT streaming-manifest objects, 65.16% of attempted plan; 33.51% of 93-day hourly target |
| OKX | 5,825 objects / 79,852,835,301 bytes | Partial | 2026-03-01 through 2026-04-04 accepted; April 5 onward not accepted |
| Binance | 4,701 payloads / 42,358,207,176 bytes | Complete for current seven-token scope | Final manifest complete |
| Hyperliquid core | 10,180 objects / 9,162,435,699 bytes | Partial | Manifest has 799 source-availability gaps |
| Bybit | 319 payloads / 305,822,707 bytes | Partial | Archive tick trades only; REST, delivery, and volatility not complete |
| Hyperliquid HIP-4 | 87 objects / 44,592,796 bytes | Targeted tranche only | HIP-4 prediction-market metadata/data tranche, not seven-token coverage |
| Deribit | 7,544 raw objects / 15,346,229 bytes | Partial | Manifest has 1,118 errors and incomplete base coverage |
| Hyperliquid HIP-3 | 25 objects / 2,184,326 bytes | Targeted tranche only | HIP-3 metadata/funding style tranche |

Seven-token venue accepted total excluding Polymarket: 28,681 objects and
131,741,424,234 bytes.

Combined accepted total including Polymarket (PMXT source): 418,562,436,536
bytes.

## Important Open Gaps

- Polymarket (PMXT source): S3 physically has 914 objects and
  344,758,628,885 bytes under
  `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/`,
  but only 748 objects / 286,821,012,302 bytes are covered by completed
  manifests. Recover missing manifests or rerun missing offsets cleanly.
- OKX: April 5 onward is not accepted. Ignore old zero-payload family-split
  manifests and the rejected `ALL_SWAP` target manifest.
- Bybit: accepted data is archive tick trades only. REST, delivery, and
  historical-volatility completion remains open.
- Deribit: retry/full completion remains open; current accepted artifact is a
  partial evidence artifact.
- Hyperliquid core: do not call it complete. The manifest explicitly records
  799 source gaps.

## Detailed Evidence

- Detailed status log:
  `specs/023-nt-research-analytics-platform/reference/oneoff-seven-token-backfill-status-2026-06-02.md`
- Source bindings:
  `specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml`
- Evidence matrix:
  `specs/023-nt-research-analytics-platform/reference/backfill-evidence-matrix.v1.toml`
- Table contract:
  `specs/023-nt-research-analytics-platform/reference/backfill-table-contract.md`
