# Quickstart: Global Shadow Execution Policy

## Pre-Implementation Gate

Implementation is blocked until all reviews approve:

```text
internal self-review: approve
Gemini adversarial review: approve
Grok adversarial review: approve
Claude adversarial review: approve
```

## Focused Local Non-Compile Gates

Use these before commit and before remote verification:

```bash
git diff --check
just fmt-check
python3 scripts/test_verify_bolt_v3_schema_current.py
python3 scripts/verify_bolt_v3_schema_current.py
python3 scripts/test_verify_bolt_v3_runtime_literals.py
python3 scripts/verify_bolt_v3_runtime_literals.py
just source-fence-static
```

## Remote Compile/Test Proof

After implementation is committed and pushed:

```bash
just verify-remote
```

Use exact-head PR CI as the proof for compile, clippy, and Rust tests. Do not substitute stale branch or local compile-heavy output for completion evidence.

Exact-head CI evidence is intentionally not pinned in this committed file: any commit that edits the evidence changes the head SHA. Record the fresh
`just verify-remote` result in the PR or review handoff after the final push instead.

## Review Evidence

The review packet is:

```text
specs/621-global-shadow-execution-policy/spec.md
specs/621-global-shadow-execution-policy/plan.md
specs/621-global-shadow-execution-policy/research.md
specs/621-global-shadow-execution-policy/data-model.md
specs/621-global-shadow-execution-policy/contracts/order-execution-policy.md
specs/621-global-shadow-execution-policy/checklists/requirements.md
specs/621-global-shadow-execution-policy/tasks.md
```
