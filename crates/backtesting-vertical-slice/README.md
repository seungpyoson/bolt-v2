# backtesting-vertical-slice

## MinIO S3 catalog smoke

The S3 catalog smoke tests are opt-in for local runs. Start a MinIO-compatible S3 endpoint using the values in `tests/fixtures/s3_catalog_smoke.toml`, then run the BVS test harness with `BVS_MINIO_S3_SMOKE=1`.

In CI, `backtester-ci.yml` sets that opt-in and runs a pinned MinIO container from the same fixture. If MinIO is unreachable in CI, the tests fail rather than skipping.

Spec prerequisite 0.6 remains open; this smoke covers the BVS MinIO-backed catalog path and does not claim broader S3 conformance.
