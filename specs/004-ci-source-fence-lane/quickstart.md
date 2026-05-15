# Quickstart: CI Source-Fence Lane

Run these from the feature worktree.

## Red Check

```bash
just ci-lint-workflow
```

Expected before workflow implementation: linter reports the missing #342 `source-fence` invariants.

## Local Source-Fence Lane

```bash
just source-fence
```

Expected after implementation: all six verifier scripts pass, then these structural tests pass:

```bash
cargo test --test bolt_v3_controlled_connect live_node_module_only_runs_nt_after_live_canary_gate -- --nocapture
cargo test --test bolt_v3_production_entrypoint -- --nocapture
```

The actual recipe runs the cargo filters through the managed Rust verification owner.

## Deliberate Stale Assertion Proof

Temporarily change one source-fence assertion to search for stale production source text, then run:

```bash
just source-fence
```

Expected: the recipe fails in the targeted source-fence filter without running `just test` or installing/running full `cargo nextest`. Revert the temporary mutation before committing.

## Final Checks

```bash
just ci-lint-workflow
just source-fence
git diff --check
```

After push, collect exact-head CI evidence for `source-fence`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `test`, and `gate`.
