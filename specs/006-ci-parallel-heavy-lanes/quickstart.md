# CI Parallel Heavy Lanes Quickstart

## Local validation

Run the topology verifier self-tests:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
```

Run the workflow verifier directly:

```bash
python3 scripts/verify_ci_workflow_hygiene.py
```

Run the integrated workflow lint:

```bash
just ci-lint-workflow
```

Check justfile passthrough shape without running tests:

```bash
just --dry-run test -- --partition count:1/4
```

If the dry-run output does not route through `rust_verification.py run --repo ... test -- --partition count:1/4`, the managed passthrough contract is broken.

## Expected CI topology

The PR workflow should include these required jobs:

```text
detector
fmt-check
deny
clippy
check-aarch64
source-fence
test (matrix shard 1/4, 2/4, 3/4, 4/4)
build
gate
deploy
```

The aggregate gate should require:

```text
detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build
```

The `test` job should log a local reproduction command equivalent to:

```bash
just test -- --partition count:<shard>/4
```

## Source-fence ownership

This slice intentionally keeps the #342 source-fence filters inside full nextest as duplicate coverage. The duplication is valid only because:

- `source-fence` remains required before `test`.
- `gate` requires both `source-fence` and aggregate `test`.
- The workflow and PR body document the duplicate ownership.

## Exact-run evidence

Before opening or updating the PR as ready, record:

- Baseline: #343 run `25855655415`, `docs/ci/ci-baseline-2026-05-15.md`
- Final PR-head SHA
- Final PR-head CI run id
- `clippy`, `check-aarch64`, each `test` shard, and `gate` durations
- Critical-path comparison

If stacked PRs do not trigger full CI for non-`main` bases, do not claim exact-head CI green. Ask before retargeting or otherwise mutating PR topology to obtain exact CI.
