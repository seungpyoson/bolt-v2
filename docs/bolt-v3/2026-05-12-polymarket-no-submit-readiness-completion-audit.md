# Polymarket No-Submit Readiness Completion Audit

Date: 2026-05-12
Branch: `codex/bolt-v3-polymarket-no-submit-readiness`

This audit checks only the no-submit authenticated readiness slice. It is not live-trading approval.

## Objective

Build a committed bolt-v3 harness that can, after separate operator approval, load v3 TOML, resolve SSM secrets through Rust SSM, build a client-only NT `LiveNode`, connect authenticated Polymarket execution infrastructure, disconnect cleanly, and write a redacted readiness report without strategy registration, actor registration, `LiveNode::run`, Python, or order APIs.

## Checklist

| Requirement | Evidence | Status |
| --- | --- | --- |
| Same v3 TOML/config path | `tests/bolt_v3_no_submit_readiness.rs::external_polymarket_no_submit_readiness_uses_real_ssm_and_writes_redacted_report` loads operator-provided root TOML through `load_bolt_v3_config` | met-local |
| SSM-only secret path | `src/bolt_v3_no_submit_readiness.rs::run_bolt_v3_no_submit_readiness` uses `check_no_forbidden_credential_env_vars`, `SsmResolverSession::new`, and `resolve_bolt_v3_secrets` | met-local |
| No injected fake resolver in production runner | `tests/bolt_v3_no_submit_readiness.rs::no_submit_readiness_public_runner_uses_real_ssm_boundary` source-fences the public runner from `resolve_bolt_v3_secrets_with`, fake resolver, and env-secret fallback helpers | met-local |
| Client-only NT build | `src/bolt_v3_no_submit_readiness.rs` builds through `make_bolt_v3_client_registered_live_node_builder` and `build_bolt_v3_live_node_from_registered_builder` after v3 adapter mapping | met-local |
| No strategy registration | `tests/bolt_v3_no_submit_readiness.rs::no_submit_readiness_builds_client_only_idle_node_without_strategy_registration` asserts NT trader strategy count is zero; source guard rejects `register_bolt_v3_strategies` | met-local |
| No reference actor registration | Source guard rejects `register_bolt_v3_reference_actors` and `register_actor` in `src/bolt_v3_no_submit_readiness.rs` | met-local |
| No `LiveNode::run` or start/run strategy path | Source guard rejects `.run(`, `.start(`, `start_async`, `kernel.start`, and `start_trader` | met-local |
| No order APIs | Source guard rejects submit, modify, cancel, order builder, Polymarket order builder, order submitter, and market data subscription APIs | met-local |
| No Python path | Source guard rejects `python`, `PyO3`, `maturin`, and `Command::new` in no-submit readiness source | met-local |
| Missing SSM fails before connect | `tests/bolt_v3_no_submit_readiness.rs::no_submit_readiness_missing_secret_stops_before_mapping_build_and_connect` proves secret failure skips mapping, builder, build, connect, and disconnect | met-local |
| Resolved secret redaction | `tests/bolt_v3_no_submit_readiness.rs::no_submit_readiness_adapter_mapping_failure_redacts_resolved_secrets` and the ignored external harness assert report output does not contain resolved fixture secret values | met-local |
| Operator runbook exists | `docs/bolt-v3/2026-05-12-polymarket-no-submit-readiness-runbook.md` gives the exact ignored-test command and stop conditions | met-local |
| Runtime-literal classification | `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` classifies the new readiness detail string | met-local |
| Real authenticated Polymarket connect artifact | Not executed; requires separate approval for real SSM/private execution connect | blocked |
| Redacted external readiness JSON artifact | Not produced; depends on approved external run | blocked |
| Live submit/cancel/fill/canary | Out of scope and still forbidden | not accepted |

## Verification

- `cargo test --test bolt_v3_no_submit_readiness -- --test-threads=1`: passed 5, ignored 1.
- `just fmt-check`: passed.
- `git diff --check`: passed.

## Decision

The local no-submit readiness harness is complete. The production-readiness claim is not complete until an approved external run produces a redacted report showing either successful connect/disconnect facts or a concrete NT/venue error.
