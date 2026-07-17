# Redemption Review Findings Design

## Scope

Address the internal adversarial-review findings on the disabled Polymarket redemption-preparation slice without activating redemption, adding a production caller, changing request semantics, or introducing another secret source.

## Decisions

### Compiler-enforced disabled boundary

`prepare_redemption_request` consumes a public `RedemptionPreparationPermit` by value. The permit has a private field, implements neither `Clone` nor `Copy`, and has no production constructor or issuer. Safe Rust outside the owning module can name the type but cannot create a value, so it cannot call request preparation.

The owner module's tests construct the private permit and call the same production entrypoint. No runtime feature flag can issue the permit or activate the boundary, and there is no fallback entrypoint, compatibility adapter, source scan, caller allowlist, or inferred test/production path. The existing false-only configuration remains a fail-closed invariant. Future activation requires an explicit governed code change that adds the one permit issuer.

Compile-fail cases prove that external code cannot construct the permit, cannot clone it, cannot substitute a new-risk reservation for the recovery lease, cannot let prepared bytes escape the callback, and cannot serialize resolved credentials.

### Zeroizing signing-key value

The existing provider boundary resolves the signer once and validates it through NT's `EvmPrivateKey`. `ResolvedEvmSigningKey` copies validated scalar bytes directly into `Zeroizing<[u8; 32]>` and validates the secp256k1 scalar itself, so invalid bytes cannot construct the type and no plain fixed-width key array exists at the boundary. Its accessor returns `&[u8; 32]` through dereferencing rather than slice-shaped `AsRef` inference. Redemption preparation borrows this opaque value, copies into another already-wrapped zeroizing buffer, and performs no SSM lookup or heap-based hex decode. Validation and signer-construction failures remain redacted.

### Existing application-resource authority

The current `ApplicationResourceLedger` remains the sole owner of the risk-closure workspace authority. Redemption preparation accepts the ledger module's re-exported retained-recovery `RiskClosureWorkspaceLease`; it does not restore the retired top-level workspace module or add a production constructor, allocator, compatibility adapter, or alternate authority. A test-only ledger helper may construct the same retained-recovery lease for owner-module behavior tests, while compiler-negative tests continue to reject a new-risk reservation.

### Deterministic static verification only

The Python verifier parses TOML and compares structured values, pinned source-evidence hashes, exact dependency versions, and the declared compile-fail test target. It also invokes the existing generator in check mode. It does not parse, lex, search, or classify Rust source.

TOML authority checks walk parsed key trees. Comments, whitespace, file layout, macro spelling, aliases, generated includes, and source paths cannot affect the result. No regular expression is used.

### Secret-output finding

The resolved signing-key value retains private fields, zeroizing storage, no `Debug` implementation, and no serialization implementation. The provider secrets retain their redacted `Debug` implementation. The implementation contains no logging or output sink. Evidence is direct inspection plus compile-fail serialization proof and internal adversarial review, not a predictive macro-name scanner.

### One provider secret representation

Polymarket SSM resolution remains owned by the existing provider boundary. That boundary resolves the private key once, validates it, and retains only the exact zeroizing string needed by existing NT consumers and redaction scans. It does not retain a second fixed-width representation that could disagree with the string. The disabled redemption primitive consumes a checked `ResolvedEvmSigningKey` directly; its future production binding remains in issue #1384 and must choose one representation rather than adding parallel fields or a fallback.

Builder credentials, AWS region, and the signer SSM path are not repeated in the redemption TOML or generated projection. The existing `clients.polymarket_main` provider configuration is their sole authority. Builder credential consumption belongs to the later submission work tracked by issue #1384 and must use that provider boundary when it lands.

### Derived protocol selector and reproducible evidence

The evidence TOML carries the canonical function signature but no independently editable selector. The generator derives the selector with an in-repository Ethereum Keccak-256 implementation that has known-answer tests; NIST SHA3 and external Python packages are not used.

Each pinned upstream file has a repository-owned immutable hexadecimal byte capture and SHA-256. Its capture path is derived from repository URL, revision, and source path rather than configured independently. URLs, DNS-bounded hostnames, repository-relative source paths, and observation dates are accepted only when they equal their single derived canonical host, path, and ISO-date spelling. The official Polymarket contracts page is captured the same way at a path derived from its URL and observation date, decoded as Markdown, hashed, and parsed structurally to prove chain ID, collateral address, and both adapter addresses against runtime TOML. CI performs no network fetch and has no cache, refresh, or missing-file fallback. Repository checks therefore reproduce the exact external bytes reviewed for this slice.

Every evidence input has one deterministic transformation. Validation can only accept that transformation or reject the input; it cannot choose an alternate source, decoder, selector, cached value, compatibility rule, or condition-specific result.

### Review-fix closure

The fixed-width signer type is explicit at every boundary and has no unchecked constructor. An oversized request nonce reports a request-input error rather than a configuration error. The unused `output_asset` and credential-set authorities are removed from runtime config. Compiler-negative tests invoke `cargo` through governed `PATH`, never through the build-time absolute `CARGO` path. The PR body describes verification as a merge requirement, not exact-head evidence already obtained, and describes #1384 without a GitHub closing keyword.

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
- No network-backed evidence refresh, conditional evidence source, or cache-as-proof path.
- No changes to calldata, signing-domain, signature-packing, retry, lease, callback, or identity semantics.
- No attempt to reproduce Rust compiler semantics in Python.
- No output-asset/post-state binding, builder API resolution, network submission, query/finality handling, LiveNode wiring, deployment, activation, or trading. Those remain tracked by issue #1384; application-resource-ledger ownership was delivered separately by #1382/#1441.
