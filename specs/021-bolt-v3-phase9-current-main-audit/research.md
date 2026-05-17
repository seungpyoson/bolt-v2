# Research: Bolt-v3 Phase 9 Comprehensive Audit

## Method

Current source of truth is main at `d6f55774c32b71a242dcf78b8292a7f9e537afab`. Stale PRs and local branches are reference-only until accepted.

Commands run in the Phase 9 worktree:

- `git status --short --branch`
- `git rev-parse HEAD main origin/main`
- `which no-mistakes`
- `no-mistakes --version`
- `no-mistakes daemon status`
- `cargo test --lib`
- `rg -n "std::process::Command|python|PyO3|pyo3|maturin|pip|aws ssm|aws-cli|op item|getenv|env::var|std::env::var|dotenv|1Password|private_key|api_secret|api_key" src tests docs specs config scripts Cargo.toml Cargo.lock`
- `rg -n "no_submit|readiness|live_canary|submit_admission|run_bolt_v3_live_node|LiveNode::run|node\\.run|connect_bolt_v3_clients|disconnect_bolt_v3_clients" src tests docs specs`
- `rg -n "submit_order|cancel_order|replace_order|amend_order|reconcile|OrderStatusReport|ExecutionMassStatus|register_external_order|cache\\(" src tests`
- `rg -n "\\[live_canary\\]|\\[persistence\\]|\\[persistence\\.decision_evidence\\]|\\[venues|\\[reference|\\[strategies|default_max_notional|pricing_kurtosis|theta_decay|edge_threshold|feed_id|exchange_refs" config tests/fixtures docs/bolt-v3 specs`
- `rg -n "source.*fence|verify_bolt_v3|runtime_literal|provider_leak|core_boundary|naming" tests scripts specs docs/bolt-v3`
- `rg -n "Phase 7|Phase 8|Phase 9|T037|tiny-capital|no-submit|stale|closed|PR #318|PR #319|PR #320" specs docs`
- `rg -n "runbook|rollback|on-call|incident response|alert" docs/bolt-v3 docs/postmortems config/live.local.example.toml`
- debt-marker scan over source, tests, docs, specs, config, scripts, and cargo metadata

## Decisions

### Decision 1: Phase 9 Is Blocked From Final Certification

Phase 9 can audit current main, but cannot certify post-Phase7/8 readiness because Phase 7 and Phase 8 are not accepted on main. Current main still shows Phase 7 no-submit readiness tasks open in `specs/001-thin-live-canary-path/tasks.md:87-92` and Phase 8 tasks open at `specs/001-thin-live-canary-path/tasks.md:94-106`.

### Decision 2: No Cleanup In This Planning Commit

Cleanup is not performed in this planning slice. The scan found candidate stale docs, fixture literals, and verifier gaps, but cleanup needs behavior tests and review so it cannot be bundled into audit artifact creation.

### Decision 3: Live Action Is Blocked

The fresh worktree has no `config/live.local.toml`. The tracked example identifies itself as a legacy live-config render input, not a bolt-v3 root TOML, and says a bolt-v3 root TOML must include `[live_canary]` (`config/live.local.example.toml:8-17`). The ETH strategy template is commented and warns not to uncomment without matching ETH ruleset and reference venues (`config/live.local.example.toml:132-155`).

### Decision 4: Submit Ordering Exists But Does Not Prove Live Readiness

Main has a Phase 6 submit ordering path: `src/strategies/eth_chainlink_taker.rs:2827-2838` records decision evidence, admits through submit admission, then calls NT `submit_order`. This is necessary but not sufficient for Phase 8 because NT submit/accept/reject/fill/cancel/restart evidence is not yet accepted.

### Decision 5: Runtime Pure-Rust Claim Needs Scope Precision

The scan found Python scripts under `scripts/` for verification tooling and no PyO3/maturin references in runtime paths. This supports "no Python runtime layer" only with scope precision: Python exists as verification tooling, not as the live binary runtime. Existing source-grounded status map still lists a missing dedicated verifier at `docs/bolt-v3/2026-04-28-source-grounded-status-map.md:65`.

### Decision 6: Provider Boundary Remains Partial

Status-map rows 14-16 mark provider-specific adapter mapping, secret handling, and client factory registration as partial or wrong-placement (`docs/bolt-v3/2026-04-28-source-grounded-status-map.md:76-78`). Doctrine also records missing verifier coverage for provider-specific adapter, secret, and client registration leaks (`docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md:404-431`).

### Decision 7: Live Ops Readiness Is Not Source-Closed

Search for runbook, rollback, on-call, incident response, and alert evidence found a reconnect alert threshold in example config and archived rollback mention only. The root-volume postmortem recommends data volume, fixed service working directory, capped logs, and corrected audit S3 destination (`docs/postmortems/2026-04-20-root-volume-incident.md:313-332`), but that is incident history, not an accepted current live-ops runbook.

## Alternatives Rejected

- Continue from PR #318, #319, or #320: rejected because old branches are closed stale/superseded and current main is authoritative.
- Perform code cleanup before review: rejected because behavior tests and external review are not yet in place.
- Treat submit admission as live readiness: rejected because Phase 6 ordering does not prove real no-submit readiness, strategy input safety, NT venue acceptance, fill/cancel, restart, or ops readiness.
