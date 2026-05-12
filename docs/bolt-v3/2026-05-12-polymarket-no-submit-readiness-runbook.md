# Polymarket No-Submit Readiness Runbook

Date: 2026-05-12
Scope: authenticated Polymarket execution readiness only; no order submit, cancel, amend, replace, fill, or canary.

## Preconditions

- Explicit operator approval exists for real SSM resolution and private Polymarket execution connect.
- Approved v3 root TOML exists outside source control and contains the intended `[clients.<id>]` Polymarket execution config and SSM paths.
- Report output path points to a private operator-controlled location.
- No provider credential environment variables are set; secrets resolve only through Rust SSM.

## Command

```bash
BOLT_V3_NO_SUBMIT_READINESS_CONFIG_PATH=/path/to/approved/root.toml \
BOLT_V3_NO_SUBMIT_READINESS_REPORT_PATH=/path/to/redacted-no-submit-readiness.json \
cargo test --test bolt_v3_no_submit_readiness \
  external_polymarket_no_submit_readiness_uses_real_ssm_and_writes_redacted_report \
  -- --ignored --exact --test-threads=1
```

## Expected Evidence

- JSON report written to `BOLT_V3_NO_SUBMIT_READINESS_REPORT_PATH`.
- Report has no resolved secret values.
- Report shows either:
  - satisfied connect and disconnect facts, or
  - concrete SSM, NT, or venue error facts.
- No strategy registration.
- No reference actor registration.
- No `LiveNode::run`.
- No submit/cancel/order API.
- No Python path.

## Stop Conditions

Stop immediately if output contains any resolved secret value, raw credential, wallet private key, API secret, submit/cancel evidence, fill evidence, or position mutation evidence.

## Not Accepted As Evidence

- Local mock-only pass.
- Python script output.
- Direct Polymarket REST/WebSocket probe outside NT.
- Any unredacted secret-bearing log or artifact.
- Any live order/cancel/amend/replace/fill.
