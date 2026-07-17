# Redemption Review Findings Design

## Scope

Address the three internal adversarial-review findings on the disabled Polymarket redemption-preparation slice without activating redemption, adding a caller, changing request semantics, or introducing another secret source.

## Design

### Zeroizing signer-key decode

Replace `PrivateKeySigner::from_str` with an explicit decode into `Zeroizing<[u8; 32]>`. Use `alloy_primitives::hex::decode_to_slice` so the accepted optional `0x` prefix remains unchanged, then construct the signer with `PrivateKeySigner::from_slice`. Map both decoding and signer-construction failures to the existing redacted `InvalidSigningKey` error. The static fence positively requires the ordered decode-to-zeroizing-buffer and `from_slice` structure in `prepare_redemption_request`, and also rejects equivalent `FromStr` and `.parse::<PrivateKeySigner>` paths.

### Complete production-caller inspection

Conservatively inspect Rust code in every repository `.rs` file outside source-control worktrees and build-output directories. Do not infer test-only status from a `tests` path, and do not exempt generated or dormant source. Lexically ignore comments and string/character literals so embedded compile-fail fixtures are not mistaken for callers.

The redemption owner remains the sole exception: its production prefix may contain the one declaration. Its single `#[cfg(test)] mod tests` item must be structurally valid and final, with no later production items. All other Rust code rejects any reference to `prepare_redemption_request`, covering nested packages, custom target layouts, `src/**/tests/`, and generated source without path-specific caller exemptions.

### Secret-output sinks

Extend the existing forbidden-observability surface with `dbg!`, `print!`, and `eprint!`. These are prohibited in the production portion of the redemption owner because direct formatting of an inner `Zeroizing<String>` exposes its value despite the containing credential type's redacted `Debug` implementation.

## Evidence

Use the existing Python mutation-test harness for test-first evidence:

1. Add regressions for unsafe signer-parser spellings and missing positive decode structure; owner, nested-package, test-named-directory, post-test, custom-target-descendant, and generated callers; and each missing output macro.
2. Run the focused tests before implementation and confirm they fail for the missing fences.
3. Implement the smallest verifier and signer-decode changes that make those regressions pass.
4. Run the full redemption verifier tests, the verifier against the repository, formatting, and permitted static/source-fence gates.
5. Use exact-head remote Rust verification for compile and behavior evidence; do not run local compile-heavy Rust commands.

## Non-goals

- No redemption activation, network submission, durable authority, or production caller.
- No new configuration fields, secret sources, dependencies, or compatibility paths.
- No changes to calldata, signing-domain, signature-packing, retry, lease, callback, or identity semantics.
