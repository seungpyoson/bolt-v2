# IV Fixture Evidence Inventory

**Feature**: `specs/026-nt-backed-iv-engine/`

## Inventory

| Fixture | Owning task | Status |
|---|---:|---|
| `capability-ledger.toml` | `T035` | Not created yet |
| Full IV profile TOML fixture | `T081` | Not created yet |
| Source/product mismatch TOML fixture | `T083` | Not created yet |
| Numeric and convention bounds TOML fixture | `T084` | Not created yet |
| Policy TOML fixtures | `T085` | Not created yet |
| Raw event fixtures | `T051`-`T056` | Not created yet |
| Query authorization fixtures | `T101`-`T104` | Not created yet |
| Source-fence fixture cases | `T105`-`T108` | Not created yet |

## Fixture Rules

- Fixtures must remain strategy-, venue-, market-, asset-, instrument-, source-,
  and cadence-agnostic unless they quote external NT evidence.
- Runtime values must come from TOML fixtures or test-local builders, not IV
  core code.
- Raw NT payload fixtures are audit/replay evidence only and must not become
  strategy query-handle inputs.
