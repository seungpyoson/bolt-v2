# Quickstart: CI Nextest Artifact Cache

## Local Verification

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

## Inspect #195 Cache Invariants

```bash
rg -n "managed_target_dir_relative|cache-workspace-crates|add-rust-environment-hash-key|workspaces:|nextest-v3-shard|github.sha" .github/actions/setup-environment/action.yml .github/workflows/ci.yml scripts/verify_ci_workflow_hygiene.py
```

Expected workflow shape:

```yaml
workspaces: . -> ${{ steps.setup.outputs.managed_target_dir_relative }}
cache-targets: true
cache-workspace-crates: "true"
add-rust-environment-hash-key: "true"
key: nextest-v3-shard-${{ matrix.shard }}-of-4
```

## Exact CI Evidence Required Before Completion

After exact PR-head CI can run:

```bash
gh run view <cold-run-id> --repo seungpyoson/bolt-v2 --log
gh run view <warm-run-id> --repo seungpyoson/bolt-v2 --log
gh cache list --repo seungpyoson/bolt-v2 --sort last_accessed_at --limit 100
```

Record:

- cold and warm run IDs
- final head SHA
- test shard job IDs and durations
- cache-hit/restored-key lines
- archive size lines or cache API `sizeInBytes`
- whether warm shard logs still show `Compiling bolt-v2`
- timing comparison against the post-#193/#343 baseline

## Blocker Rule

If exact PR-head CI has not run, leave #195 and the PR in blocked/draft state. Local verification proves workflow shape only; it does not prove warm rerun behavior or cache size.
