# bolt-v2 Next Session: SSM Secrets + Deploy to EC2

## Context

bolt-v2 is a Polymarket trading system built on NautilusTrader's Rust `LiveNode` API. It's a standalone Rust binary — no Python layer. The binary compiles, builds, and has been **proven working locally** (connected to Polymarket, authenticated, reconciled, submitted and accepted an order).

## What Was Done This Session

### Investigation Results
- **PyO3 cdylib approach is dead.** NT's example strategies (EmaCross, GridMarketMaker) have NO Python bindings. `PyStrategy.inner()` is `pub(crate)`, `order_factory` is on the Cython layer. Can't extend from external crate.
- **Rust `LiveNode` is the correct path.** The old `src/main.rs` already had this working with `ExecTester`.
- **Observability features from Rust:** Cache (in-memory) and Portfolio work. Streaming works via `FeatherWriter::subscribe_to_message_bus()`. Msgbus Redis is a TODO in NT's Rust core (`crates/common/src/msgbus/core.rs:519`).
- **Redis msgbus gap:** The Rust `MessageBus` struct has no database adapter field. This is a known NT upstream TODO. Workaround: subscribe a handler to the message bus that pushes to Redis/S3 (same pattern as FeatherWriter). Not built yet.

### Code Changes
1. **Config alignment** — `src/config.rs` updated: `signature_type_id: u8` (was `signature_type: String`), `LoggingConfig` stripped of unused fields, `StrategyConfig` fields defaulted with `#[serde(default)]`.
2. **Panic removal** — All `panic!` calls on config errors converted to `Result` returns.
3. **Secrets consolidation** — Added `bolt-v2 secrets` CLI subcommand that resolves secrets from the TOML and outputs `KEY=VALUE` for `--env-file`. Single source of truth: TOML `[secrets]` section with `source = "op"` and op:// references. `secrets.env` deleted. `run.sh` has zero hardcoded op:// paths.
4. **DRY fix** — Base64 padding logic extracted to `pad_base64()`. Secret resolution unified through `SECRET_FIELDS` constant and `resolve_field()`.
5. **Partial env check fix** — `inject()` now checks all 5 env vars before skipping, not just `POLYMARKET_PK`.
6. **Dead code removal** — `main.py`, `strategy.py`, `requirements.txt`, `test_latency.py`, `.env.example`, `.venv/`, `__pycache__/` all removed.
7. **Dockerfile rewritten** — Multi-stage Rust build from `rust:1.94-bookworm`.
8. **`.gitignore` updated** — Removed Python entries, added `*.log`, `.omx/`.

