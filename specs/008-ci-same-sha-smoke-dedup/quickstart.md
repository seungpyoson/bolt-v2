# Quickstart: #205 Same-SHA Smoke-Tag Dedup

Run from the feature worktree.

## Local verification

```bash
python3 -B scripts/test_find_same_sha_main_evidence.py
python3 -B scripts/test_verify_ci_workflow_hygiene.py
python3 -B scripts/verify_ci_workflow_hygiene.py
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml"); puts "OK"'
just ci-lint-workflow
```

## Inspect workflow contract

```bash
rg -n "same-sha-main-evidence|artifact-ids|source_run_id|check_suite_id|needs\\.same-sha-main-evidence|startsWith\\(github\\.ref, 'refs/tags/v'\\)" .github/workflows/ci.yml scripts
```

Expected properties:

- `same-sha-main-evidence` runs only on tag refs and exposes source run, check suite, artifact, and SHA outputs.
- Duplicate heavy lanes skip on tag refs.
- `gate` requires evidence success and duplicate-lane skips on tag refs.
- `gate` preserves normal success checks on PR and `main` push refs.
- `deploy` downloads with `artifact-ids`, `github-token`, `repository`, and source `run-id`.

## After-merge evidence needed for #205 closure

After this stack lands on `main`, push a smoke tag on the same SHA and capture:

- source `main` CI run ID and check suite ID
- tag CI run ID
- artifact ID reused by deploy
- tag run job timings showing duplicate `test` and `build` lanes skipped
- deploy timing and S3 target

Post that evidence to #205 and #333 before claiming the issue complete.
