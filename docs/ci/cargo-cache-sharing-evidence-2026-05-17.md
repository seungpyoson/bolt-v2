# Cargo Cache Sharing Evidence - 2026-05-17

Issue: #366

## Source Research

Pinned action source checked:

- `https://raw.githubusercontent.com/Swatinem/rust-cache/v2.9.1/action.yml`
- `https://raw.githubusercontent.com/Swatinem/rust-cache/v2.9.1/src/config.ts`

Observed semantics:

- `shared-key` is used instead of the automatic job-based key.
- runner OS and CPU architecture remain in the key.
- `cache-targets:false` avoids workspace target dirs.
- `cache-bin:false` keeps Cargo binaries out of the payload.
- `cache-directories` appends extra directories to the rust-cache payload, so managed target dirs must not be placed there for #366.

## Local Verification

Passed:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
python3 -m py_compile scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py
just ci-lint-workflow
git diff --check
```

Pending exact PR-head evidence:

- GitHub CI run ID
- job IDs
- cache restore/save durations
- cache keys observed in logs
- gate status
