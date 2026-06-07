# T038 Binance Runner Guide

Date: 2026-05-22
Scope: T038 strategy-free connectivity readiness only.

This is not live-submit approval and not production trading approval. T038 is now
satisfied by the EC2/EIP strategy-free proof below. T046 remains unchecked.

## Current Answer

The Binance blocker on the local reruns was runner IP mismatch.

Evidence:

- Local reruns used public IP `58.232.146.158`.
- Operator attested the Binance API key allowed EC2 EIP `34.248.143.2`.
- AWS showed EIP `34.248.143.2` attached to EC2 instance `i-0b68843392a62e359`.
- The local strategy-free rerun failed Binance SBE with `Invalid X-MBX-APIKEY header`.
- The EC2 rerun from EIP `34.248.143.2`, using the same approved config hash and resolved SSM secrets, connected Binance SBE and satisfied every strategy-free readiness stage.

This proves the local failure cause for T038. It does not prove first live-order
safety or production live trading readiness.

## EC2 Strategy-Free Proof

Runner:

- EC2 instance: `i-0b68843392a62e359`
- Public IP: `34.248.143.2`
- SSM state before run: `Online`
- Current head: `1245264f294ae096155bffc3236fb692cc46b46f`
- Current-head Linux aarch64 binary SHA-256: `7ef548c74688fc96ef3f06726df1838fb0742fe59176d386211ba3d680eccdc7`
- Binary path on EC2: `/tmp/bolt-v2-t038-1245264f`

Config:

- Root config path on EC2: `/tmp/config/live.local.toml`
- Root config SHA-256: `85fe8e17f2ffe813d464e8f5fe1908604060b5af9c5fd79f7b22ffe770b25289`
- Root config mode/size on EC2: `0600`, `5024`
- Strategy config path on EC2: `/tmp/config/strategies/binary_oracle.example.toml`
- Strategy config SHA-256: `3961588674c44e2265ad1797856be6e2a4f386ca2c55b7691e4e0f3c500e22b1`

Secret prechecks:

- `secrets check` passed on EC2 without printing secret values.
- `secrets resolve` passed on EC2 without printing secret values.

Command:

The EC2 run used the retired T038 readiness command with the checked local
config under `/tmp/config/live.local.toml`.

Runtime evidence:

- Binance SBE connected: `Connected: client_id=binance_reference`
- Polymarket data connected.
- Polymarket execution connected.
- Reference readiness stage was satisfied.
- Controlled disconnect completed.
- The strategy-free run wrote the readiness report.

Report:

- Path on EC2: readiness report under `/Users/spson/Projects/Claude/bolt-v2/var/bolt-v3-live/reports/`
- Mode/size: `0644`, `935`
- SHA-256: `53b945f92a2c747345ff65fb551ebf337cc4a5b5ab5f9552a92a4c6f68fb4126`
- Schema: retired T038 readiness schema v2
- Generated timestamp: `1779377947` (`2026-05-22 00:39:07 KST`)
- Config bundle checksum: `a6f0f1d1e472c88d848b8505dc138e136a55314ec89d80dbb6be926ab7b88639`
- Executable identity: `7ef548c74688fc96ef3f06726df1838fb0742fe59176d386211ba3d680eccdc7`
- Satisfied stages: `operator_approval`, `secret_resolution`, `live_node_build`, `controlled_connect`, `reference_readiness`, `controlled_disconnect`, `report_write`

T038 is satisfied by this evidence only.

## Pre-Existing EC2 Service Finding

Starting EC2 also auto-started a pre-existing service that was not part of the
T038 current-head strategy-free run.

Evidence:

- Unit: `bolt-v2.service`
- Unit state after remediation: `inactive`, `disabled`
- Unit command: `/opt/bolt-v2/bolt-v2 run --config /opt/bolt-v2/config/live.toml`
- Installed binary SHA-256: `4c95cd843f3329e4d267f0c9db91997f9ba8b411be2e9efbe89aab57b4f45078`
- Installed config SHA-256: `fa7d129c2d17bc6762458b7f48591797a4130ac5d523ab7e09ed340764d3eb06`
- Current-head binary used for T038: `/tmp/bolt-v2-t038-1245264f`
- Current-head binary SHA-256: `7ef548c74688fc96ef3f06726df1838fb0742fe59176d386211ba3d680eccdc7`

Classification:

- This was a pre-existing `bolt-v2` live service, not the current-head T038 strategy-free runner.
- It is a production control-surface blocker before any T046 first live-order run or production trading.
- The service was stopped and disabled during the EC2 session.
- Follow-up process check showed no `bolt-v2` process.
- Targeted journal review for the final auto-start window showed `Not starting trader: engine client(s) not connected` after a Binance SBE schema mismatch in the stale service.

## Remaining Gate

Do not run T046 until there is separate explicit operator approval for the
tiny-capital live-order attempt and the current live-submit admission path is
checked against this fresh T038 report, current executable identity, config
checksum, approval evidence, and report freshness.

## Binance References

- Error `-2015`: <https://raw.githubusercontent.com/binance/binance-spot-api-docs/master/errors.md>
- SBE auth requirements: <https://raw.githubusercontent.com/binance/binance-spot-api-docs/master/sbe-market-data-streams.md>
