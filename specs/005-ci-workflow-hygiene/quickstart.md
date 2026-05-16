# CI Workflow Hygiene Quickstart

## Local Red/Green Checks

From the #203 worktree:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
```

Expected result after implementation: all commands pass.

## Workflow Sanity Checks

```bash
rg -n "fmt-check:|needs:|include-managed-target-dir|deploy:|source-fence|needs\\.(detector|fmt-check|deny|clippy|source-fence|test|build)\\.result" .github/workflows/ci.yml
rg -n "live-node|max-threads|bolt_v3_adapter_mapping|bolt_v3_client_registration|bolt_v3_controlled_connect|bolt_v3_credential_log_suppression|bolt_v3_live_canary_gate|bolt_v3_readiness|bolt_v3_strategy_registration|bolt_v3_submit_admission|bolt_v3_tiny_canary_operator|config_parsing|eth_chainlink_taker_runtime|lake_batch|live_node_run|nt_runtime_capture|platform_runtime|polymarket_bootstrap|venue_contract" .config/nextest.toml
```

Expected evidence:

- `fmt-check` has no `needs: detector`.
- `build` still has `needs: detector` and `if: needs.detector.outputs.build_required == 'true'`.
- Jobs using `steps.setup.outputs.managed_target_dir` set `include-managed-target-dir: "true"`.
- `deploy.needs` includes all required safety lanes directly.
- `gate` checks all required lane results.
- `.config/nextest.toml` assigns the full LiveNode binary set to the `live-node` group with `max-threads = 1`:
  `bolt_v3_adapter_mapping`, `bolt_v3_client_registration`, `bolt_v3_controlled_connect`, `bolt_v3_credential_log_suppression`, `bolt_v3_live_canary_gate`, `bolt_v3_readiness`, `bolt_v3_strategy_registration`, `bolt_v3_submit_admission`, `bolt_v3_tiny_canary_operator`, `config_parsing`, `eth_chainlink_taker_runtime`, `lake_batch`, `live_node_run`, `nt_runtime_capture`, `platform_runtime`, `polymarket_bootstrap`, and `venue_contract`.

## Verification Gate

```bash
just fmt-check
just ci-lint-workflow
git diff --check
```

Exact-head CI must pass before external review:

- `detector`
- `fmt-check`
- `deny`
- `clippy`
- `source-fence`
- `test`
- `build`
- `gate`
