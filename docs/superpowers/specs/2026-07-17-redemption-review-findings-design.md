# Redemption Review Findings Design

## Scope

Address the three internal adversarial-review findings on the disabled Polymarket redemption-preparation slice without activating redemption, adding a caller, changing request semantics, or introducing another secret source.

## Design

### Zeroizing signer-key decode

Replace `PrivateKeySigner::from_str` with an explicit decode into `Zeroizing<[u8; 32]>`. Use `alloy_primitives::hex::decode_to_slice` so the accepted optional `0x` prefix remains unchanged, then construct the signer with `PrivateKeySigner::from_slice`. Map both decoding and signer-construction failures to the existing redacted `InvalidSigningKey` error. Add a static fence that rejects restoration of the `FromStr` path.

### Complete production-caller inspection

Discover Rust production sources from every repository `Cargo.toml` containing a package. Inspect each package's `src` tree plus explicit library and binary target paths, while excluding test-only source paths. Inspect only the pre-`#[cfg(test)]` portion of mixed production/test modules.

The redemption owner remains a special case: its one declaration is allowed, but any additional production reference to `prepare_redemption_request` is rejected. All other discovered production sources reject any reference to that symbol. This covers the root package, nested packages such as the Backtester vertical slice, and future packages without hardcoding their paths.

### Secret-output sinks

Extend the existing forbidden-observability surface with `dbg!`, `print!`, and `eprint!`. These are prohibited in the production portion of the redemption owner because direct formatting of an inner `Zeroizing<String>` exposes its value despite the containing credential type's redacted `Debug` implementation.

## Evidence

Use the existing Python mutation-test harness for test-first evidence:

1. Add regressions for `PrivateKeySigner::from_str`, an owner-module caller, a nested-package caller, and each missing output macro.
2. Run the focused tests before implementation and confirm they fail for the missing fences.
3. Implement the smallest verifier and signer-decode changes that make those regressions pass.
4. Run the full redemption verifier tests, the verifier against the repository, formatting, and permitted static/source-fence gates.
5. Use exact-head remote Rust verification for compile and behavior evidence; do not run local compile-heavy Rust commands.

## Non-goals

- No redemption activation, network submission, durable authority, or production caller.
- No new configuration fields, secret sources, dependencies, or compatibility paths.
- No changes to calldata, signing-domain, signature-packing, retry, lease, callback, or identity semantics.
