# Evidence Map: Developer-Tool Storage Hygiene

Issue: #375
Branch: `codex/375-developer-tool-storage-hygiene`
Base: `origin/main` at `7a700fbf8129b04b7c94488880322a1f0df82fc6`
Workspace date: 2026-05-23 Asia/Seoul

## Control Sources

| Source | Evidence | Scope impact |
|---|---|---|
| GitHub issue #375 | The issue names `~/.codex/log/codex-tui.log`, `~/.codex/sessions/**/*.jsonl`, `~/.factory/logs/droid-log-single.log`, and `~/.rustup/toolchains/` as measured developer-tool storage symptoms. | #375 owns direct mitigation for those path families. |
| GitHub issue #375 comment 2026-05-17 | No implementation PR may merge until Phase 1 enumerates developer tools, exact written paths, growth shape, native rotation support, and ownership. | This evidence map is a gate artifact before implementation design. |
| GitHub issue #123 | #375 owns Codex log rotation, Codex sessions TTL, and rustup toolchain hygiene; #376 owns bolt-v3 runtime output, local CI/test artifacts, cargo registry, and cargo git. | Prevents overlap with #376. |
| PR #391 | Planning-only disk-pressure governance; records #375 as future implementation owner. | Does not implement #375. |
| PR #433 | Adds #374 cargo invocation enumeration only. | Does not implement #375. |
| PR #436 | Implements #374 T013/T014/T015 verifier/wrapper-governance slice only and states #375 remains separate. | Do not touch verifier/parser architecture for #375 unless evidence requires it. |
| `specs/014-disk-pressure-governance/spec.md:80-87` | FR-005 blocks #375 implementation until developer-tool enumeration exists; FR-008 routes bolt-v3 runtime and cargo registry/git to #376. | This PR must keep #375 and #376 separate. |
| `specs/014-disk-pressure-governance/contracts/disk-pressure-governance.md:48-55` | Cleanup safety requires status/dry-run before apply, active-process refusal, and never removing pinned or active Rust toolchains. | #375 cleanup must be dry-run first and protect pinned/active toolchains. |

## Current Local Measurements

Commands measured directory sizes only. No secret file contents were printed.

| Surface | Measurement | Current behavior |
|---|---:|---|
| Data volume | `/System/Volumes/Data` 460 GiB, 267 GiB used, 168 GiB available, 62% capacity. | The disk is not currently at the 95% incident level, but #375 surfaces still consume multiple GiB. |
| `~/.codex/log` | 3.5 GiB | Single active Codex log directory is unbounded in the current installation. |
| `~/.codex/log/codex-tui.log` | 3,731,232,741 bytes, 3.5 GiB | Single rolling file. |
| `~/.codex/sessions` | 3.5 GiB, 1744 JSONL files | Many transcript files; 97 files older than 7 days and 0 older than 14 days at measurement time. |
| `~/.codex/archived_sessions` | 378 MiB | Related Codex session storage, lower priority than active sessions. |
| `~/.codex/logs_2.sqlite*` | 2.6 GiB db, 1.6 GiB wal, 4.1 MiB shm | Additional Codex telemetry database family not named in the original issue body. Cleanup semantics are not documented by Codex config reference, so it is report-only unless a deterministic safe policy is proven. |
| `~/.codex/history.jsonl` | 27 MiB | Codex has native `history.max_bytes` and `history.persistence`; this file is not the current space driver. |
| `~/.factory/logs` | 12 MiB | Currently bounded by small observed size, but historical issue evidence recorded 767 MiB. |
| `~/.factory/logs/droid-log-single.log` | 12,649,632 bytes | Single rolling file. |
| `~/.rustup/toolchains` | 4.3 GiB | Three installed toolchains. |
| `~/.rustup/toolchains/1.94.1-aarch64-apple-darwin` | 1.4 GiB, default | Protected unless no project and no active process uses it. |
| `~/.rustup/toolchains/1.95.0-aarch64-apple-darwin` | 1.4 GiB, active | Protected; it matches `rust-toolchain.toml`. |
| `~/.rustup/toolchains/stable-aarch64-apple-darwin` | 1.5 GiB | Candidate only if not default, not active, not pinned, and past retention. |

## Bolt Source Trace

| Source | Lines | Evidence |
|---|---:|---|
| `Cargo.toml` | 5 | Package requires Rust `1.95.0`. |
| `Cargo.toml` | 22-35, 51 | NautilusTrader crates are pinned to rev `7c2aafb30fb143069c915a3f2057bb12174405f6`. |
| `rust-toolchain.toml` | 1-2 | Project toolchain channel is `1.95.0`. |
| `.github/actions/setup-environment/action.yml` | 129-145 | CI reads `rust_toolchain` from `rust-toolchain.toml` and build tool pins from `justfile`. |
| `.github/actions/setup-environment/action.yml` | 158-184 | CI installs the selected Rust toolchain through `dtolnay/rust-toolchain`. |
| `justfile` | 91-134 | Rust verification commands route through `scripts/rust_verification.py`; local Rust cache status/prune is separate #286 scope. |
| `justfile` | 297-304 | `just setup` adds the configured target through rustup. |
| `justfile` | 306-339 | `just setup` requires prebuilt cargo-nextest, cargo-deny, cargo-zigbuild, and Zig; it does not install tools from source. |
| `.github/workflows/ci.yml` | 12-25 | Pull-request CI ignores `.codex/**`, `.claude/**`, `.gemini/**`, `.specify/**`, and root agent docs. |
| `.github/workflows/ci.yml` | 70-78 | Build-affecting detector includes Rust inputs and workflow/verifier files, not #375 developer-home storage. |
| `scripts/verify_ci_path_filters.py` | 19-33 | The verifier expects `.codex/**` and related agent config dirs to be path-filter ignored. |

