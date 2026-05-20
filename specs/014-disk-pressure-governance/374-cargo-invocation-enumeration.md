# Issue #374 Phase 1 Cargo Invocation Enumeration

Status: Draft for T011/T012. This is not an implementation design and does not approve a shim, cleanup, or preflight change.

Authoritative issue: #374, live body verified 2026-05-20. Parent: #123. History anchor: #48. Managed-cache retention boundary: #286 / PR #404.

## Required Coverage

#374 implementation is blocked until this enumeration is reviewed for gaps and overlaps. The enumeration must cover every way bolt-v2 Cargo or Rust toolchain work can:

- create unmanaged `target/` output,
- bypass managed `CARGO_TARGET_DIR`,
- mutate the shared managed target root,
- hide active `cargo` / `rustc` / `rust_verification.py` processes from cleanup safety checks,
- or trigger heavy work without a disk preflight.

## Evidence Inputs

| Evidence | Source | Required conclusion |
|---|---|---|
| Local worktree `target/` dirs reached 13 GiB on 2026-04-11 | #48 comments | Worktree-local Cargo output is a real recurrence class. |
| Main checkout `target/` reached 75-93 GiB on 2026-04-30 | #48 comments | Root checkout raw Cargo output is a real recurrence class. |
| Gemini-shell created an 8.8 GiB worktree `target/` in about 30 minutes on 2026-05-01 | #48 comments | Bash-shell agent launchers bypass zsh-only cargo wrapping. |
| Root checkout `target` reached 18 GiB on 2026-05-17 | #374 body | The same raw-Cargo target recurrence remained live after earlier fixes. |
| `.no-mistakes.yaml` runs `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` | `.no-mistakes.yaml` | no-mistakes is a live raw-Cargo producer until #374 changes it. |
| no-mistakes daemon had no `CARGO_TARGET_DIR` in its environment and wrote under `USER_HOME_DIR/.no-mistakes/worktrees/.../target` | `research.md` | no-mistakes must be enumerated as a launcher and target class. |
| `scripts/rust_verification.py cargo --repo ... -- clean` and managed `cargo test` ran concurrently against `USER_HOME_DIR/.cache/rust-verification/bolt-v2/target` | #374 body, 2026-05-20 | Destructive managed cargo subcommands must be separate from ordinary build/test commands. |
| `cmd_cargo` uses shared cache lock, while `cache_prune_payload` apply uses exclusive lock | `scripts/rust_verification.py` | Generic managed `cargo clean` can mutate the shared target without the cache-prune exclusive safety model. |
| `cmd_cleanup` currently reports ok with no removals | `scripts/rust_verification.py` | Existing cleanup entrypoint does not enforce the #374 artifact lifecycle sweep. |
| `env -iuLD_PRELOAD cargo build` parses to `{'env', '-iuLD_PRELOAD'}` on current main | local probe, 2026-05-20 | Current active-process parsing misses a listed wrapper residual. |
| `rustup run stable -- -- cargo build` parses to `{'rustup', '--'}` on current main | local probe, 2026-05-20 | Current active-process parsing misses repeated end-of-options markers. |

Probe command:

```bash
python3 -c "import importlib.util; spec=importlib.util.spec_from_file_location('rv','scripts/rust_verification.py'); rv=importlib.util.module_from_spec(spec); spec.loader.exec_module(rv); cases=['env -iuLD_PRELOAD cargo build','env -iu LD_PRELOAD cargo build','rustup run stable -- -- cargo build','rustup run --install stable cargo build']; print('\n'.join(f'{c} -> {rv.process_names_from_tokens(c.split())}' for c in cases))"
```

Observed output:

```text
env -iuLD_PRELOAD cargo build -> {'env', '-iuLD_PRELOAD'}
env -iu LD_PRELOAD cargo build -> {'env', '-iu'}
rustup run stable -- -- cargo build -> {'rustup', '--'}
rustup run --install stable cargo build -> {'cargo', 'rustup'}
```

## Enumeration Dimensions

### 1. Launcher Surface

