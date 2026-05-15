# CI Parallel Heavy Lanes Research

## Decision: Stack #332 on the current #203 branch

**Rationale**: #333 sequences #332 after #342 and #203 lint coverage. The worktree is based on `origin/codex/ci-203-workflow-hygiene@85e6fb722adddaf28e6cda122154b60be973ba09`, so `source-fence`, managed target-dir setup, and the standard-library workflow verifier are active prerequisites.

**Alternatives considered**:
- Base directly on `main`: rejected because it would ignore the already-open #342 and #203 topology that #332 must coordinate with.
- Base only on #342: rejected because #203 has already introduced the verifier surface #332 should extend.

## Decision: Use `count:` partitioning as requested by #332

**Rationale**: The #332 body explicitly requires `just test -- --partition count:${{ matrix.shard }}/4`. The nextest documentation says counted partitioning is supported but recommends `slice:` because counted partitioning is less even and stable than sliced partitioning. That is a possible future improvement, but changing to `slice:` here would narrow or reinterpret the issue without a strong reason.

**References**:
- Nextest partitioning docs: https://nexte.st/docs/ci-features/partitioning/

**Alternatives considered**:
- Use `slice:${{ matrix.shard }}/4`: rejected for this issue because the body requires `count:` and there is no evidence that `count:` is unsafe or unsupported.
- Use dynamic shard count: rejected because #332 names four shards and #195 needs bounded cache-key shape.

## Decision: Set test matrix `fail-fast: false`

**Rationale**: GitHub Actions matrix fail-fast defaults to true and can cancel queued or running siblings when one matrix job fails. #332 requires fail-closed behavior if any shard fails, is cancelled, or is skipped unexpectedly, and it requires actionable shard labels. Keeping all shards observable is more useful than fail-fast cancellation for this CI topology change.

**References**:
- GitHub Actions matrix fail-fast docs: https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstrategyfail-fast

**Alternatives considered**:
- Keep default fail-fast: rejected because sibling cancellation would become an expected outcome after one failure, reducing shard evidence.
- Add four separate non-matrix jobs: rejected because #332 explicitly asks for `strategy.matrix.shard: [1,2,3,4]`.

## Decision: Preserve the managed `just test` path with passthrough args

**Rationale**: Repo rules require CI and local Rust execution to go through managed recipes. The Rust verification owner already supports extra args for recipe-delegated commands, so the justfile can add variadic recipe args and pass them through without adding a workflow raw cargo path.

**Alternatives considered**:
- Run `cargo nextest` directly in workflow YAML: rejected by repo rules and existing lint.
- Add a second `test-shard` recipe: rejected because #332 asks to preserve the existing managed-recipe contract and `just test` passthrough.

## Decision: Intentionally duplicate #342 source-fence filters inside full nextest

**Rationale**: #332 allows duplicate execution if it is explicitly documented and covered by one required aggregate gate. Excluding source-fence filters from full nextest would require a filter expression that precisely excludes only the canonical source-fence tests without accidentally removing neighboring integration-test coverage. The safer root choice for this slice is to keep full nextest broad, keep `test` behind `source-fence`, keep both required by `gate`, and document the intentional duplication.

**Alternatives considered**:
- Exclude the canonical filters from full nextest now: rejected because the marginal speedup is small relative to the risk of an imprecise filter removing non-source-fence coverage.
- Remove `source-fence` dependency from `test`: rejected because #342 exists to fail deterministic source-fence drift before full nextest.

## Decision: Use bounded shard-aware cache keys

**Rationale**: #332 asks for independent rust-cache keys and coordination with #195 so keys remain shard-aware but not unbounded. Host clippy and aarch64 get separate fixed keys. Test shards use a fixed `nextest-v3-shard-${{ matrix.shard }}-of-4` dimension so #195 can later tune artifact reuse without inheriting unbounded commit-specific keys.

**Alternatives considered**:
- Keep one `nextest-v2` cache key for all shards: rejected because it makes shard cache behavior harder to reason about for #195.
- Include `github.sha` in the key: rejected because it is unbounded and would defeat warm rerun reuse.

## Decision: Extend #203 verifier only for #332 topology

**Rationale**: The #332 issue comment says this issue owns lane parallelization and only the lint allow-list extension for the specific new lanes. Generic workflow lint hygiene remains #203. The correct extension is check-aarch64 and test-shard topology, not unrelated cleanup.

**Alternatives considered**:
- Rewrite the entire workflow linter now: rejected as generic #203 work.
- Skip lint changes: rejected because #332 explicitly requires topology lint updates.