Current Bolt reaches rustup through the pinned toolchain and target setup path. Current Bolt does not reach Codex or Factory logs as product code; those are developer-tool side effects from working on the repo.

## Pinned NautilusTrader Trace

Pinned checkout: `/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb`
Pinned rev: `7c2aafb30fb143069c915a3f2057bb12174405f6`

| Query | Result | Scope impact |
|---|---|---|
| `codex-tui`, `.codex`, `droid-log` | No matches in pinned NT checkout. | NT does not own Codex or Factory storage hygiene. |
| `rustup toolchain`, `toolchains` | One Miri install hint in `Makefile:761-764`; generic prose references in `build.py` and Binance credential comments. | NT does not define Bolt rustup retention policy. |
| `.specify/memory/constitution.md:20-22` | NT owns runtime adapter, protocol, market data, execution, order lifecycle, cache, portfolio/account/order/fill, reconciliation, venue wire translation. | #375 is outside NT runtime semantics. |

NT source is therefore N/A for the cleanup mechanics, except that Bolt must not change NT runtime or adapter behavior while addressing developer-tool storage.

## Native Tool Capability Trace

| Tool | Evidence | Native rotation/retention status |
|---|---|---|
| Codex CLI 0.133.0 | `codex --help` exposes `login`, `logout`, `resume`, `fork`, and scrollback/history UI help but no log rotation, session TTL, or prune subcommand. `codex debug --help` exposes model/app-server/prompt-input debug helpers but no retention command. | No native CLI cleanup command found. |
| Codex config | Local `~/.codex/config.toml` has a `[tui]` table but no matched `log_dir`, `history.max_bytes`, `history.persistence`, `retention`, `rotation`, or `ttl` keys from the non-secret pattern search. | Current install is not configured to cap Codex history or relocate logs. |
| OpenAI Codex config reference | Documents `history.max_bytes`, `history.persistence`, and `log_dir`: https://developers.openai.com/codex/config-reference | Native support exists for history file size and log directory, but not for `codex-tui.log` rotation or `sessions/**/*.jsonl` TTL. |
| Factory | `factory` executable was not found; `~/.factory/logs/droid-log-single.log` exists. | No native Factory cleanup capability proven. |
| rustup | `rustup toolchain list` reports stable, `1.94.1` default, and `1.95.0` active. | rustup can list/remove toolchains, but #375 must protect active, default, and project-pinned toolchains. |

## Phase 1 Developer-Tool Enumeration

Ownership values:

- `this issue`: #375 owns direct policy or bounded reporting.
- `tracked elsewhere`: existing issue owns implementation.
- `out of repo`: machine or user-profile policy, not bolt-v2 implementation.
- `report-only`: inventory and preflight evidence are in #375, but destructive cleanup is unsafe without a native contract.

