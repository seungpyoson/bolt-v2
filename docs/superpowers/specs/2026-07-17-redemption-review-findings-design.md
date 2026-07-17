# Redemption Review Findings Design

## Scope

Address the internal adversarial-review findings on the disabled Polymarket redemption-preparation slice without activating redemption, adding a production caller, changing request semantics, or introducing another secret source.

## Decisions

### Compiler-enforced disabled boundary

`prepare_redemption_request` consumes a public `RedemptionPreparationPermit` by value. The permit has a private field, implements neither `Clone` nor `Copy`, and has no production constructor or issuer. Safe Rust outside the owning module can name the type but cannot create a value, so it cannot call request preparation.

The owner module's tests construct the private permit and call the same production entrypoint. No runtime feature flag can issue the permit or activate the boundary, and there is no fallback entrypoint, compatibility adapter, source scan, caller allowlist, or inferred test/production path. The existing false-only configuration remains a fail-closed invariant. Future activation requires an explicit governed code change that adds the one permit issuer.

Compile-fail cases prove that external code cannot construct the permit, cannot clone it, cannot substitute a new-risk reservation for the recovery lease, cannot let prepared bytes escape the callback, and cannot serialize resolved credentials.

### Zeroizing signer-key snapshot

The existing provider boundary resolves the signer once and validates it through NT's `EvmPrivateKey`. The provider copies the validated 32-byte scalar into `ResolvedEvmSigningKey`, whose fixed-width storage is zeroized on drop. Redemption preparation borrows that snapshot, copies it into its own fixed-width zeroizing buffer, and passes the bytes to `PrivateKeySigner::from_slice`; it performs no second SSM lookup or heap-based hex decode. Validation and signer-construction failures remain redacted. Verification uses Rust behavior tests, direct diff inspection, and internal adversarial review; it does not infer safety from source spelling.

### Existing application-resource authority

The current `ApplicationResourceLedger` remains the sole owner of the risk-closure workspace authority. Redemption preparation accepts the ledger module's re-exported retained-recovery `RiskClosureWorkspaceLease`; it does not restore the retired top-level workspace module or add a production constructor, allocator, compatibility adapter, or alternate authority. A test-only ledger helper may construct the same retained-recovery lease for owner-module behavior tests, while compiler-negative tests continue to reject a new-risk reservation.

### Deterministic static verification only

The Python verifier parses TOML and compares structured values, pinned source-evidence hashes, exact dependency versions, and the declared compile-fail test target. It also invokes the existing generator in check mode. It does not parse, lex, search, or classify Rust source.

TOML authority checks walk parsed key trees. Comments, whitespace, file layout, macro spelling, aliases, generated includes, and source paths cannot affect the result. No regular expression is used.

### Secret-output finding

The resolved signing-key view retains private fields, zeroizing storage, no `Debug` implementation, and no serialization implementation. The provider snapshot retains its redacted `Debug` implementation. The implementation contains no logging or output sink. Evidence is direct inspection plus compile-fail serialization proof and internal adversarial review, not a predictive macro-name scanner.

### One resolved signer snapshot

Polymarket SSM resolution remains owned by the existing provider boundary. That boundary resolves the private key once, validates it once, and retains a neutral opaque `ResolvedEvmSigningKey` inside the provider snapshot alongside the exact resolved string needed by existing NT consumers and redaction scans. Redemption preparation borrows that stored signing-key view; it performs no SSM lookup, does not depend on the concrete provider type, and has no credential fallback.

Builder credentials remain grouped in TOML but are not resolved by this disabled preparation slice. Their first consumer belongs to the later submission work tracked by issue #1384.

### Review-fix closure

The request signer is copied into `Zeroizing<[u8; 32]>` before Alloy signer construction. An oversized request nonce reports a request-input error rather than a configuration error. The unused `output_asset` authority is removed from runtime config and deployment-fact evidence. Compiler-negative tests invoke `cargo` through governed `PATH`, never through the build-time absolute `CARGO` path. The PR body describes #1384 without a GitHub closing keyword.

## Evidence

1. A Python regression first demonstrates that arbitrary Rust text is outside the verifier's authority.
2. Focused Python tests cover parsed configuration authority, pinned evidence, dependency pins, and compile-fail target wiring.
3. Rust compile-fail tests prove the capability and borrow boundaries at exact head.
4. Existing Rust behavior tests exercise the same preparation entrypoint with the owner-only test permit.
5. Permitted local formatting and source-fence gates run before internal adversarial review.
6. Exact-head remote Rust verification supplies compiler and behavior evidence; no local compile-heavy Rust command is used.

## Non-goals

- No redemption activation, submission, new durable authority, or production caller.
- No new runtime configuration, secret source, dependency, or compatibility path.
- No changes to calldata, signing-domain, signature-packing, retry, lease, callback, or identity semantics.
- No attempt to reproduce Rust compiler semantics in Python.
- No output-asset/post-state binding, builder API resolution, network submission, query/finality handling, LiveNode wiring, deployment, activation, or trading. Those remain tracked by issue #1384; application-resource-ledger ownership was delivered separately by #1382/#1441.
