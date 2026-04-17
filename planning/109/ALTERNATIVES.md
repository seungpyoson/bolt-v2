# Issue #109 Design Alternatives

## Recommended: Structured Parsing With Canonical Basis Strings At The Seam

- Parse ruleset strings and market metadata into the same internal `ResolutionBasis` structure.
- Persist the canonical normalized string at the catalog boundary.
- Compare bases by parsing both sides structurally in the selector.
- Use the parsed family component for runtime validation.

Why this wins:

- removes asset-specific literals from the selector seam
- eliminates brittle raw-string equality
- keeps the config format and candidate payloads stable
- allows fail-closed parsing at both config and metadata boundaries

## Alternative 2: Canonical String Normalization Only

- Keep strings everywhere.
- Parse market metadata into a canonical string and normalize ruleset strings before compare.

Why it loses:

- trust boundary stays implicit
- structure is harder to inspect and validate
- future logic will be tempted back into ad hoc string slicing

## Alternative 3: Full Config Schema Redesign

- Replace `resolution_basis = "..."` with nested TOML tables.

Why it loses for this slice:

- broader migration than issue #109 requests
- touches unrelated config/rendering surfaces
- increases rollout cost without improving selector safety enough to justify scope expansion
