# #630 Supervised BTC Live Pilot — Runbook

Scope: one supervised live pilot of the binary-oracle taker on the BTC 5-minute up/down
family, closing the two gaps named in issue #630 (fill realism, model calibration). This
runbook is the pilot's documented procedure per the issue's acceptance criteria. It makes
no permanent strategy changes: the pilot runs the shipped strategy config plus
operator-local additions listed here.

Authority references (values live there, not here):
- strategy + caps: `config/strategies/binary_oracle_btc.toml` (`[parameters]`
  `order_notional_target`, `maximum_position_notional`; `[signal_data.primary]`;
  `[target.gate_subscriptions]`)
- root risk ceiling: `config/root.toml` `[risk] default_max_notional_per_order`
- kill switch: `src/bolt_v3_config.rs` (`RiskBlock.kill_switch` →
  `[risk.kill_switch]`: `state_path`, `forced_reduction_max_notional_per_order`,
  `authorized_operator_ids`), `src/bolt_v3_kill_switch_store.rs` (fail-closed recovery),
  `src/bolt_v3_submit_admission.rs` (rejects non-forced-reduction orders unless Armed)
- arming: `bolt-v2 provider-artifacts write-live-submit-approval` /
  `preflight-live-submit-arming` (see `--help` for flags; approval artifact carries an
  expiry — the supervised window's natural disarm)
- deploy mechanics: `deploy/README.md`
- root-cause being fixed by this redeploy: #630 diagnosis comment (version skew;
  deployed `build-7b9b2a8` predates the `[signal_data.primary]` loader split of
  7c3dedf25)

Gating: every box-mutating step (deploy, service start/stop, arming) is operator-present
and individually approved. Read-only probes (`ls`/`cat`/log greps via SSM) need no gate.

## Phase 0 — build + local preflight

- [ ] `git fetch && git log -1 origin/main` — record the SHA; must contain 7c3dedf25
      (signal/pricing role split). Build THIS sha: `just build` (aarch64 cross-build).
- [ ] `sha256sum` the binary; record for the deploy evidence file.
- [ ] `just live-check` and `just live-resolve` pass against the tracked production
      overlay `config/profiles/prod-btc-5m.overlay.toml` (the recipes compose the runtime
      config from the overlay + base `config/root.toml` first, then check secret
      completeness / SSM resolution).
- [ ] Confirm the overlay's `strategy_files` selection points at the BTC strategy
      from `config/strategies/binary_oracle_btc.toml` (its `strategy_instance_id` is
      `binary_oracle_btc`). Historical note: the Jun-6 smoke ran the OLD on-box
      config's id `bitcoin_updown_main` — expect the instance id (and the evidence
      directory name derived from it) to CHANGE after this redeploy.

## Phase 1 — deploy (operator present)

- [ ] Stop the service. Install binary per `deploy/README.md`, ship the overlay, the base
      `config/root.toml`, and `config/strategies/`, then generate + verify the runtime config
      from the tracked overlay on the box, using the same absolute paths the systemd unit runs:
      `/opt/bolt-v2/bolt-v2 ops generate-live-config --profile /opt/bolt-v2/config/profiles/prod-btc-5m.overlay.toml --output /opt/bolt-v2/config/live.toml`,
      then
      `/opt/bolt-v2/bolt-v2 ops verify-live-config --profile /opt/bolt-v2/config/profiles/prod-btc-5m.overlay.toml --deployed /opt/bolt-v2/config/live.toml`;
      config `/opt/bolt-v2/config/live.toml` root:bolt 0640.
- [ ] Record a `deploy/<date>-<shortsha>/deploy.txt` evidence entry (existing
      convention): binary sha256, config sha256, git SHA, operator id, date.

## Phase 2 — smoke gate with submission disabled (no approval artifact present)

Start the service WITHOUT a live-submit approval artifact; the node runs its readiness
probe with submission structurally disabled.

