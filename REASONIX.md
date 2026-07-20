# bolt-v2

Rust binary for automated trading on Polymarket via NautilusTrader.

## Stack

- **Language** — Rust 2024 edition, pinned to 1.97.0 (`rust-toolchain.toml`).
- **Framework** — NautilusTrader (`nautilus_*` crates, git dep) — live trading, exchange clients, data pipelines.
- **Async** — `tokio` (full features); WebSocket transport is provided by the NautilusTrader provider crates.
- **CLI** — `clap` derive, subcommands: `run`, `secrets check`, `secrets resolve`.
- **Config** — TOML via `serde` + custom schema; secrets resolved from AWS SSM at startup.
- **Data / Persistence** — Apache Arrow + Parquet.
- **Exchange adapters** — Binance and Polymarket via NautilusTrader (`nautilus-binance`, `nautilus-polymarket`). Legacy direct adapters for Bybit, Deribit, Hyperliquid, Kraken, OKX, and Chainlink Data Streams were retired in Phase 9 (see `specs/003-phase9-current-main-audit/tasks.md` T035).
## Layout

- `src/` — library crate (`lib.rs`) + two binaries (`bolt-v2`, `stream_to_lake`). The legacy `render_live_config` and `raw_capture` binaries were retired in Phase 9 (see `specs/003-phase9-current-main-audit/tasks.md` T068).
- `tests/` — integration tests (`.rs` files in root, not `*_test.rs`); unit tests live in-source under `#[cfg(test)]`.
- `config/` — live TOML runtime config (secrets excluded per `.gitignore`).
- `scripts/` — product/deploy tooling (config generators, migration helpers, systemd unit rendering) plus `sccache_eligibility.py`, the sole CI Python helper.
- `deploy/` — systemd unit + install script for production deployment.
- `contracts/` — Polymarket CLOB contract addresses / ABI.
- `docs/` — postmortems, bolt-v3 specs, superpowers documentation.
- `.worktrees/` — git worktrees for parallel feature branches (each is a full checkout).

## Commands

All via `just` (must be installed). The justfile is the single source of truth. `just build`, `just test`, and `just clippy` are workflow/operator lanes. The local agent path is `just fmt` plus a plain `git push` of the exact branch head; remote evidence for a pushed head comes from the advisory CI workflow and targeted `just rust-probe` dispatches.

| Command | What it does |
|---------|-------------|
| `just build` | Release cross-compile via `cargo zigbuild` (target: `aarch64-unknown-linux-gnu`). |
| `just test` | `cargo nextest run --locked`; pass nextest args after `--`, e.g. `just test -- --partition count:1/4`; LiveNode-heavy integration binaries are serialized by `.config/nextest.toml`. |
| `just fmt` | Format the root and backtesting-vertical-slice workspaces. |
| `just clippy` | `cargo clippy --locked -- -D warnings`. |
| `just check-aarch64` | `cargo check --target aarch64-unknown-linux-gnu`. |
| `just setup` | Install pinned `cargo-nextest`, `cargo-deny`, `cargo-zigbuild`; verify Zig 0.15.2 is installed. |
| `just live` | Require `BOLT_LIVE_PROFILE=<profile-id>`, derive `config/profiles/<profile-id>.overlay.toml`, compose it with `config/root.toml` → `config/live.toml`, then run. |
| `just live-verify` | Prove a deployed runtime config re-composes from the tracked overlay+base and still loads against this binary. |

## Conventions

- **No hardcoded runtime values** — all IDs, quantities, timeouts come from TOML config.
- **Secrets via SSM only** — AWS SSM is the sole credential source; no env vars, no local files, no CLI subprocesses.
- **Snake_case** for module names, identifiers, and file names.
- **`bolt_v3_` prefix** on most library modules reflecting an incremental v3 migration within the v2 crate.
- **One branch = one scope** — branches implement exactly one declared issue; PRs must flag scope drift.
- **cargo-deny** enforces allowed licenses, bans multiple-versions, and ignores two unmaintained transitive advisories (`RUSTSEC-2024-0436`, `RUSTSEC-2025-0134`).

## Watch out for

- **`just` is the convenience entry point locally** — recipes run plain raw cargo; CI runs the same raw cargo commands directly in workflow YAML, so any red check is reproduced by running the identical command.
- **Remote-first Rust verification** — agent sessions use the workflow in `AGENTS.md`: push, read advisory CI evidence (fmt/clippy/test/build), detach.
- **Release builds require Zig** — `cargo-zigbuild` + Zig 0.15.2 are needed for cross-compilation to `aarch64-unknown-linux-gnu`.
- **`config/live.toml` is a generated, gitignored runtime artifact** — set `BOLT_LIVE_PROFILE=<profile-id>`, derive `config/profiles/<profile-id>.overlay.toml`, compose that tracked overlay over the base template `config/root.toml` via `just live` / `bolt-v2 ops generate-live-config`, and never hand-edit it (#768). The legacy gitignored `config/live.local.toml` is no longer a source of truth.
- **Reasonix context** — `REASONIX.md` is repo-shared agent context at the same level as `AGENTS.md` / `CLAUDE.md`; local AI tool config dirs (`.claude/`, `.gemini/`, `.opencode/`, `.codex/`, `.pi/`, `.agents/`, `.factory/`, etc.) are local state, not project docs.