| Launcher | Must enumerate | Current evidence / risk |
|---|---|---|
| zsh interactive and login shells | Whether `USER_HOME_DIR/.zshenv` cargo wrapper or repo-local script path is active | Historic managed path worked only for zsh-launched cargo. |
| bash, sh, dash, fish, non-login shells | Whether wrapper is absent, PATH shim present, or `CARGO_TARGET_DIR` preserved | Gemini-shell bash created local `target/`; Codex/Claude/Aider bash shapes remain in #374 scope. |
| shell aliases, shell functions, and builtins | alias/function `cargo`, `command cargo`, `exec cargo`, and builtin bypass forms | Must prove the selected route does not depend on one interactive-shell startup file. |
| clean environment launchers | `env -i`, `env -u*`, `env -S`, scrubbed PATH, absent routing env | Current parser misses bundled `env` forms. |
| no-mistakes daemon | daemon env, repo `.no-mistakes.yaml`, worktree target path, exact-head CI alternative | Current config is raw Cargo. |
| `just` recipes | Whether recipes call managed wrapper or raw Cargo | Existing policy routes managed recipes, but raw shell usage remains forbidden until #374 proves coverage. |
| install/build scripts | npm, cargo install scripts, setup scripts, and any script-launched Cargo | Must be classified before assuming managed routing. |
| external agent shells | Gemini-shell, Codex shell tool, Claude `bash -c`, Aider `/run` | Gemini-shell has direct 8.8 GiB evidence. |
| container shells | `docker exec` or other container-launched Cargo | Must be explicitly excluded or routed if it can touch host worktrees. |
| IDE/tooling | rust-analyzer, IDE cargo check, background diagnostics | Must be classified as managed, excluded, or separately bounded. |
| wrapper utilities | `command`, `exec`, `nohup`, `time`, `timeout`, `xargs`, `setsid`, `taskset`, `ionice`, `chrt`, `make`, `python -c` / `os.system(...)` | #374 body and contracts require wrapper inventory; current parser only handles a subset. |
| symlink or renamed tools | symlink-renamed `cargo` / `rustc`, direct rustup binary paths | Must not hide active Rust work from cleanup safety checks. |

### 2. Environment State

