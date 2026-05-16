# Research: CI Baseline Measurement

## Decision: Use GitHub Actions run metadata as primary timing source

**Rationale**: `gh run view --json jobs` provides exact run IDs, event type, SHA, job names, job start/completion timestamps, statuses, conclusions, and URLs. This satisfies #343 without modifying CI.

**Alternatives considered**: Issue body numbers alone were rejected because they can become stale. Workflow YAML inspection alone was rejected because it cannot prove actual wall time.

## Decision: Use raw active runner minutes plus rounded-per-job estimate

**Rationale**: #343 asks for billed runner-minute estimates from job durations. Raw active runner minutes show total runner work. Rounded-per-job estimates make short jobs visible because hosted runner billing often rounds job duration.

**Alternatives considered**: Workflow elapsed time alone was rejected because parallel jobs hide billed runner consumption. Account-level billing export was not used because this task is scoped to exact run evidence available from Actions.

## Decision: Classify cache warmth only from logs

**Rationale**: Cache hit, restored key, and restored archive size are visible in `Swatinem/rust-cache` logs. Those are concrete enough to call a run warm-cache. Runs without log evidence stay unknown.

**Alternatives considered**: Inferring warmth from short wall time was rejected because #343 explicitly says not to infer warmth without log evidence.

## Decision: Include multiple run shapes

**Rationale**: #333 spans PR critical path, main push, tag deploy, path filters, and source-fence failures. One run cannot represent all of those. The baseline includes:

- PR without build lane: #332 bottleneck evidence.
- PR with build lane: current source/build-affecting PR shape.
- Failing PR source-fence example: #342 late-failure evidence.
- Main push: post-merge path.
- Same-SHA main/tag pair: #205 duplicate deploy path evidence.

**Alternatives considered**: Only using the newest successful run was rejected because it would hide issue-specific bottlenecks and contradict the user's instruction not to narrow requirements.

## Decision: Record live-source scope conflicts explicitly

**Rationale**: The #333/#335 live text says drift-detection lint was stripped from PR #339, while #344 still says that PR #339 shipped drift-detection lint. Because that affects residual #344 scope, the baseline records the conflict and follows the #333/#335 accepted-scope text for current interpretation.

**Alternatives considered**: Silently following only #344 was rejected because it contradicts #333/#335. Silently following only #333/#335 was rejected because it would hide a live issue-body mismatch that future #344 implementation must resolve.