PASS requires, within the first minutes of decision evidence:
- [ ] `fair_probability_up=Some(...)` present in entry evaluations (the redeploy's whole
      point; Jun 3–6 runs were 0/137,157)
- [ ] `spot_price` populated and `fast_venue_available=true`
- [ ] ZERO `Failed to parse instruments item` / `instCategory` errors from
      `nautilus_okx` (two Jun-5 runs had thousands; if this recurs on the new build it
      is the blocking sub-item on #630 — NT adapter pin-bump or patch — and the pilot
      stops here)
- [ ] resolution gate fresh (Chainlink strike present; `valid_from==window` per the
      #553 live-strike proof pattern)

FAIL on any → stop the service, collect logs, file findings on #630. Do not proceed.

## Phase 3 — abort + kill-switch verification (before any order)

- [ ] Abort drill before any approval artifact exists: `systemctl stop bolt-v2`; confirm clean shutdown in
      journal and zero open orders/positions on the venue account.
- [ ] Configure `[risk.kill_switch]` in the operator-local config (`state_path` on the
      `/srv/bolt-v2` data volume; `forced_reduction_max_notional_per_order` at or below
      the strategy's per-order target; `authorized_operator_ids` = the supervising
      operator). Note: the shipped `config/root.toml` does not carry this block; adding
      it is an operator-local pilot addition, not a repo change.
- [ ] Fail-closed check: boot once with `state_path` pointing at a non-existent file —
      recovery must report fail-closed (`MissingEvidence`) and the admission gate must
      reject entries (`RejectedKillSwitchLatched`). This proves the latch path works
      without risking an order.
- [ ] Known gap: there is no operator CLI to seed or manually reset the store
      (`src/bolt_v3_kill_switch_store.rs` is the only writer). If re-arming for the
      pilot requires hand-writing store JSON, record that as a scoped sub-item on #630
      per the issue's "blockers found become scoped sub-items" rule, and decide with
      the operator whether to proceed on the artifact-expiry abort path alone.
- [ ] Primary pilot abort path (independent of the in-code kill switch): delete/let
      expire the live-submit approval artifact + `systemctl stop` + venue-side flatten.
      Rehearse the command sequence before arming.

## Phase 4 — armed supervised window

- [ ] Write the live-submit approval artifact with expiry = end of the supervised
      window (`bolt-v2 provider-artifacts write-live-submit-approval ...`), then
      `preflight-live-submit-arming` must PASS; record its JSON output as evidence.
- [ ] Caps stated in config and verified in logs before arming: per-order and position
      notional from `config/strategies/binary_oracle_btc.toml`, root ceiling from
      `config/root.toml`. Do not raise any cap for the pilot.
- [ ] Operator watches the full window. Per fired order, the decision evidence must
      capture quoted ask + displayed size at decision time; order events capture
      achieved fill price/size.
- [ ] Window ends at artifact expiry (natural disarm). Then `systemctl stop`.

## Phase 5 — evidence + report

- [ ] Collect from the box: decision-evidence logs and `order-intents.jsonl` under
      `/srv/bolt-v2/evidence/<instance>/`, journal for the window, config + binary
      hashes.
- [ ] Report (committed under `docs/research/`): per-fill expected-vs-achieved table
      (quoted ask vs achieved price; displayed size vs filled size), and live
      Brier/reliability of `fair_probability_up` against realized settlement.
- [ ] Post-run hygiene: leave the box with no valid approval artifact (submission
      disabled), service stopped or back in probe mode per operator choice.

## Acceptance mapping (issue #630)

| #630 criterion | Runbook phase |
|---|---|
| reference feed connected (`spot_price`/`fair_probability_up`, `fast_venue_available=true`) | Phase 2 |
| BTC-only scope; tiny notional cap stated in config, verified before arming | Phases 0, 4 |
| armed window supervised; kill-switch path verified before first order | Phases 3, 4 |
| expected-vs-achieved + live Brier/reliability report under docs/research/ | Phase 5 |
| pilot run config + runbook documented, no permanent strategy changes | this file |