| State | Required classification |
|---|---|
| `CARGO_TARGET_DIR` set to managed target | Accept only if target resolves to `USER_HOME_DIR/.cache/rust-verification/bolt-v2/target` or reviewed namespace. |
| `CARGO_TARGET_DIR` unset | Unsafe for bolt-v2 Cargo unless another approved routing layer applies. |
| `CARGO_TARGET_DIR` set to repo/worktree/tmp/no-mistakes path | Unmanaged target producer; #374 must block, route, or explicitly exclude. |
| `CARGO_BUILD_TARGET_DIR` set | Must be classified as a target-dir override if supported by the pinned Cargo version. |
| `PATH` contains shell-agnostic shim before rustup cargo | Candidate route, must prove non-bolt-v2 pass-through. |
| `PATH` resolves rustup cargo directly | Unsafe unless repo `.cargo/config` or wrapper env routes target. |
| repo `.cargo/config.toml` `build.target-dir` | Must be classified because repo config can route target dirs even when shell env is absent. |
| `cargo --config build.target-dir=...` | Must be classified as a CLI target-dir override, not treated as ordinary Cargo. |
| `RUST_VERIFICATION_ROOT_BASE` overrides cache base | Must be config-controlled and bounded; not a raw workaround. |
| `RUST_VERIFICATION_REAL_CARGO` used by wrapper | Must not become a bypass path for ordinary users or no-mistakes. |
| `RUST_VERIFICATION_PRESERVE_ROUTING_ENV` present | Must be scoped to managed wrapper behavior. |
| `RUSTFLAGS`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER` | Must be classified for process visibility, rustc interception, and target/output side effects. |
| `CARGO_TARGET_TMPDIR` | Must be classified because build/test temporary output can move outside the expected target subtree. |
| `CARGO_INCREMENTAL` | Must be classified for cache-growth behavior even when target location is managed. |
| `CARGO_INSTALL_ROOT` | Must be classified for `cargo install` output ownership; unmanaged build targets remain forbidden. |
| `CARGO_HOME` / `RUSTUP_HOME` custom paths | Invocation-affecting state for #374; disk-retention ownership stays #375/#376 as applicable. |

### 3. Invocation Forms

| Form | Class | Required #374 handling |
|---|---|---|
| `cargo build`, `cargo check`, `cargo test`, `cargo clippy`, `cargo fmt`, `cargo doc`, `cargo run` | ordinary build/test/tool command | Must route to managed target or be explicitly denied in bolt-v2 scope. |
| `cargo --target-dir X ...` | explicit target override | Must reject unmanaged targets or prove `X` is managed. |
| `command cargo ...`, absolute cargo path, rustup cargo path | wrapper bypass | Must be blocked, routed, or listed as explicitly unsupported until fixed. |
| `rustup run TOOLCHAIN cargo ...` | toolchain wrapper | Must unwrap active process detection and preserve target routing across plain, single `--`, and repeated `-- --` variants. |
| `cargo +TOOLCHAIN ...` | rustup toolchain shorthand | Must route to managed target, remain visible to process checks, and receive explicit T013/T014 parser coverage. |
| `rustup run TOOLCHAIN -- -- cargo ...` and repeated `--` variants | wrapper residual | Current main misses cargo; contrast with `rustup run --install TOOLCHAIN cargo ...`, which detects `cargo` but may add noisy process names. |
| `env -iuLD_PRELOAD cargo ...`, `env -iu LD_PRELOAD cargo ...`, and related bundled or separated short options | wrapper residual | Current main misses cargo; must be tested before implementation. |
| `bash -c 'cargo ...'`, `sh -c`, `fish -c`, nested shells | shell wrapper | Must enumerate shell sourcing and target routing behavior. |
| `python -c 'os.system("cargo ...")'` | indirect wrapper | Must be classified. Parser currently does not inspect Python code strings. |
| `make`, `xargs`, `timeout`, `setsid`, `taskset`, `ionice`, `chrt` | indirect wrapper | Must be classified. Do not assume active-process detection sees cargo. |
| shebang-executed Cargo scripts, such as `#!/usr/bin/env cargo` | indirect wrapper | Must not hide cargo behind a script process name. |
| `cargo install ...` | installer/build command | Must classify build-target use separately from install output root; unmanaged build targets remain forbidden. |
| `rustup component add ...`, `rustup toolchain install ...` | rustup-managed install command | Must be classified as #375/#376 storage unless it launches repo-scoped Cargo/Rust work. |
| `cargo -Z unstable-options ... --out-dir X`, `cargo ... --artifact-dir X` | final-artifact override | Must classify copied artifacts outside the target tree; nightly-only paths do not bypass #374 lifecycle accounting. |
| direct `rustc`, `cargo-clippy`, `cargo-fmt`, `cargo-nextest`, `nextest` | Rust toolchain command | Must be active-process-visible and target-safe where applicable. |
| nested build-script/proc-macro Cargo/Rust work | nested toolchain work | Must be classified as cargo-managed artifact or non-applicable with evidence. |
| `cargo clean` | destructive managed mutation | Must not run through generic shared managed-cargo path while related processes are active. |
| future `cache-clean` / `cache-reset` | explicit destructive cache op | Must take exclusive lock, refuse active processes, and print rebuild-cost tradeoff. |

### 4. CWD Classes

| CWD | Target risk |
|---|---|
| canonical repo root | Raw Cargo creates `REPO_ROOT_PATH/target`. |
| registered git worktree root | Raw Cargo creates `.worktrees/<name>/target` or temp worktree target. |
| repo subdir | Cargo walks to repo root unless target overridden; must still route managed. |
| no-mistakes worktree | Raw Cargo creates `USER_HOME_DIR/.no-mistakes/worktrees/.../target`. |
| `/private/tmp/bolt-v2-*` review bundle | Raw Cargo or explicit `CARGO_TARGET_DIR` can create persistent temp artifacts. |
| outside any bolt-v2 repo | Shim must pass through or reject only if policy claims ownership. |

### 5. Target / Artifact Destinations

| Destination | Owner / required action |
|---|---|
| `USER_HOME_DIR/.cache/rust-verification/bolt-v2/target` | managed cache, #286 retention plus #374 destructive-op safety. |
| repo-local `target/` | #374 unmanaged target class, must be prevented and lifecycle-swept. |
| worktree-local `target/` | #374 unmanaged target class, must be prevented and lifecycle-swept. |
| `/private/tmp/bolt-v2-*` target dirs | #374 lifecycle sweep when bolt-v2-owned review/build bundle. |
| no-mistakes worktree `target/` | #374 no-mistakes raw Cargo drift. |
| `USER_HOME_DIR/.cargo/registry`, `USER_HOME_DIR/.cargo/git` | #376 steady-state inventory, not #374 target routing. |
| `USER_HOME_DIR/.rustup/toolchains` | #375 toolchain hygiene, not #374 target routing. |
| S3 | not an active target cache; artifacts/evidence only. |

### 6. Cargo Target And Profile

