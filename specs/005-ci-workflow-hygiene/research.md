# CI Workflow Hygiene Research

## Decision: Stack #203 on #342

**Rationale**: Issue #203 says if #342 lands first, workflow lint must validate the new fast source-fence lane by exact job name and gate dependency. The worktree is based on `origin/codex/ci-342-source-fence@b6703a733c1f7a988aa916f71b8e7fb5a7daa54f`, so source-fence is an active invariant.

**Alternatives considered**:
- Base on `main`: rejected because it would ignore already-open #342 topology and force later rework.
- Base on #343 only: rejected because #342 has already produced the topology #203 must lint.

## Decision: Add a dedicated standard-library verifier

**Rationale**: The current `just ci-lint-workflow` has useful awk checks, but #203 asks for exact job existence and actionable future topology guardrails. A small Python verifier can self-test YAML-shape parsing without adding PyYAML or unpinned dependencies.

**Alternatives considered**:
- Extend the existing awk blocks only: rejected because self-testing fixture mutations and exact topology coverage are harder to keep readable.
- Add PyYAML: rejected because repo rules forbid unnecessary dependencies and #342 already removed ambient PyYAML reliance.

## Decision: Remove only `fmt-check` detector serialization

**Rationale**: #203 explicitly names `fmt-check needs: detector`. `fmt-check` does not consume detector output. Removing that edge gives small wall-time improvement without weakening gate semantics because `gate` still requires detector success.

**Alternatives considered**:
- Remove detector from every non-build job: rejected for this slice because #342 currently requires source-fence to depend on detector, and broader topology decomposition belongs to #332.
- Keep the edge: rejected because it leaves a named #203 item unaddressed without a strong reason.

## Decision: Make managed target-dir resolution opt-in

**Rationale**: `fmt-check` and `deny` do not use the managed target cache output. Clippy, source-fence, test, and build do. An opt-in input keeps the shared setup action while trimming unused work where safe.

**Alternatives considered**:
- Split the composite action into multiple actions: rejected as unnecessary architecture for one target-dir branch.
- Remove the managed owner from fmt-check: rejected because `just fmt-check` still invokes managed Rust formatting.

## Decision: Add direct deploy needs as defense-in-depth

**Rationale**: Deploy currently depends on `gate` and `build`. Gate is sufficient transitively, but #203 asks whether direct needs should be restored. Direct needs make deploy's safety surface visible at the job boundary while retaining gate as the single aggregate signal.

**Alternatives considered**:
- Keep only transitive gate needs: rejected because #203 specifically asks to re-evaluate direct edges and defense-in-depth is cheap here.
- Remove gate and rely only on direct needs: rejected because #333 requires one required aggregate signal.

## Decision: Do not invent future topology lint

**Rationale**: #332 sharding, #205 same-SHA reuse, and #344 pass-stub workflows are not present in the stacked base. Enforcing them now would narrow or distort those issues. This slice records those conditions but does not implement them.

**Alternatives considered**:
- Pre-build lint for hypothetical shards or reuse paths: rejected because it would create false requirements before the workflow surfaces exist.

## Decision: Enforce prebuilt CI Rust helper-tool installs

**Rationale**: #250 priority item 1 removes repeated CI source builds for `cargo-deny`, `cargo-nextest`, and `cargo-zigbuild`. `cargo-deny` and `cargo-nextest` are available in pinned `taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c`, so those jobs use the action with `fallback: none` and versions from setup outputs. `cargo-zigbuild` `0.22.1` is installed from the upstream Linux x86_64 release archive, with the expected SHA256 pinned in the justfile and exported by setup. The verifier blocks source-build regressions, action fallback, same-origin zigbuild checksum use, and incomplete manual install steps.

**Alternatives considered**:
- Keep `cargo install --locked`: rejected because it preserves the recurring tool compile time called out by #250.
- Use `taiki-e/install-action` for `cargo-zigbuild`: rejected because the pinned `0.22.1` archive path and manifest availability did not match the needed release asset.
- Download the `.sha256` file from the same `cargo-zigbuild` release during CI: rejected after review because it makes the release asset set its own trust anchor.
