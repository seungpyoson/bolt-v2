# Research: #205 Same-SHA Smoke-Tag Dedup

## Decision: Resolve source evidence from GitHub Actions metadata

**Rationale**: #205 requires exact SHA, trusted `main` run, required lanes, and fail-closed behavior. The GitHub Actions workflow runs API supports filtering runs by event, branch, head SHA, and status. The resolver then validates workflow path/name, event, branch, SHA, status, and conclusion before checking jobs and artifacts.

**Rejected alternatives**:

- Trust the latest `main` branch state: rejected because issue #205 requires exact tag SHA, not branch inference.
- Trust a check summary only: rejected because deploy also needs a concrete source artifact ID.
- Rebuild on missing evidence: rejected because that recreates the duplicate work and violates fail-closed acceptance.

## Decision: Require current topology jobs in the source run

**Rationale**: Epic #333 says reused evidence must include all required lanes that exist when this lands. In this stacked topology that includes `detector`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, four `test` shards, `build`, and `gate`.

**Rejected alternatives**:

- Reuse historical #205 proof runs directly: rejected because those runs predate the current four-shard and `check-aarch64` topology.
- Accept only aggregate gate success: rejected because #205 asks reused evidence to include tests, build/artifact, and structural verifier/gate lanes.

## Decision: Download by artifact ID from the source run

**Rationale**: `actions/download-artifact` supports `artifact-ids`, `github-token`, `repository`, and `run-id`. Downloading by ID from the validated source run binds deploy to the exact artifact produced by the trusted `main` run.

**Rejected alternatives**:

- Download by artifact name from the current run: rejected because tag runs no longer build/upload the artifact.
- Download by artifact name only from the source run: rejected because artifact ID is the immutable exact evidence handle.

## Decision: Gate tag runs by expected skips instead of fallback lanes

**Rationale**: The tag workflow must prove it avoided duplicate heavy work. In tag mode, `gate` requires evidence success and requires duplicate heavy lanes to be skipped. In non-tag mode, `gate` requires evidence skipped and preserves normal lane success checks.

**Rejected alternatives**:

- Let tag runs rerun heavy lanes when evidence is missing: rejected by fail-closed acceptance.
- Remove direct deploy needs and trust only gate: rejected because #203 established direct deploy needs as defense-in-depth.

## Sources

- GitHub REST workflow runs API: https://docs.github.com/en/rest/actions/workflow-runs
- GitHub REST workflow jobs API: https://docs.github.com/en/rest/actions/workflow-jobs
- GitHub REST workflow artifacts API: https://docs.github.com/en/rest/actions/artifacts
- `actions/download-artifact` inputs: https://github.com/actions/download-artifact/blob/v5/action.yml
- `actions/download-artifact` cross-run docs: https://github.com/actions/download-artifact/blob/v5/README.md