| Target/profile | Required classification |
|---|---|
| host debug/test target | Must route to managed target and remain visible to active-process checks. |
| host release target | Must route to managed target or declared build artifact path. |
| `aarch64-unknown-linux-gnu` | Managed-cache retention is #286; invocation routing and preflight still #374. |
| other cross-targets | Must not silently create unmanaged per-worktree target trees. |
| custom Cargo profiles | Must be classified before cleanup assumes only `debug`, `release`, `tmp`, and cross-target classes matter. |

## Gap Review

Known current gaps before implementation:

1. no-mistakes config still invokes raw Cargo.
2. Current active-process parser misses bundled and separated env-scrub forms such as `env -iuLD_PRELOAD cargo build` and `env -iu LD_PRELOAD cargo build`.
3. Current active-process parser misses `rustup run stable -- -- cargo build`; single and repeated end-of-options markers must be contrasted with `rustup run --install stable cargo build`.
4. `cmd_cargo` runs all subcommands under shared cache lock, including destructive `cargo clean`.
5. There is no dedicated `cache-clean` / `cache-reset` command with exclusive lock and active-process refusal.
6. Wrapper inventory and depth-cap observability are incomplete for shell aliases/functions/builtins, `command`, `exec`, `nohup`, `time`, `timeout`, `xargs`, `setsid`, `taskset`, `ionice`, `chrt`, `make`, Python command strings, and symlink-renamed tool binaries.
7. Worktree and `/private/tmp/bolt-v2-*` artifact lifecycle is not enforced.
8. Heavy managed cargo entry lacks a preflight gate for free disk, managed-cache size, and legacy roots.
9. Repo Cargo config, Cargo CLI `--config`, `cargo +TOOLCHAIN`, rustc wrapper env, shebang-executed Cargo scripts, `cargo install`, `CARGO_TARGET_TMPDIR`, `CARGO_INCREMENTAL`, `CARGO_INSTALL_ROOT`, artifact output overrides, and custom profiles need explicit test classification before implementation.

`cargo clean` note: this enumeration treats shared-lock `cargo clean` as an operational safety and stale-cache hazard. It does not claim Cargo lacks all internal locking; #374 still requires wrapper-level exclusive refusal so operators cannot reset the shared managed target while related managed work is active.

## Overlap Review

| Surface | Owner | Reason |
|---|---|---|
| managed cache status/prune, subtree retention | #286 | PR #404 owns `cache-status` / `cache-prune` policy and is closed. |
| raw cargo routing, shell/tool launcher coverage, no-mistakes Cargo drift | #374 | These are invocation-path and wrapper-hardening failures. |
| managed `cargo clean` overlap with managed build/test | #374 | It is wrapper/lifecycle safety, not retention policy. |
| Codex logs/sessions, factory logs, rustup toolchains | #375 | Developer-tool storage hygiene, not Cargo target routing. |
| cargo registry/git steady state, bolt-v3 runtime output, local CI artifacts | #376 | Uncovered surface inventory and caps. |
| unknown future disk classes | #377 | Detection layer, not known-class enforcement. |
| Claude/Codex temp `.output` spools | #125 / claude-config | Separate temp-output containment track. |

## T013/T014 Inputs

These are not implementation steps yet. They are the minimum red-test seams implied by this enumeration:

1. Parser/active-process tests for listed wrapper residuals, including bundled/separated `env -iu` forms, `rustup run` single and repeated `--` variants, `cargo +TOOLCHAIN`, shell aliases/functions/builtins, and `cargo install`.
2. Source-fence or verifier test that rejects raw Cargo in `.no-mistakes.yaml`.
3. Test that generic managed `cargo -- clean` refuses or is redirected to a dedicated exclusive path when active related processes exist.
4. Test that any proposed no-mistakes verification path uses managed commands or exact-head CI evidence, never a worktree-local target.
5. Test that S3 is rejected as an active mutable target cache.
6. Classification tests for `CARGO_TARGET_TMPDIR`, `CARGO_INCREMENTAL`, `CARGO_INSTALL_ROOT`, and `--out-dir` / `--artifact-dir` output overrides.

## Completion Criteria For T011/T012

T011/T012 can be checked only after:

1. This enumeration is reviewed for gaps and overlaps.
2. The live #374 body contains or links to the pinned enumeration, per #374 acceptance update.
3. External/adversarial reviewers agree it covers the newly added `cargo clean` residual.
4. No implementation code has been changed before T013/T014 RED.
