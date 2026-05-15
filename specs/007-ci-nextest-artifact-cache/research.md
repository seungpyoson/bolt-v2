# Research: CI Nextest Artifact Cache

## Decision: Use rust-cache workspace target mapping for `test`

**Decision**: Change the sharded `test` rust-cache step from opaque `cache-directories: ${{ steps.setup.outputs.managed_target_dir }}` to a rust-cache `workspaces` entry that maps `.` to the managed target directory.

**Rationale**: rust-cache documents `workspaces` as the cargo workspace and target-directory configuration, and its source constructs target paths by joining the workspace root with the configured target. Using the workspace path gives rust-cache cleanup logic a real Cargo workspace/target pair instead of an extra unmanaged directory.

**Source**: https://github.com/Swatinem/rust-cache/tree/v2.9.1 and `src/config.ts` in that tag.

**Alternatives rejected**:
- Keep `cache-directories` only: rejected because #195 specifically identifies opaque managed-target caching and workspace artifact cleanup as the residual risk.
- Add direct `actions/cache`: rejected for this slice because it creates a second cache backend/path unless rust-cache proves insufficient.

## Decision: Add relative managed target-dir output

**Decision**: Extend `.github/actions/setup-environment/action.yml` with `managed_target_dir_relative`, computed from `GITHUB_WORKSPACE` to the managed target directory.

**Rationale**: rust-cache `workspaces` target paths are interpreted relative to the workspace root. The managed Rust owner currently exposes an absolute target directory, so the setup action must expose the relative path for rust-cache workspace mapping while preserving the existing absolute output for other jobs.

**Source**: rust-cache v2.9.1 source joins `root` and `target` when constructing workspace cache paths.

**Alternatives rejected**:
- Pass the absolute path directly to `workspaces`: rejected because Node `path.join(root, absolute-looking-target)` would not preserve the intended absolute target path in this context.
- Replace the absolute output: rejected because existing `clippy`, `check-aarch64`, `source-fence`, and `build` cache steps already consume it.

## Decision: Enable workspace crate artifact preservation for test

**Decision**: Set `cache-workspace-crates: "true"` on the sharded `test` rust-cache step.

**Rationale**: rust-cache defaults `cache-workspace-crates` to false and documents that workspace crates are generally not cached. Its save path cleans target directories using only packages outside the workspace unless `cache-workspace-crates` is true. #195's core requirement is to preserve workspace `bolt-v2` test-profile artifacts for warm nextest reruns.

**Source**: https://raw.githubusercontent.com/Swatinem/rust-cache/v2.9.1/README.md and `src/save.ts`.

**Alternatives rejected**:
- `cache-all-crates: "true"`: broader than required and affects dependency registry cleanup too.
- Cache only `target/nextest`: insufficient because the observed rebuild is workspace test-profile compilation, not only nextest run metadata.

## Decision: Keep rust-environment hashing explicit

**Decision**: Set `add-rust-environment-hash-key: "true"` explicitly for the `test` cache and verify it stays enabled.

**Rationale**: rust-cache keys can include rustc version/host/hash, compiler-related environment variables, Cargo manifests/lockfile, rust-toolchain files, and cargo config. #195 requires invalidation by real Rust inputs; explicit YAML plus verifier tests prevent a future cleanup from disabling that behavior.

**Source**: rust-cache README and `src/config.ts`.

**Alternatives rejected**:
- Rely only on rust-cache default: functional but less reviewable; #195 asks for deterministic cache-key evidence.
- Add `github.sha`: rejected because it creates unbounded per-commit cache namespaces and conflicts with warm rerun reuse.

## Decision: Keep per-shard bounded key dimension

**Decision**: Preserve the #332 key shape's bounded shard dimension, `nextest-v3-shard-${{ matrix.shard }}-of-4`, while relying on rust-cache's automatic job/rust-environment/hash suffixes for real inputs.

**Rationale**: #195 must adapt to #332 shards but avoid unbounded cache growth. The matrix shard has four bounded values, while rust-cache hashes the real Rust inputs.

**Source**: #332 issue body and #195 requirements; rust-cache README key documentation.

**Alternatives rejected**:
- One shared cache for all shards: risks cross-shard save contention and larger cache archives.
- Per-commit keys: likely thrash and weak warm reuse.

## Decision: Evidence remains blocked until exact CI runs

**Decision**: Local verification can prove workflow shape, but #195 completion remains blocked until exact cold/warm GitHub Actions run IDs, logs, timings, and cache-size evidence are recorded.

**Rationale**: #195 acceptance explicitly requires exact GitHub Actions run IDs and log excerpts. The current stacked PR base does not receive full `pull_request` CI because the workflow trigger targets `main`.

**Source**: #195 issue body and current PR #348 stacked CI behavior.

**Alternatives rejected**:
- Infer success from YAML: rejected by the issue body.
- Treat local tests as CI evidence: rejected because cache archive size/pruning/warm rebuild behavior is GitHub Actions-specific.

## Primary Source Notes

- rust-cache v2.9.1 README: `workspaces`, `cache-targets`, `cache-workspace-crates`, `add-rust-environment-hash-key`, cleanup behavior, and cache limits.
- rust-cache v2.9.1 source: `src/config.ts`, `src/save.ts`, `src/workspace.ts`.
- cargo-nextest partitioning docs: `--partition` selects buckets after filters and logs `Finished test profile`.
- cargo-nextest archiving docs: build once/run elsewhere is a different design and not needed for same-run warm cache.
- GitHub dependency cache docs: cache entries expire by access age and repository storage limits can cause eviction/cache thrash.
