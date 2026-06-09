# IV Fixture Evidence Inventory

**Feature**: `specs/026-nt-backed-iv-engine/`

## Inventory

| Fixture | Owning task | Status |
|---|---:|---|
| `capability-ledger.toml` | `T035` | Created and enforced by `tests/bolt_v3_iv_capability.rs` |
| Full IV profile TOML fixture | `T081` | Covered by `tests/bolt_v3_iv_config.rs::valid_iv_toml` |
| Source/product mismatch TOML fixture | `T083` | Covered by config and selector mismatch tests |
| Numeric and convention bounds TOML fixture | `T084` | Covered by config, policy, and derive tests |
| Policy TOML fixtures | `T085` | Covered by projection, interpolation, fallback, quorum, helper, and derived-input policy fixtures |
| Raw event fixtures | `T051`-`T056` | Covered by ingest, store, live integration, and raw audit tests |
| Query authorization fixtures | `T101`-`T104` | Covered by `tests/bolt_v3_iv_query.rs` and live registry tests |
| Source-fence fixture cases | `T105`-`T108` | Covered by `tests/bolt_v3_iv_source_fence.rs` and `just source-fence` |

## Fixture Rules

- Fixtures must remain strategy-, venue-, market-, asset-, instrument-, source-,
  and cadence-agnostic unless they quote external NT evidence.
- Runtime values must come from TOML fixtures or test-local builders, not IV
  core code.
- Raw NT payload fixtures are audit/replay evidence only and must not become
  strategy query-handle inputs.
