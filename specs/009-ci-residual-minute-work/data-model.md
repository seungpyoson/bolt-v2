# Data Model: #344 Residual Minute-Consumption Work

## SafePathSet

- Patterns from `.github/workflows/ci.yml` `pull_request.paths-ignore`.
- Must remain narrow and exact.
- Drives docs table and pass-stub classifier.

## PathClassification

- `docs_only_safe`: every changed file matches `SafePathSet`.
- `full_ci_required`: at least one changed file does not match `SafePathSet`.
- `invalid`: changed-file evidence unavailable or malformed.

Rules:

- Empty changed-file input is invalid.
- Mixed safe+unsafe paths are `full_ci_required`.
- `.claude/rust-verification.toml`, `Cargo.lock`, workflow files, Rust source, `docs/**`, and `specs/**` are `full_ci_required`.

## PassStubGate

- Job names: `build`, `clippy`, `test`, and `gate`.
- Each required stub job runs the changed-file classifier directly.
- Classification failure fails the required stub job directly.

## BranchHygieneEntry

- `branch`: remote branch name.
- `sha`: branch head SHA.
- `classification`: active, reference-only, or dead-merged-prunable.
- `rationale`: evidence used.
- `proposed_action`: keep, label/reference, or delete only after explicit approval.
