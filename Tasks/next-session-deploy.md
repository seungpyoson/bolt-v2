# bolt-v2 Next Session: Deploy to EC2

> Historical handoff. Current operator-config workflow:
> - `config/live.local.toml` is the human-edited local source of truth.
> - `config/live.local.example.toml` is the tracked template.
> - `config/live.toml` is the generated runtime artifact.
> - Generate before deploy, upload, check, resolve, or run.

## What Was Done

### Task 1: SSM-Only Secrets + Config Restructuring (COMPLETE)
- Removed `source` field, `"op"` and `"env"` backends from `config.rs`
- Single secret source: `aws ssm get-parameter --with-decryption --region <region>`
- Wallet config grouped: `[wallet]` (signature_type_id, funder) + `[wallet.secrets]` (region, pk, api_key, api_secret, passphrase)
- Funder moved from secrets to venue config (public on-chain address, not a secret)
- Env var mappings consolidated in single `resolve_env_vars()` function
- `unsafe { set_var() }` moved before tokio runtime creation (no more UB)
- CLAUDE.md updated: removed stale architecture sections, added rules 5-7
- Generated `config/live.toml` reflects the intended runtime structure, including funder = `0xA3a5E9c062331237E5f1403b2bba7A184e5de983`
- All 4 SSM secrets resolve from eu-west-1. Verified against the generated runtime config

### Task 2: Archive Bolt v1 (COMPLETE)
- AMIs created for ALL 5 instances — complete disk images, all `available`:

| Instance | Region | Instance ID | AMI |
|----------|--------|-------------|-----|
| bolt-polymarket | eu-west-1 | i-0a69531fd362c499a | ami-0c2f847b2685d9afa |
| bolt-kalshi | us-east-2 | i-0fa8bed60c0ecbb1e | ami-0ccef4684165f3a3a |
| pm-data-collector | eu-west-1 | i-000aa97ca34258eb1 | ami-03da945eb6a71ee58 |
| kalshi-data-collector | us-east-2 | i-0aff41dce9d8fffbf | ami-0f4ae63b57f49b25f |
| parquet-runner | us-east-1 | i-08c71fc6a70b02033 | ami-075abe9fd9f69b0e2 |

- bolt-polymarket services STOPPED: `bolt@pm-eth5m-sniper`, `bolt-log-forwarder`
- Other 4 instances still running normally
- S3 tar archives cancelled (AMIs are the authoritative backup)
- IAM policy `bolt-archive-write` created and attached to all 5 roles (for future S3 uploads)

### SSM Notes
- Session manager plugin must be installed from AWS directly (not brew): `curl + sudo installer`
- SSM agent on bolt-polymarket and parquet-runner needed reboots to fix broken RunShellScript execution
- `send-command` uses JSON file input: `--cli-input-json file:///tmp/ssm-xxx.json`
- Commands go through `safe_external.py` wrapper

## Task 3: Deploy Bolt v2 to EC2 (NOT STARTED)

### Prerequisites
- Cross-compile for `aarch64-unknown-linux-gnu` (instance is ARM64 Ubuntu 22.04)
- No Docker needed: use `cargo-zigbuild` (`brew install zig`, `cargo install cargo-zigbuild`)
- Or build directly on the instance (install Rust there)

### Deploy Steps
1. Cross-compile: `cargo zigbuild --release --target aarch64-unknown-linux-gnu`
2. Upload binary to S3: `aws s3 cp target/.../bolt-v2 s3://bolt-deploy-artifacts/artifacts/bolt-v2/`
3. SSM to instance: download from S3, place at `/opt/bolt-v2`
4. Upload generated runtime config: `config/live.toml` → instance
5. Create systemd service: `bolt-v2.service`
6. Disable v1 units: `systemctl disable bolt@pm-eth5m-sniper bolt-log-forwarder`
7. Delete v1 files (AMIs are the backup)
8. Start v2: `systemctl start bolt-v2`
9. Verify: journalctl — NT banner, Polymarket connection, order lifecycle

### Architecture Decisions
- NT LiveNode replaces trading binary + data collectors + parquet converter (5 instances → 1 per venue)
- FeatherWriter for data persistence: available in NT but NOT wired up yet. Needs `FeatherWriter::subscribe_to_message_bus()` call in main.rs. Deploy trading-only first, add persistence in follow-up.
- `StreamingConfig` exists on `LiveNodeConfig` but builder sets it to `None`. Need to pass it manually or create FeatherWriter post-build.
- Data collector instances keep running until FeatherWriter is wired up in v2

### Instance Details
- Target: bolt-polymarket (`i-0a69531fd362c499a`, eu-west-1)
- Architecture: aarch64, Ubuntu 22.04
- Public IP: 34.248.143.2
- IAM role: bolt-polymarket-exec-role
- SSH key pair: bolt-polymarket-key (user doesn't have the .pem)
- v1 services stopped, v1 files still on instance

### Generated Runtime Config (`live.toml`) — READY
- `[wallet]` with signature_type_id=2, funder=0xA3a5E9c0...
- `[wallet.secrets]` with region=eu-west-1, SSM paths for 4 credentials
- Verified: all secrets resolve from SSM
