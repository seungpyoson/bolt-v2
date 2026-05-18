# Quickstart: CI Cargo Cache Sharing

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

After PR CI runs, collect cache evidence:

```bash
gh pr view <PR> --json headRefOid,statusCheckRollup
gh run view <RUN_ID> --json jobs
gh run view <RUN_ID> --job <JOB_ID> --log
```

Required evidence:

- shared registry/git key appears as `cargo-registry-git-v1`
- target cache keys include `clippy-host`, `check-aarch64-dev`, `source-fence-test`, and `build-aarch64-release` where those jobs run
- source-fence, test, build, clippy, deny, and gate still pass or skip only under existing detector/tag policy
