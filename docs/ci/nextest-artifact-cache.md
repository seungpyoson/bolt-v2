# Nextest Artifact Cache

Root `nextest-archive` / `root-bin-sidecars` payloads and BVS
`bvs-nextest-archive` / `bvs-bin-sidecars` payloads are stored in S3 under
digest-derived keys. Root keys come from `scripts/nextest_fingerprint.py`; BVS
keys come from the `backtester_cache` input-set digest. GitHub Actions cache is
still used for smaller registry and managed-target reuse, but the multi-GB
archive payloads do not write to the branch-scoped Actions cache.

## Controls

- `CI_NEXTEST_ARCHIVE_S3_ENABLED=true` enables S3 restore attempts.
- `CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX` selects the S3 prefix for archive objects.
- `CI_SCCACHE_BUCKET` and `CI_SCCACHE_REGION` select the shared S3 bucket for
  nextest artifact-cache restores and saves.
- `ci/sccache-location.toml` owns the expected shared sccache bucket, region,
  and `key_prefix`; the setup action reads that location directly and enables
  sccache only for allowed cache roles/events.
- `AWS_CI_CACHE_ROLE_ARN` is used only by `push` and `workflow_dispatch` runs
  on `refs/heads/main`.
- `AWS_CI_CACHE_PR_READONLY_ROLE_ARN` is the read-only cache consumer role. It
  is used for nextest-archive restores by PR, merge queue, and manual dispatch
  restore attempts, and for sccache read-only access by CI test-archive PR,
  merge queue, and manual dispatch builds, Rust Probe, Debug Test, and
  scheduled Flaky Test Smoke.

Tag pushes intentionally rebuild nextest archive payloads locally. They do not
receive an S3 cache role or `cache_mode`, so deploy-lane tag runs fail open to
the local archive/sidecar build path instead of restoring or saving S3 payloads.

Set `CI_NEXTEST_ARCHIVE_S3_ENABLED` to any value other than `true` to disable
the S3 backend. A disabled backend or failed restore is fail-open: CI builds the
archive and sidecars normally, and only the post-merge main writer may save new
objects. Managed-target cache saves and shared Cargo registry/Git rust-cache
saves are also limited to `push` runs on `refs/heads/main`; PR and merge-queue
runs restore from the default-branch cache namespace but do not write cache
entries.

Every S3 restore validates the object's `nextest-digest` metadata against the
current digest before the payload is used. A missing object or unavailable S3
read is a cache miss; a missing or mismatched `nextest-digest` metadata value
is an integrity failure and must fail the job. Delete the object or repopulate
it from a main push.

## Post-Merge Acceptance Evidence

Do not close the rollout issue from pre-merge checks alone. The PR or follow-up
issue must link these artifacts:

- A push-to-main run that saves the root and BVS S3 payloads.
- A later PR or merge-queue run that reports a restore HIT for each main-saved
  payload key.
- A manually dispatched `CI Storage Tripwire` run from `main` after the first
  main save.
- The first week-one metrics comparison for S3 egress bytes/run, restore time,
  save time, and Actions cache listed bytes.

## Week-One Metrics

Each S3 restore/save step writes byte counts and elapsed seconds to the job
summary. During the first week after rollout, compare:

- nextest archive S3 egress bytes per run
- root binary sidecars S3 egress bytes per run
- BVS nextest archive S3 egress bytes per run
- BVS binary sidecars S3 egress bytes per run
- restore seconds for each payload
- save seconds on push-to-main runs
- Actions cache listed bytes from `ci-storage-tripwire`

## Transition Cleanup

After the first post-merge `push` run saves the S3 payloads, manually dispatch
`CI Storage Tripwire` from `main`. The workflow measures Actions cache listed
bytes against the 10 GiB cache threshold and opens or updates the governed
tripwire issue when the store is still above the limit.

If the tripwire remains above 10 GiB, delete only retired GitHub Actions cache
entries whose keys start with the old payload prefixes:

- `nextest-archive-`
- `root-bin-sidecars-`
- `bvs-nextest-archive-`
- `bvs-bin-sidecars-`

Do not delete `managed-target-*` or `cargo-registry-*` entries during this
transition cleanup. Re-run `CI Storage Tripwire` afterward and use its cache
listed bytes as the convergence evidence.
