# Research: CI Cargo Cache Sharing

## Decision: Use `shared-key` for registry/git-only rust-cache

**Rationale**: Swatinem/rust-cache v2.9.1 constructs the key from `shared-key` instead of the automatic job key. With `cache-targets: false`, cache paths are Cargo registry/git plus optional bin and any `cache-directories`.

**Alternatives considered**:

- Keep per-job `key`: rejected because it fragments registry/git cache payloads across jobs.
- Use only `actions/cache` for Cargo registry/git: rejected because rust-cache already handles Cargo dependency cleanup and lockfile/environment keying.

## Decision: Disable cargo-bin caching in shared registry/git cache

**Rationale**: #366 scope is Cargo registry/git sharing. Tool binaries are installed by pinned prebuilt actions or scripts and should not be part of the shared payload.

## Decision: Use pinned `actions/cache` for managed target dirs

**Rationale**: rust-cache always includes Cargo registry/git in its payload and `cache-directories` adds target dirs to that same payload. Splitting target dirs into `actions/cache` keeps target artifacts isolated by job/target/profile while the rust-cache payload stays registry/git-only.

**Alternatives considered**:

- Remove target-dir caches: rejected because it risks slowing clippy/source-fence/build lanes.
- Keep target dirs in rust-cache: rejected because it keeps duplicating registry/git payloads per job.

## Decision: Single save owner for shared registry/git cache

**Rationale**: All jobs can restore the shared key, but only `test-archive` should save on normal PR/main CI. Tag-only runs skip test-archive, so `build` is allowed to save there.