| Category | Tool/path family | Observed size | Growth shape | Native rotation | Owner |
|---|---|---:|---|---|---|
| AI agents | `~/.codex/log/codex-tui.log` | 3.5 GiB | Single rolling file | No native rotation found | this issue |
| AI agents | `~/.codex/sessions/**/*.jsonl` | 3.5 GiB | Many transcript files | No TTL found | this issue |
| AI agents | `~/.codex/logs_2.sqlite*` | 4.2 GiB | SQLite db plus WAL | No documented cleanup found | report-only in this issue |
| AI agents | `~/.codex/archived_sessions` | 378 MiB | Tree of archived transcripts | No TTL found | report-only in this issue |
| AI agents | `~/.codex/history.jsonl` | 27 MiB | Single history file | Native `history.max_bytes` and `history.persistence` documented | report-only native-config surface in this issue |
| AI agents | `~/.factory/logs/droid-log-single.log` | 12 MiB current, 767 MiB historical | Single rolling file | No native policy proven | this issue |
| AI agents | `~/.claude` | 3.0 GiB | Tool profile, logs, state, and outputs | Outside bolt-v2 proof here | tracked elsewhere by #125 / claude-config owner |
| AI agents | `~/.gemini` | 667 MiB | Tool profile and state tree | Outside bolt-v2 proof here | report-only; no direct #375 cleanup |
| AI agents | `~/.kimi` | 1.6 GiB | Tool profile and state tree | Outside bolt-v2 proof here | report-only; no direct #375 cleanup |
| AI agents | `~/.opencode` | 158 MiB | Tool profile and state tree | Outside bolt-v2 proof here | report-only; no direct #375 cleanup |
| AI agents | `~/.pi` | 510 MiB | Tool profile and state tree | Outside bolt-v2 proof here | report-only; no direct #375 cleanup |
| AI agents | `~/.aider` | 2.0 MiB | Tool config/session state | Outside bolt-v2 proof here | report-only; no direct #375 cleanup |
| Version managers | `~/.rustup/toolchains` | 4.3 GiB | One directory per installed toolchain | rustup can uninstall, no project TTL | this issue |
| Build tools | `~/.cache/rust-verification/bolt-v2` | 15 GiB | Managed target/cache tree | Repo policy exists | tracked elsewhere by #286/#374 |
| Build tools | repo/worktree `target/` and `/private/tmp/bolt-v2-*` | Not owned by this issue; largest sampled tmp path 1.8 GiB | Build and review artifacts | Repo policy exists or future #374 cleanup | tracked elsewhere by #374 |
| Build tools | `~/.cargo/registry` and `~/.cargo/git` | 1.9 GiB and 699 MiB | Registry/git cache trees | cargo has cache behavior but no repo policy here | tracked elsewhere by #376 |
| Package managers | `~/.npm`, `~/Library/pnpm`, `~/.bun` | 1.2 GiB, 991 MiB, 566 MiB | Machine package caches | Tool-specific | out of repo per #123/#014 edge case |
| Package managers | Homebrew and Xcode caches | 92 MiB, 0 B | Machine caches | Tool-specific | out of repo per #123/#014 edge case |
| IDEs/editors | `~/.cursor`, Cursor app support | 587 MiB, 449 MiB | Profile/cache tree | App-specific | out of repo unless later tied to bolt-v2-only artifacts |
| IDEs/editors | VS Code profile/app support | 8 KiB, 14 MiB | Profile/cache tree | App-specific | out of repo unless later tied to bolt-v2-only artifacts |
| IDEs/editors | JetBrains profile/caches/logs | Not present | N/A | N/A | out of repo |
| Cloud CLIs | `~/.aws`, `~/.config/gcloud` | 8 KiB, 97 MiB | Config/cache trees | Tool-specific | out of repo; no secrets inspected |
| Browser tooling | Chrome profile/cache | 14 GiB profile, 3.3 GiB cache | Browser profile/cache trees | Browser-managed | out of repo unless a bolt-v2 browser harness creates dedicated profiles |
| Browser tooling | Chromium profile/cache | 4 KiB profile, cache absent | Browser profile/cache trees | Browser-managed | out of repo |
| MCP/plugins | `~/.codex/plugins`, MCP dirs | 104 MiB; common MCP dirs absent | Plugin cache tree | Plugin-specific | report-only; direct cleanup unsafe without plugin owner |
| Python tooling | `~/.cache/uv`, `~/.cache/pip` | 165 MiB, pip absent | Package cache | Tool-specific | out of repo unless used by repo verification |
| GitHub CLI | `~/.cache/gh`, `~/.config/gh` | 33 MiB, 8 KiB | Cache/config tree | Tool-specific | out of repo; no secrets inspected |

## Reachable And Future-Reachable Paths

Current reachable paths:

- Codex sessions/logs are reachable whenever Codex is used for bolt-v2 work.
- Codex SQLite log/state files are reachable in the current installed profile and already exceed 4 GiB.
- Factory droid log is reachable from the installed user profile even though the `factory` executable was not on PATH.
- rustup toolchains are reachable through `rust-toolchain.toml`, CI setup, and `just setup`.
- Managed Rust cache paths are reachable through `just` and `scripts/rust_verification.py`, but #286/#374 own those policies.

Future-reachable paths:

- New Codex plugin or app surfaces under `~/.codex/plugins`, sqlite state files, and generated assets can grow without appearing in the original issue body.
- Browser-driven tools can write large Chrome profile/cache trees; current Chrome data is multi-GiB but not bolt-v2-specific.
- Additional AI agents may write state under `~/.claude`, `~/.gemini`, `~/.kimi`, `~/.opencode`, and `~/.pi`; #375 should report them but not delete them without an owner-specific contract.
- Package managers and cloud CLIs can grow during bolt-v2 setup, but #123 and #014 classify general machine caches as out of repo.

## Behavior Classification

Current behavior:

- Codex log/session storage is multi-GiB and lacks local rotation/TTL configuration.
- rustup has active/default/stable toolchains installed, with `1.95.0` active and project-pinned.
- Factory droid log is small now but has historical evidence of unbounded growth.

Latent risk:

- Codex SQLite db/WAL files can become the largest Codex-owned surface, but no safe cleanup contract is currently proven.
- Browser and package-manager caches can dwarf #375 surfaces, but they are not bolt-v2-specific and must not be silently pulled into this PR.
- Removing default, active, or pinned rustup toolchains can break builds.

Future enablement requirement:

- The #375 implementation must expose deterministic status/preflight data before cleanup.
- Cleanup candidates must be config-driven, dry-run first, and protected by explicit exclusions for pinned/active toolchains and unsafe Codex database families.
- Any operator-facing new cleanup command or changed command semantics requires explicit operator approval before implementation.