### Architecture Decisions
- **Rust binary only, no Python.** NT's Python `TradingNode` has all features but Python overhead. NT's Rust `LiveNode` is faster but missing msgbus Redis (upstream TODO).
- **Controlplane (bolt v1) is the deployment layer.** The controller/helper is binary-agnostic — it downloads from S3, verifies checksums, swaps symlinks, starts systemd. Bolt v2 is a managed workload under it.
- **Deploy stack (`deploy.sh`) is NOT the deployment path.** The controlplane replaces it with mechanical promotion, health gates, and automatic rollback.
- **Separation of concerns:** Bolt v2 owns in-process trading runtime (NT LiveNode, strategy composition). Bolt v1 controlplane owns deployment lifecycle (apply, stop, quarantine, rollback, health monitoring).
- **Multi-strategy config (Codex's hybrid schema with `[[venues]]` + `[[strategies]]`)** is deferred until we have multiple strategies. Current single-strategy config works.

## Current State

### Working
- `cargo build --release` succeeds, zero warnings
- `bolt-v2 run --config config/live.toml` connects to Polymarket, authenticates (GnosisSafe), loads 2 instruments from event slug, reconciles, subscribes to quotes, submits order, order accepted by venue
- `bolt-v2 secrets --config config/live.toml` resolves secrets from 1Password and outputs KEY=VALUE lines
- Tested on macOS M4 natively (no Docker needed for Rust path)

### Files in Repo
```
bolt-v2/
  Cargo.toml           # Rust deps (NT git rev af2aefc2)
  Cargo.lock           # Pinned deps
  src/main.rs          # Entry point — CLI with `run` and `secrets` subcommands
  src/config.rs        # Config parsing + secret resolution (SSM only after Task 1)
  config/example.toml  # Template with SSM parameter paths
  config/live.toml     # Live config (gitignored)
  .gitignore           # target/, config/live.toml, *.log, .omx/
  tests/verify_build.sh # Compilation + CLI verification
```

### Removed This Session
- `Dockerfile`, `.dockerignore`, `run.sh` — Docker was needed for Python (macOS segfault). Rust binary runs natively on both macOS and Linux. No Docker needed. Deploy native binary via controlplane helper.

### Key Technical Details
- NT git rev: `af2aefc24451ed5c51b94e64459421f1dd540bfb` (pinned in Cargo.toml)
- Rust toolchain: 1.94.1, edition 2024
- NT version at this rev: 1.225.0 (Rust crate, different from pip 1.224.0)
- `signature_type_id` is `u8`: 0=EOA, 1=PolyProxy, 2=PolyGnosisSafe
- API secret needs base64 padding (handled in `pad_base64()`)
- serde/toml silently ignores unknown TOML sections (streaming, portfolio, cache, msgbus pass through without dead structs)
- Secret sources: `"op"` (1Password CLI), `"env"` (environment variables). SSM not yet implemented.

## Task 1: Replace All Secret Sources with SSM Only

### What
Remove `source = "op"` and `source = "env"` from `src/config.rs`. SSM is the single secret source. No dual paths.

### Why
Three secret sources = three paths to maintain. SSM works everywhere: EC2 reads via instance profile, local dev reads via AWS CLI. 500ms startup cost (5 parameters x ~100ms) is negligible.

### Changes Required

**`src/config.rs`:**
- Remove `source` field from `SecretsConfig` — it's always SSM
- `resolve_secret()` becomes SSM-only: calls `aws ssm get-parameter --name <field> --with-decryption --query 'Parameter.Value' --output text`
- Remove the `"op"` branch (1Password CLI)
- Remove the `"env"` branch (environment variables)
- Remove the `inject()` method's "all env vars already set" shortcut — no longer needed
- `inject()` resolves from SSM and sets env vars for NT
- `print_env()` resolves from SSM and prints KEY=VALUE (for Docker `--env-file`)

**`src/main.rs`:**
- Remove the `secrets` subcommand — no longer needed if we're not piping secrets to Docker
- OR keep it as a diagnostic tool (resolve from SSM, print to verify)
- Decision: keep it. Useful for verifying SSM connectivity.

**`config/example.toml`:**
- Remove `source` field from `[secrets]`
- Field values are SSM parameter paths directly:
```toml
[secrets]
polymarket_pk = "/bolt/polymarket/private-key-b64"
polymarket_api_key = "/bolt/polymarket/api-key"
polymarket_api_secret = "/bolt/polymarket/api-secret"
polymarket_passphrase = "/bolt/polymarket/api-passphrase"
polymarket_funder = "/bolt/polymarket/funder"
```

**`run.sh`:**
- Deleted. No Docker. Binary runs natively.

**`Dockerfile` + `.dockerignore`:**
- Deleted. Docker was solving a Python segfault problem. Rust binary runs natively on macOS and Linux.

### SSM Parameter Names
Verify against bolt v1's `src/secrets.rs`:
```rust
pub const POLYMARKET_PARAMS: &[&str] = &[
    "private-key-b64",
    "api-key",
    "api-secret",
    "api-passphrase",
    "rpc-url",
];
```
SSM prefix from bolt v1 configs: `/bolt/polymarket`

Note: bolt v1 has `rpc-url` but bolt v2 doesn't use it (NT handles RPC internally). Bolt v1 has `private-key-b64` but bolt v2's field is `polymarket_pk` — verify these map correctly. NT expects `POLYMARKET_PK` env var with the raw private key, not base64-encoded.

### Verification
- `bolt-v2 secrets --config config/live.toml` resolves all 5 secrets from SSM
- `bolt-v2 run --config config/live.toml` starts and connects to Polymarket
- Test locally with AWS CLI configured
- Test on EC2 with instance profile

## Task 2: Archive Bolt v1 on Strategy Instance

### What
Snapshot the current state of `/opt/bolt/` on the `bolt-polymarket` instance (eu-west-1) to S3 before deploying bolt v2.

### Steps
1. SSM into the instance: `aws ssm start-session --target <instance-id> --region eu-west-1`
2. Check what's running: `systemctl list-units 'bolt@*.service' --state=active`
3. Stop any running strategies: `sudo systemctl stop bolt@<instance>.service`
4. Archive: `tar czf /tmp/bolt-v1-archive.tar.gz /opt/bolt/`
5. Upload: `aws s3 cp /tmp/bolt-v1-archive.tar.gz s3://bolt-deploy-artifacts/archives/bolt-v1-$(date +%Y%m%d).tar.gz`
6. Clean up: `rm /tmp/bolt-v1-archive.tar.gz`

### Instance Details
- Instance tag: `bolt-polymarket`
- Region: `eu-west-1`
- Instance ID: resolve via `aws ec2 describe-instances --filters Name=tag:Name,Values=bolt-polymarket --region eu-west-1`
- User: `ubuntu`
- S3 bucket: `bolt-deploy-artifacts`

## Task 3: Deploy Bolt v2 to EC2

### What
Cross-compile bolt v2 for aarch64 Linux, upload to the strategy instance, configure, and run.

### Steps
1. Cross-compile: `cross build --release --target aarch64-unknown-linux-gnu` (requires `cross` tool + Docker)
2. Upload binary to instance: via SSM/S3
3. Create EC2 config: `config/ec2.toml` with `source = "ssm"` and SSM parameter paths
4. Upload config to `/opt/bolt/config/bolt-v2.toml`
5. Create systemd unit or reuse `bolt@bolt-v2.service` template
6. Create env file at `/opt/bolt/.env.bolt-v2` with `BOLT_CONFIG=/opt/bolt/config/bolt-v2.toml`
7. Start: `sudo systemctl start bolt@bolt-v2.service`
8. Verify: `journalctl -u bolt@bolt-v2 -f` — should show NT banner, Polymarket connection, instrument loading, order lifecycle

### EC2 Architecture
- Target: `aarch64-unknown-linux-gnu` (ARM64, Ubuntu 22.04)
- The instance already has: `/opt/bolt/` directory structure, systemd `bolt@.service` template, AWS CLI, SSM agent, IAM instance profile with SSM/S3 permissions

## Hard Rules (NON-NEGOTIABLE)

1. **NO HARDCODES** — every value comes from config.
2. **NO DUAL PATHS** — one way to do each thing.
3. **NO DEBTS** — no TODO, no "fix later".
4. **NO CREDENTIAL DISPLAY** — never cat/print/log secrets.
5. **VERIFY BEFORE CLAIMING** — run it, see output, then claim it works.

## Future Work (NOT this session)

- **Multi-strategy config** — Codex's `[[venues]]` + `[[strategies]]` hybrid schema. Deferred until we have multiple strategies.
- **Controlplane integration** — heartbeat metric + halt flag for automated health monitoring and rollback. Deferred until after initial deploy is proven.
- **Streaming (FeatherWriter)** — wire up after deploy is stable.
- **Custom persistence (Redis/S3 event logger)** — wire up when strategies hold real positions.
- **Msgbus Redis** — upstream NT TODO, no clean workaround from outside the crate.
