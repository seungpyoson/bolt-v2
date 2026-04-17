# Issue #109 Universe Enumeration

## Scope

Resolution-basis parsing, market translation, selector matching, and runtime validation for the ruleset-to-market seam.

## Reachable State Classes

| ID | State | Entry condition | Expected outcome | Safe failure mode | Owning mechanism |
| --- | --- | --- | --- | --- | --- |
| U1 | Valid ruleset basis | `ruleset.resolution_basis` is structurally parseable | selector can compare canonically | n/a | `src/platform/resolution_basis.rs`, `src/platform/ruleset.rs` |
| U2 | Invalid ruleset basis | config value is non-empty but malformed | runtime validation rejects config before selection starts | halt config load with validation error | `src/validate.rs` |
| U3 | Supported family without matching reference venue | parsed family implies a required configured reference venue kind that is absent | runtime validation rejects config | halt config load with validation error | `src/validate.rs` |
| U4 | Unknown family but structurally valid basis | basis parses but does not imply a configured venue kind | preserve current semantics; no family-specific validation error | selection still compares canonically | `src/validate.rs`, `src/platform/ruleset.rs` |
| U5 | Market metadata resolves to a basis | Polymarket description or source yields a canonical basis | market becomes a candidate | n/a | `src/platform/resolution_basis.rs`, `src/platform/polymarket_catalog.rs` |
| U6 | Market metadata is ambiguous or missing | no safe canonical basis can be derived | market must not enter candidate set | drop market from candidates | `src/platform/polymarket_catalog.rs` |
| U7 | Ruleset and market basis match structurally | family, symbol pair, and cadence agree canonically | candidate remains eligible | n/a | `src/platform/ruleset.rs` |
| U8 | Ruleset and market basis mismatch | any canonical component differs | candidate rejected with `resolution_basis_mismatch` | reject candidate | `src/platform/ruleset.rs` |
| U9 | ETH family in existing oracle pattern | metadata/config differ only by asset pair, not code literals | candidate can be discovered and selected | reject only on true mismatch | same as U5 and U7 |

## Unsafe States To Eliminate

- BTC-only parsing logic in platform code.
- Raw-string equality as the selector match rule.
- Config values that bypass validation because they are non-empty but malformed.
- Metadata heuristics that can coerce ambiguous descriptions into the wrong basis.
