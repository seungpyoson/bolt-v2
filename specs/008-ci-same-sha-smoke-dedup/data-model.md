# Data Model: #205 Same-SHA Smoke-Tag Dedup

## SameShaMainEvidence

- `source_run_id`: GitHub Actions run ID selected as trusted source evidence.
- `source_run_url`: Human-readable source run URL.
- `check_suite_id`: Source run check suite ID.
- `artifact_id`: Immutable artifact ID for `bolt-v2-binary`.
- `artifact_name`: Expected artifact name, `bolt-v2-binary`.
- `artifact_size`: Artifact size from GitHub metadata.
- `source_sha`: Exact SHA shared by the tag and source `main` run.

Validation:

- `source_sha` equals `GITHUB_SHA`.
- Source run is not the current tag run.
- Output fields are written to `GITHUB_OUTPUT` for downstream `gate` and `deploy`.

## TrustedSourceRun

- `name`: `CI`.
- `path`: `.github/workflows/ci.yml`.
- `event`: `push`.
- `head_branch`: `main`.
- `head_sha`: exact tag SHA.
- `status`: `completed`.
- `conclusion`: `success`.

Validation:

- Runs with wrong branch, wrong SHA, wrong workflow path, incomplete status, or non-success conclusion are not eligible.
- At least one eligible run must also pass required job and artifact checks.

## RequiredJobEvidence

- Required non-test jobs: `detector`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, `build`, `gate`.
- Required test evidence: four successful `test` matrix shards.

Validation:

- Every required job must be `completed/success`.
- Missing, failed, cancelled, skipped, malformed, or incomplete required jobs fail resolution.

## ReusableArtifact

- `name`: `bolt-v2-binary`.
- `id`: artifact ID used by deploy.
- `expired`: must be `false`.
- `workflow_run.id`: source run ID.
- `workflow_run.head_branch`: `main`.
- `workflow_run.head_sha`: exact tag SHA.

Validation:

- Missing, expired, ambiguous, wrong-run, wrong-branch, or wrong-SHA artifacts fail resolution.

## TagReuseGate

- Inputs: `needs.detector.result`, `needs.same-sha-main-evidence.result`, duplicate lane results, normal lane results.
- Tag mode: requires detector success, evidence success, and duplicate lanes skipped.
- Non-tag mode: requires detector success, evidence skipped, normal lane success, and existing build-required semantics.

Validation:

- `gate` runs with `always()` so expected skipped jobs do not hide failures.
- `deploy` also uses `always()` but only proceeds after `gate` and `same-sha-main-evidence` success on tag refs.
