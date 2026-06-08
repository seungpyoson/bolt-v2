# Internal Review

**Feature**: `specs/026-nt-backed-iv-engine/`
**Review date**: 2026-06-08
**Reviewed head before final evidence commit**: `ebd1a09d790ee6b242a9de49189bcfb7e361dd6e`

## Findings

No blocking findings remain after the local review fixes below.

## Issues Found And Fixed

| Issue | Evidence | Resolution |
|---|---|---|
| Strategy query contract mentioned projected scalar and derived IV products, but `IvQueryHandle` returned `UnsupportedProductKind` for both. | Added RED test `cargo test --locked --test bolt_v3_iv_query`; it failed with missing projection/helper handle methods and missing projected/derived product variants. | Added `IvProjectedScalarIv`, `IvQueryProduct::ProjectedScalarIv`, boxed `IvQueryProduct::DerivedIv`, projection-policy routing, helper-policy routing, and engine-owned derived input routing. GREEN query target now passes 5 tests. |
| Source-fence legacy default check rejected production IV defaults. | `just source-fence` failed on production `Default` derives, `IvStore::default()`, and `unwrap_or_default` calls. | Replaced implicit production defaults with explicit `empty()` constructors and explicit fallback handling; updated tests to call those constructors. |
| Clippy rejected the derived query product enum layout. | `cargo clippy --locked --lib -- -D warnings` failed on `clippy::large-enum-variant`. | Boxed `IvQueryProduct::DerivedIv`; clippy passes. |

## Residual Risk

- No GitHub PR or CI run exists for this exact final local head. External review was intentionally not requested under the no-PR workflow.
- Live integration coverage proves root TOML parsing, strategy handle registration, IV lifecycle planning, source health, and source-fence behavior. It does not run an end-to-end live market-data session against a real NT venue adapter.
- Derived IV strategy queries require engine-owned `IvDerivedInputSet` state. The query handle rejects missing helper policy or missing derived inputs; source-fence prevents strategies from deriving locally.

## Verification

- `cargo test --locked bolt_v3_iv`: PASS
- `cargo test --locked --test bolt_v3_iv_capability --test bolt_v3_iv_config --test bolt_v3_iv_live_integration --test bolt_v3_iv_subscription --test bolt_v3_iv_ingest --test bolt_v3_iv_store --test bolt_v3_iv_query --test bolt_v3_iv_policy --test bolt_v3_iv_derive --test bolt_v3_iv_source_fence --test config_parsing`: PASS
- `cargo fmt --check`: PASS
- `cargo clippy --locked --lib -- -D warnings`: PASS
- `just source-fence`: PASS
