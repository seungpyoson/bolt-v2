# T038 Binance Runner Guide

Date: 2026-05-21
Scope: T038 no-submit readiness only.

This is not a readiness claim, not canary approval, and not production trading
approval. T038 and T046 remain unchecked.

## Current Answer

Do not rerun T038 from the local Mac while the Binance API key is whitelisted
only for EC2 EIP `34.248.143.2`.

Fresh evidence says the latest no-submit command ran on local macOS
`SP-MB-Pro.local` from public IP `58.232.146.158`, while AWS says EIP
`34.248.143.2` is attached to EC2 instance `i-0b68843392a62e359` and that
instance is `stopped`.

Therefore the latest Binance `Invalid X-MBX-APIKEY header` failure is consistent
with runner-IP mismatch. It is not proof that the SSM key/secret pair is wrong.

## What Is Verified Or Operator-Attested

- Current head: `ac656c2bdd9c5457a3682aa29355d94c48715049`
- `origin/main`: `d55ccfefc316928a67a23cf076c8b7e584e011bd`
- PR #388: open, head `ac656c2bdd9c5457a3682aa29355d94c48715049`, base `831368756bf5a7f8398944502dcce5fcc7c7952d`, merge state `CLEAN`
- `config/live.local.toml` mode `0600`, size `5024`, SHA-256 `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`
- `config/live.local.toml` configures Binance `environment = "mainnet"`, REST `https://api.binance.com`, and SBE WS `wss://stream-sbe.binance.com/ws`
- Rust mapping passes those URLs into NT `BinanceDataClientConfig` as `Some(...)`
- `secrets check` and `secrets resolve` passed without printing secret values
- Operator-confirmed Binance console checks: key type confirmed, active confirmed, configured public-key fingerprint matched, EIP `34.248.143.2` allowed

## Latest T038 Rerun

Command:

```sh
cargo run --bin bolt-v2 -- no-submit-readiness --config /Users/spson/Projects/Claude/bolt-v2/.worktrees/production-readiness-evidence-audit/config/live.local.toml
```

Result:

- Report path: `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/no-submit-readiness.json`
- Report mode/size: `0600`, `1283` bytes
- Report SHA-256: `1ea225543fad0f739e711b2842db254bc9a52f6677eba015a84f032a69c4b5a4`
- Schema: `bolt-v3.no-submit-readiness.v2`
- Generated timestamp: `1779373729` (`2026-05-21 23:28:49 KST`)
- Config bundle checksum: `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`
- Executable identity: `ffb56ce27899987b5028e2913dfd203d78297eb89968b99016bdfdb5f5d4ace3`
- Satisfied stages: `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_disconnect`, `report_write`
- Failed stage: `controlled_connect`
- Skipped stage: `reference_readiness`
- Runtime evidence: `binance_reference` data did not connect; `polymarket_main` data/execution connected; NT did not start trader
- Process check: no `bolt-v2` executable remained running after command exit

This is blocker evidence only. T038 is not satisfied.

## Why EC2 Matters

Evidence:

- Local public IP probe returned `58.232.146.158`
- AWS `describe-addresses` for `34.248.143.2` returned instance `i-0b68843392a62e359`
- AWS `describe-instances` returned state `stopped` for that instance
- AWS `describe-security-groups` for `sg-08921a4b725682171` returned SSH ingress only from `59.8.178.135/32` and `118.129.66.2/32`, not current local IP `58.232.146.158`
- AWS `ssm describe-instance-information` returned no managed-instance record for `i-0b68843392a62e359` while stopped
- Binance SBE rejected the local run with `Invalid X-MBX-APIKEY header`

If Binance allows only EIP `34.248.143.2`, T038 must run from that EIP or the
allowlist must temporarily include the runner IP. Running from the local Mac is
expected to fail the Binance SBE handshake.

## Next Safe Step

Pick one runner path:

1. Preferred: start EC2 instance `i-0b68843392a62e359`, verify it still has public IP `34.248.143.2`, then prove access by either SSM online status or SSH from an allowed source before rerunning T038 from that host.
2. If using SSH from the current local network, first update or use an allowed EC2 security-group source; current local IP `58.232.146.158` is not in the observed SSH ingress list. If the security group is changed temporarily, revert it after the run and record that revert in the evidence.
3. Alternative: temporarily allow the local runner public IP in Binance, rerun T038 locally, then remove that allowlist entry.

Do not run T046 until T038 produces a fresh report where every required stage is
`satisfied` and the live canary gate accepts the report's approval hash,
executable identity, config checksum, freshness, and stages.

## Binance References

- Error `-2015`: <https://raw.githubusercontent.com/binance/binance-spot-api-docs/master/errors.md>
- SBE auth requirements: <https://raw.githubusercontent.com/binance/binance-spot-api-docs/master/sbe-market-data-streams.md>
