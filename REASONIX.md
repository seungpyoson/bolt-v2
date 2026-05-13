# bolt-v2

Rust binary for automated trading on Polymarket via NautilusTrader.

## Stack

- **Language** — Rust 2024 edition, pinned to 1.95.0 (`rust-toolchain.toml`).
- **Framework** — NautilusTrader (`nautilus_*` crates, git dep) — live trading, exchange clients, data pipelines.
- **Async** — `tokio` (full features) + `tokio-tungstenite` (WebSocket).
- **CLI** — `clap` derive, subcommands: `run`, `secrets check`, `secrets resolve`.
- **Config** — TOML via `serde` + custom schema; secrets resolved from AWS SSM at startup.
- **Data / Persistence** — Apache Arrow + Parquet.
- **Exchange adapters** — Binance, Bybit, Deribit, Hyperliquid, Kraken, OKX, Polymarket.
- **Oracles** — Chainlink Data Streams Report (`chainlink-data-streams-report`).

## Layout

- `src/` — library crate (`lib.rs`) + four binaries (`bolt-v2`, `render_live_config`, `stream_to_lake`, `raw_capture`).
- `tests/` — integration tests (`.rs` files in root, not `*_test.rs`); unit tests live in-source under `#[cfg(test)]`.
- `config/` — live TOML runtime config (secrets excluded per `.gitignore`).
- `scripts/` — Python verification scripts for runtime literals, provider leaks, naming conventions.
- `deploy/` — systemd unit + install script for production deployment.
- `contracts/` — Polymarket CLOB contract addresses / ABI.
- `docs/` — postmortems, bolt-v3 specs, superpowers documentation.
- `.worktrees/` — git worktrees for parallel feature branches (each is a full checkout).

## Commands

All via `just` (must be installed). The justfile is the single source of truth — CI calls these same recipes.

| Command | What it does |
|---------|-------------|
| `just build` | Release cross-compile via `cargo zigbuild` (target: `aarch64-unknown-linux-gnu`). |
| `just test` | `cargo nextest run --locked` — requires `cargo-nextest`. |
| `just fmt` | `cargo fmt` (gated by rust-verification wrapper). |
| `just fmt-check` | `cargo fmt --check` + Python verification scripts as prerequisites. |
| `just clippy` | `cargo clippy` (gated by rust-verification wrapper, `-D warnings`). |
| `just deny` | `cargo deny check bans`. |
| `just deny-advisories` | `cargo deny check advisories`. |
| `just check-aarch64` | `cargo check --target aarch64-unknown-linux-gnu`. |
| `just setup` | Install pinned `cargo-nextest`, `cargo-deny`, `cargo-zigbuild`; verify Zig 0.15.2 is installed. |
| `just live` | Generate runtime config from `config/live.local.toml` → `config/live.toml`, then run. |
| `just ci-lint-workflow` | Lint CI workflow YAML + shell scripts for hardcoded cargo invocations. |

## Conventions

- **No hardcoded runtime values** — all IDs, quantities, timeouts come from TOML config. Verified by `scripts/verify_bolt_v3_runtime_literals.py`.
- **Secrets via SSM only** — AWS SSM is the sole credential source; no env vars, no local files, no CLI subprocesses.
- **Snake_case** for module names, identifiers, and file names.
- **`bolt_v3_` prefix** on most library modules reflecting an incremental v3 migration within the v2 crate.
- **One branch = one scope** — branches implement exactly one declared issue; PRs must flag scope drift.
- **cargo-deny** enforces allowed licenses, bans multiple-versions, and ignores two unmaintained transitive advisories (`RUSTSEC-2024-0436`, `RUSTSEC-2025-0134`).

## Watch out for

- **`just` is the entry point** — never call `cargo build` / `cargo test` directly in CI or recipes; the justfile validates workspace boundaries and runs verification checks.
- **Release builds require Zig** — `cargo-zigbuild` + Zig 0.15.2 are needed for cross-compilation to `aarch64-unknown-linux-gnu`.
- **Python verification layer** — several lint/check commands go through `rust_verification.py` which wraps cargo; running cargo directly bypasses those checks.
- **`config/live.toml` and `config/live.local.toml` are gitignored** — the example template is `config/live.local.example.toml`.
- **Reasonix context** — `REASONIX.md` is repo-shared agent context at the same level as `AGENTS.md` / `CLAUDE.md`; local AI tool config dirs (`.claude/`, `.gemini/`, `.opencode/`, `.codex/`, `.pi/`, `.agents/`, `.factory/`, etc.) are local state, not project docs.
