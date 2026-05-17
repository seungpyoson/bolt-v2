# Data Model: CI Cargo Cache Sharing

## SharedCargoRegistryGitCache

Fields:

- `shared_key`: `cargo-registry-git-v1`
- `cache_targets`: `false`
- `cache_bin`: `false`
- `save_if`: `github.job == 'test-archive' || startsWith(github.ref, 'refs/tags/v')`

Validation:

- Present in deny, clippy, check-aarch64, source-fence, test-archive, and build jobs.
- No `cache-directories` in rust-cache blocks.

## ManagedTargetCache

Fields:

- `path`: `${{ steps.setup.outputs.managed_target_dir }}`
- `key`: `managed-target-v1-${{ runner.os }}-${{ runner.arch }}-<lane>-...`
- `lane`: one of `clippy-host`, `check-aarch64-dev`, `source-fence-test`, `build-aarch64-release`

Validation:

- Cache key names the job/target/profile isolation boundary.
- Standalone check-aarch64 cache is guarded by `needs.detector.outputs.build_required != 'true'`.

## CacheEvidence

Fields:

- `run_id`
- `job_id`
- `head_sha`
- `cache_key`
- `restore_result`
- `restore_duration`
- `save_duration`

Validation:

- Evidence comes from exact PR-head GitHub Actions logs where available.
