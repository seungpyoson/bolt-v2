# Root Artifact Review Fixes Design

## Scope

Address the two internal adversarial-review blockers on PR #1448 without changing the broader root-artifact architecture or the shared behavior expected by other workflows.

## Cache-stat isolation

The `root-artifact` producer will establish its own mandatory cache-stat baseline after the shared sccache setup action and before Cargo starts. The workflow will require `sccache --zero-stats` to succeed, read fresh JSON stats, and reject any nonzero compile-request, hit, or miss count. This keeps the root-artifact evidence fail closed even though the reusable action permits cache degradation for other consumers.

The final manifest will continue to use the post-build sccache JSON, but its counters will then be attributable to the current producer run.

## Mutation evidence

Add a focused Python policy test for `.github/workflows/root-artifact.yml`. The test will load the real workflow, prove the accepted form passes, then mutate isolated copies and require rejection for every issue-owned invariant:

- automatic triggers;
- a second producer or Cargo build;
- hidden Cargo test or nextest execution;
- missing mandatory wrapper or zero-stat baseline;
- retries or result fallback;
- omitted overlay handling;
- weakened byte or digest checks;
- artifact upload, installer, launch, readiness, merge, deploy, or trading authority consumers.

The policy checker will inspect parsed workflow structure and the governed shell steps rather than depend on line numbers. The mutation test will exercise the checker through temporary files so each mutation demonstrates a real failing gate.

## Verification

Implementation follows red-green-refactor:

1. Add the focused mutation tests and confirm they fail against the current workflow/policy support.
2. Add the minimal checker and fail-closed workflow baseline until the focused tests pass.
3. Run formatting/static checks applicable to Python and YAML.
4. Run the repository's public workflow hygiene and source-fence static gates. No local compile-heavy Rust commands will be used.

Runtime ARM64 execution and post-landing concurrency/ephemeral-disk evidence remain exact-current-`main` follow-up evidence because the workflow cannot dispatch from the PR branch.

## Non-goals

- Do not change the shared sccache action's degradation policy.
- Do not add a second build, test lane, artifact consumer, installer, or authority path.
- Do not treat the root artifact as deploy, readiness, merge, or trading permission.
