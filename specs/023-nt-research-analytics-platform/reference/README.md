# Reference Directory

This directory is the cross-project reference layer for the
`023-nt-research-analytics-platform` planning package. It is not a fourth
implementation project.

## Authoritative Inputs

These files are active inputs inherited by the numbered project docs:

- `evidence.md`: claim ledger and evidence status control surface.
- `data-model.md`: cross-project entities and artifact contract vocabulary.
- `contracts.md`: binding cross-project rules and source-of-truth contracts.
- `backfill-table-contract.md`: canonical table-family, row, identity-column,
  and schema vocabulary. Its legacy venue/product coverage and source notes are
  historical evidence, not acquisition authority.
- `historical-data-acquisition-architecture.v1.md`: owner-approved source
  selection, immutable publication/read binding, NT replay prerequisites,
  data-family ownership, and AWS cost boundary for historical backtests. It
  supersedes `normalization-catalog-plan.v3.md`.

Per-project evidence tables and requirements are derived views of these files.
If they disagree, fix the derived project doc or update the authoritative file
with evidence.

For historical acquisition and backtest-input identity,
`historical-data-acquisition-architecture.v1.md` overrides older project text
and immutable proof artifacts. It specifically overrides the source-selection
and venue/product-coverage clauses in `backfill-table-contract.md`, whose table
schema remains active. General result and derived-artifact discovery remains
governed by `contracts.md`.

Audit and decision-history files live in `../archive/`. Do not add historical
or superseded planning material here unless it is promoted into live authority.
