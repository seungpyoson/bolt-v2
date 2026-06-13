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

Latest recorded evidence:

```text
PR: https://github.com/seungpyoson/bolt-v2/pull/695
HEAD: 1a1662cee29015856e379d8dbc7f62f67832ec05
CI: https://github.com/seungpyoson/bolt-v2/actions/runs/27481811305
Result: completed/success
```

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
