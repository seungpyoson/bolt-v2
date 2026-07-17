# Redemption Review Findings Design

## Scope

Address the internal adversarial-review findings on the disabled Polymarket redemption-preparation slice without activating redemption, adding a production caller, changing request semantics, or introducing another secret source.

## Decisions

### Compiler-enforced disabled boundary

`prepare_redemption_request` consumes a public `RedemptionPreparationPermit` by value. The permit has a private field, implements neither `Clone` nor `Copy`, and has no production constructor or issuer. Safe Rust outside the owning module can name the type but cannot create a value, so it cannot call request preparation.

The owner module's tests construct the private permit and call the same production entrypoint. No runtime feature flag can issue the permit or activate the boundary, and there is no fallback entrypoint, compatibility adapter, source scan, caller allowlist, or inferred test/production path. The existing false-only configuration remains a fail-closed invariant. Future activation requires an explicit governed code change that adds the one permit issuer.

Compile-fail cases prove that external code cannot construct the permit, cannot clone it, cannot substitute a new-risk reservation for the recovery lease, cannot let prepared bytes escape the callback, and cannot serialize resolved credentials.

### Zeroizing signer-key decode

The signer key is decoded directly into `Zeroizing<[u8; 32]>` and passed to `PrivateKeySigner::from_slice`. Decode and signer-construction failures remain mapped to the redacted `InvalidSigningKey` error. Verification uses Rust behavior tests, direct diff inspection, and internal adversarial review; it does not infer safety from source spelling.

### Deterministic static verification only

The Python verifier parses TOML and compares structured values, pinned source-evidence hashes, exact dependency versions, and the declared compile-fail test target. It also invokes the existing generator in check mode. It does not parse, lex, search, or classify Rust source.

TOML authority checks walk parsed key trees. Comments, whitespace, file layout, macro spelling, aliases, generated includes, and source paths cannot affect the result. No regular expression is used.

### Secret-output finding

Resolved credentials retain private fields, zeroizing storage, a redacted `Debug` implementation, and no serialization implementation. The implementation contains no logging or output sink. Evidence is direct inspection plus compile-fail serialization proof and internal adversarial review, not a predictive macro-name scanner.

## Evidence

1. A Python regression first demonstrates that arbitrary Rust text is outside the verifier's authority.
2. Focused Python tests cover parsed configuration authority, pinned evidence, dependency pins, and compile-fail target wiring.
3. Rust compile-fail tests prove the capability and borrow boundaries at exact head.
4. Existing Rust behavior tests exercise the same preparation entrypoint with the owner-only test permit.
5. Permitted local formatting and source-fence gates run before internal adversarial review.
6. Exact-head remote Rust verification supplies compiler and behavior evidence; no local compile-heavy Rust command is used.

## Non-goals

- No redemption activation, submission, durable authority, or production caller.
- No new runtime configuration, secret source, dependency, or compatibility path.
- No changes to calldata, signing-domain, signature-packing, retry, lease, callback, or identity semantics.
- No attempt to reproduce Rust compiler semantics in Python.
