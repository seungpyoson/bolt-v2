# Phase 8 Strategy-input Safety Audit

## Recommendation

**BLOCK Phase 8 live-capital action.**

Local implementation readiness may proceed for fail-closed preflight, dry/no-submit evidence, and ignored harness shape. Actual live order must not proceed until every blocker below is resolved with source-backed evidence and user approval naming exact head and command.

## Evidence

- Current main lacks Phase 7 no-submit readiness producer; only Phase 6 live canary gate and submit admission are present.
- `config/live.local.toml` is gitignored and absent from the fresh Phase 8 worktree; tracked config is example/snapshot only.
- `config/live.local.example.toml:108-109` shows a commented Chainlink venue named `CHAINLINK-BTC` with one feed id.
- `config/live.local.example.toml:133-155` warns not to uncomment `eth_chainlink_taker` unless the example ruleset and reference venues are switched to matching ETH/Chainlink operator config.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:635-636` states Polymarket fees are not proven by a pre-entry Bolt fee-rate path under current NT CLOB V2 candidate pin and live entry remains blocked until match-time fee behavior is represented in readiness and evidence.
- `src/strategies/eth_chainlink_taker.rs:2827-2838` records decision evidence and submit admission before NT submit, so the submit boundary exists. This does not prove strategy math or economics are safe.
- `src/strategies/eth_chainlink_taker.rs:3306-3308` runs cache recovery and shell subscriptions on strategy start. That is NT strategy behavior; Phase 8 must account for it in readiness and evidence, not bypass it.

## Required Review Items

### Strategy Choice

`eth_chainlink_taker` is the only current ETH-specific production strategy candidate in tracked config/comments. It is not approved for live action because the approved live config is absent and the runtime contract still blocks fee/economics proof.

### Chainlink Feed Id And Environment

Blocked. The tracked example's active ruleset is BTC-oriented and the Chainlink section is commented. The example explicitly warns that ETH taker requires matching ETH/Chainlink operator config. The exact full ETH/USD Chainlink Data Streams feed id and environment must be verified from official Chainlink source or approved operator evidence before live action.

### Chainlink Data Streams Semantics

Blocked for live action. Runtime contract requires `GET /api/v1/reports?feedID=<feed_id>&timestamp=<boundary_unix>` for the selected market boundary and fails closed on missing, non-numeric, non-positive, or ambiguous Chainlink reports. Phase 8 must prove the configured feed id and report timestamp semantics for the exact ETH up/down market before live action.

### Exchange Reference Venues

Blocked. Required venue list includes Binance, Bybit, OKX, Kraken, Deribit, and Hyperliquid. Current tracked example only shows Binance reference config and Chainlink comments. Missing venue configuration and runtime evidence for the other venues blocks a production-grade multi-venue safety approval.

### Weighting, Staleness, Disable Rules

Blocked for live action. Requirements exist in runtime validation for reference venue weight, stale, and disable settings, but the approved operator config is absent. Phase 8 must show exact configured weights and fail-closed stale/disable thresholds before live action.

### Realized Volatility

Blocked for live action. Tests cover many realized-vol behaviors in `eth_chainlink_taker`, but a strategy-math approval requires tracing exact operator config values (`vol_window_secs`, `vol_gap_reset_secs`, `vol_min_observations`, `vol_bridge_valid_secs`) against live feed cadence and market duration.

The implementation slice must fail closed when realized volatility is zero or
negative; fair probability must be unavailable and entry evaluation skipped.

### `pricing_kurtosis`

Blocked for live action. The tracked fixture uses `pricing_kurtosis = 3.0`; the live example comment uses `pricing_kurtosis = 0.0`. The correct production value for ETH 5m up/down is not evidenced.

### Theta Decay

Blocked for live action. The tracked fixture uses `theta_decay_factor = 1.0`; the live example comment uses `theta_decay_factor = 0.0`. The correct production value is not evidenced.

### Fee And Rebate Assumptions

Blocked. Runtime contract states fee behavior is not proven for live entry. No Phase 8 live order may proceed until fee/rebate assumptions are represented in readiness/evidence and reviewed.

### Polymarket Market Selection

Blocked for live action. Current runtime contract has updown slug rules and current/next market selection, but the approved ETH 5m operator config is absent. Phase 8 must prove the exact `eth-updown-5m-*` market family selection from config and NT loaded instruments.

### Option Pricing Assumptions

Blocked for live action. Runtime contract documents Black-Scholes-like binary probability inputs, but live approval requires exact input trace: Chainlink boundary price, realized volatility, time to expiry, fee adjustment, edge threshold, and side selection.

The implementation slice must fail closed when time to expiry is zero or
negative; fair probability must be unavailable and entry evaluation skipped.

### Edge Threshold And Tiny-order Economics

Blocked. Tiny orders can be dominated by spread, adverse selection, fees, minimum order quantities, and latency. Phase 8 must prove cap economics and minimum quantity compatibility for the exact market/instrument before live action.

### Adverse Selection, Liquidity, Spread, Book Impact

Blocked. Config comments include `book_impact_cap_bps`, `forced_flat_thin_book_min_liquidity`, `lead_agreement_min_corr`, and `lead_jitter_max_ms`, but no approved live values or current book evidence are available.

## Approval Conditions Before Live Action

- Phase 7 no-submit report accepted by live canary gate.
- Approved redacted `config/live.local.toml` structure showing ETH/Chainlink/operator config.
- Official or operator-approved proof of exact ETH/USD Chainlink Data Streams feed id and environment.
- Multi-venue reference config and runtime readiness evidence, or explicit approved reduction of required venue set.
- Fee/rebate model and Polymarket minimum economics documented and reviewed.
- Dry/no-submit Phase 8 evidence artifact produced and reviewed.
- External reviewers approve exact Phase 8 implementation head.
- User approves exact live command and head SHA.
