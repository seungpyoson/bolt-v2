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
- `CI_SCCACHE_BUCKET` and `CI_SCCACHE_REGION` select the shared CI cache bucket.
- `AWS_CI_CACHE_ROLE_ARN` is used only by `push` runs on `refs/heads/main`.
- `AWS_CI_CACHE_PR_READONLY_ROLE_ARN` is used by PR, merge queue, and manual
  dispatch restore attempts.

Set `CI_NEXTEST_ARCHIVE_S3_ENABLED` to any value other than `true` to disable
the S3 backend. A disabled backend or failed restore is fail-open: CI builds the
archive and sidecars normally, and only the post-merge main writer may save new
objects. Managed-target cache saves are also limited to `push` runs on
`refs/heads/main`; PR and merge-queue runs restore from the default-branch cache
namespace but do not write cache entries.

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
