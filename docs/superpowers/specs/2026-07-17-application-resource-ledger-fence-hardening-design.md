# Application Resource Ledger Fence Hardening Design

## Goal

Close the five confirmed fail-open verifier gaps in PR #1441 without changing the disabled Rust application-resource-ledger runtime or its public API.

## Scope

This slice changes only `scripts/verify_risk_closure_workspace_authority.py` and its Python mutation tests. The existing Rust capability types, compile-fail harness, generated configuration, and runtime audit configuration remain unchanged.

The verifier continues to be the structural source fence for the mechanically disabled ledger. Every protected invariant must fail closed when production Rust uses an alternate spelling or item form that preserves the forbidden semantics.

## Approach

Extend the existing Python scanner with narrowly scoped, token-aware structural helpers rather than adding a Rust parser dependency or comparing the complete protected modules against fixed source text. The helpers inspect only the authority and ledger surfaces governed by this fence. Unrecognized relevant syntax is rejected instead of assumed safe.

This preserves the repository's single Python verification path and remote-first Rust policy while avoiding a new compile-heavy tool or dependency.

## Production Attribute And Visibility Model

All privacy and trait checks operate on production-effective source. The scanner evaluates the supported `cfg(test)`, `cfg(not(test))`, and `cfg_attr` forms that can change a protected item's visibility or derives.

The raw authority must have exactly one production-effective definition, and that definition must be `pub(super)`. A test-only private-looking definition cannot satisfy the production privacy check. Public named, grouped, glob, or aliased re-exports capable of exposing the raw authority are rejected.

Production-effective `Clone` derivation is rejected whether expressed directly or through `cfg_attr`. Unsupported conditional attributes on a protected definition fail closed.

## Exact Capability Surface

The ledger surface census covers all public items, not only ordinary function declarations. It records public struct fields, functions and raw-identifier functions, constants, statics, type aliases, and use declarations.

The allowed production surface remains exactly the two handle factories and the two capability methods already present. The three protected structs have no public fields. No public constant, static, type alias, or re-export may introduce another capability route.

## Protected Type And Trait Resolution

Protected-name discovery resolves simple and grouped `use` aliases plus qualified paths used by trait targets. Construction and conversion trait implementations for the ledger and both handles remain forbidden under every resolved alias.

The raw authority remains non-`Clone` in production. Trait syntax that references a protected type but cannot be resolved by the supported scanner fails closed.

## Constructor Census

The constructor census covers inherent methods and module-level functions whose return type is a protected type, including aliases. It also rejects public or ledger-visible constants and statics whose declared type or initializer provides a protected constructor or factory function.

The raw authority continues to have exactly the two existing test-only inherent constructors. The application resource ledger continues to have exactly one test-only inherent constructor. No production free factory or function-pointer factory is permitted.

## Canonical Source Loading

Every `#[path]`, nested `cfg_attr(path = ...)`, and `include!` string target is resolved relative to the containing Rust source. Lexical `.` and `..` components and repeated separators are normalized before comparison with the protected owner and ledger files.

The existing canonical module declarations remain the only allowed loaders. Equivalent path spellings therefore cannot load either protected source a second time.

## Test Strategy

Each review finding receives a red-first mutation test that copies the valid fixture, introduces one forbidden production form, and asserts the applicable verifier error:

1. Conditional public raw authority plus wildcard re-export.
2. Public capability field, public function-pointer static, and raw-identifier escalation method.
3. Production-only `Clone` derive and grouped-alias conversion implementation.
4. Module-level raw-authority factory.
5. Normalized alternate source-loader path.

The mutation suite must demonstrate that each test fails against the pre-fix verifier for the expected missing rejection, then passes after the minimal scanner change. The unchanged valid fixture must continue to produce no errors, and all existing verifier tests must remain green.

## Evidence

- Red/green output for each new mutation test proves that the test detects its original bypass.
- The complete Python verifier suite proves regression compatibility.
- Direct execution of the authority verifier proves the repository source remains accepted.
- `just fmt-check`, `just deny`, `just ci-lint-workflow`, and `just source-fence-static` provide the permitted local policy evidence applicable to this Python-only change.
- An internal adversarial review checks the final diff and mutation coverage before any completion claim.

No local compile-heavy Rust verification is run. If the branch is published, exact-head remote evidence remains governed by the repository's current PR workflow.
