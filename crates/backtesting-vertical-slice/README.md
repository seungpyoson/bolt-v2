# backtesting-vertical-slice

## Current decision-evidence read bound

Every compiled Bolt strategy manifest supplies a positive
`strategy.parameters.evidence_read_max_bytes`. The bound is selected before the run from the
fixture horizon and maximum event cadence, with headroom for the complete machine and observation
streams. It is never derived from the retained file length. Crossing the bound fails the run guard
closed; it does not truncate or suppress writes. Rotation and bounded retention remain owned by
issue #1385.

## MinIO S3 catalog smoke

The S3 catalog smoke tests are opt-in for local runs. Start a MinIO-compatible S3 endpoint using the values in `tests/fixtures/s3_catalog_smoke.toml`, then run the BVS test harness with `BVS_MINIO_S3_SMOKE=1`.

The advisory workflow deliberately excludes only the
`backtesting_vertical_slice_s3_catalog_smoke` module because it requires that external service.
An explicitly provisioned MinIO run sets `BVS_MINIO_S3_SMOKE=1`; if MinIO is then unreachable, the
tests fail rather than skipping.

Spec prerequisite 0.6 remains open; this smoke covers the BVS MinIO-backed catalog path and does not claim broader S3 conformance.
