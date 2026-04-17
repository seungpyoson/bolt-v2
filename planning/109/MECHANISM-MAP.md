# Issue #109 Mechanism Map

This is the final state-to-mechanism map for the changed seam.

| State | Mechanism | Condition | Timing | Outcome |
| --- | --- | --- | --- | --- |
| Valid ruleset basis parses canonically | `src/platform/resolution_basis.rs:65-92` | `ruleset.resolution_basis` splits into valid ASCII alphanumeric parts | runtime validation and selector comparison | canonical structured basis exists |
| Invalid ruleset basis halts config load | `src/validate.rs:78-90`, `src/validate.rs:728-733`, `src/validate.rs:1410-1413` | non-empty `resolution_basis` fails `parse_resolution_basis` | local/runtime validation before selector start | validation error `invalid_resolution_basis` |
| Recognized family still requires matching reference venue kind | `src/platform/resolution_basis.rs:106-118`, `src/validate.rs:1477-1494` | parsed family maps to a `ReferenceVenueKind` and no matching venue exists | runtime validation | validation error `missing_reference_venue_family` |
| Unknown structurally valid family preserves previous semantics | `src/platform/resolution_basis.rs:106-118` | parsed family does not map to a configured `ReferenceVenueKind` | runtime validation | no family-specific validation error is added |
| Market metadata resolves to canonical basis | `src/platform/resolution_basis.rs:95-166`, `src/platform/polymarket_catalog.rs:82-100` | declared metadata contains a supported family and a safe separated symbol pair | candidate translation | candidate stores canonical basis string |
| Ambiguous metadata cannot create a candidate | `src/platform/resolution_basis.rs:121-214`, `src/platform/polymarket_catalog.rs:87-88` | no safe separated symbol pair is found | candidate translation | market is dropped from the candidate list |
| Selector matches structurally, not by raw string | `src/platform/ruleset.rs:124-160` | both ruleset and market basis strings parse to equal `ResolutionBasis` values | each selector evaluation | candidate stays eligible |
| Parse failure or structural mismatch rejects safely | `src/platform/ruleset.rs:131-132`, `src/platform/ruleset.rs:150-160` | either side fails to parse or parsed values differ | each selector evaluation | reject reason `resolution_basis_mismatch` |
| ETH family succeeds without ETH literals | `src/platform/resolution_basis.rs:121-214`, `tests/polymarket_catalog.rs:233-253`, `tests/ruleset_selector.rs:66-99` | metadata/config vary only in asset pair inside an existing family pattern | parser and selector test execution | ETH basis parses and selects through the generic path |
