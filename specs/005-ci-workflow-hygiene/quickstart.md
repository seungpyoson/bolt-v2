# CI Workflow Hygiene Quickstart

## Local Red/Green Checks

From the #203 worktree:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
```

Expected result after implementation: all commands pass.

## Workflow Sanity Checks

```bash
rg -n "fmt-check:|needs:|include-managed-target-dir|deploy:|source-fence|needs\\.(detector|fmt-check|deny|clippy|source-fence|test|build)\\.result" .github/workflows/ci.yml
rg -n "live-node|max-threads|lake_batch|nt_runtime_capture|platform_runtime" .config/nextest.toml
```

Expected evidence:

- `fmt-check` has no `needs: detector`.
- `build` still has `needs: detector` and `if: needs.detector.outputs.build_required == 'true'`.
- Jobs using `steps.setup.outputs.managed_target_dir` set `include-managed-target-dir: "true"`.
- `deploy.needs` includes all required safety lanes directly.
- `gate` checks all required lane results.
- `.config/nextest.toml` assigns `lake_batch`, `nt_runtime_capture`, and `platform_runtime` to the `live-node` group with `max-threads = 1`.

## Verification Gate

```bash
just fmt-check
just ci-lint-workflow
git diff --check
```

Exact-head CI must pass before external review:

- `detector`
- `fmt-check`
- `deny`
- `clippy`
- `source-fence`
- `test`
- `build`
- `gate`
