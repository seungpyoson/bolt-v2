# bolt-v2 Agent Rules

These repo-level rules are in addition to any higher-level agent instructions.

## Scope Discipline

- One branch or PR may cover only one declared issue, spec, task, or an explicitly named slice of one broader item.
- If a branch covers only a slice of a broader issue, the PR and review request must say so explicitly and name what accepted scope remains plus where it is tracked.
- Reviewers must flag out-of-scope changes, hidden adjacent issue work, and missing claimed scope as findings.
- Do not claim a PR closes a broader issue unless the diff actually satisfies that broader issue.

## Source Of Truth

- After a merge, `main` is authoritative.
- Old feature branches or worktrees become reference-only immediately after supersession or merge.
- Do not continue implementation from a stale branch or use it as proof that accepted scope is missing from `main`.
- If stale work is consulted for forensics, port only proven missing accepted scope onto a fresh clean branch from `main`.

## Repo Rules

1. **NO HARDCODES** — every runtime value comes from TOML config. No string literals for IDs, quantities, timeouts, or any runtime value in code.
2. **NO DUAL PATHS** — one way to do each thing. One config format, one secret source, one build path.
3. **NO DEBTS** — no TODO, no "fix later", no unpinned dependencies, no uncommitted work.
4. **NO CREDENTIAL DISPLAY** — never cat/print/log API keys, private keys, secrets.
5. **PURE RUST BINARY** — no Python layer. The binary is a standalone Rust `LiveNode` using NT's Rust API directly. No PyO3, no maturin, no pip.
6. **SSM IS THE SINGLE SECRET SOURCE** — all credentials resolve from AWS SSM via the Rust AWS SDK (`aws-sdk-ssm`). No AWS CLI subprocess, no 1Password CLI, no environment variable fallbacks, no other secret backends.
7. **GROUP BY CHANGE** — if swapping a wallet, credential set, or venue requires editing more than one config section, the config is wrong. All values that share a lifecycle belong in one section. Test: "if I change X, how many places do I touch?" The answer must be one.
8. **DO NOT REFERENCE BOLT V1** — `~/Projects/Claude/bolt/` is the old repo. Do not read from it, import from it, or depend on it. NT source is in the git cache at `~/.cargo/git/checkouts/nautilus_trader-*/` or on GitHub.
9. **STRATEGIES PRODUCE INTENT ONLY** — strategy files may produce order intent and strategy-local signal state only. Execution admissibility, venue rules, fillability, rounding, minimum order size, fee-adjusted sizing, and submit gating must live in shared execution/admission modules built on NT APIs. Any change under `src/strategies/*` that handles submit mechanics is rejected unless explicitly approved as strategy-local signal logic.

## Remote-First Rust Verification

- Do not run local compile-heavy Rust verification by default: no local managed Rust test/build/clippy recipes through `just`, and no raw cargo subcommands refused by `ci/rust-verification.toml` `[local_compile_policy]`.
- Use local non-compile gates for fast feedback: `just fmt-check`, `just deny`, `just ci-lint-workflow`, Python verifiers, and `just source-fence-static`.
- For compile/test/clippy proof: commit, push, ensure the branch has an open or draft PR, then run `just verify-remote` and use exact-head PR CI evidence from Ubicloud/GitHub Actions.
- `just verify-remote` waits for all reported PR checks on the exact head SHA, not a local subset of workflow jobs.
- Human operator break-glass exists for exceptional local repro and live/operator lanes only. Agents must not use it as a normal verification path.
- Enforcement boundary: repo tooling gates cooperative paths through `just`, `scripts/rust_verification.py`, and `.no-mistakes.yaml`; standard PATH `cargo ...` is guarded by the machine-level cargo shim, whose source and installer are tracked in this repo at `scripts/cargo-shim` and `scripts/install-cargo-shim`. The shim reads this repo's `ci/rust-verification.toml` `[local_compile_policy]`.
- Residual local bypasses remain outside the accidental-use guard: absolute-path cargo invocation, `rustup run <toolchain> cargo ...`, cross-repo invocations issued outside this repo such as `cargo --manifest-path <repo>/Cargo.toml ...` or `cargo -C <repo> ...`, already-running daemons with old environments, non-no-mistakes daemon managers whose `PATH` does not include the shim directory, processes that never load shell startup files, and direct `rustc` execution.

## Review Bar

- Every unique substantive issue counts as a finding regardless of severity. Do not downgrade real issues into “just notes” or treat “tracked” as “resolved” unless the finding is actually fixed or the user explicitly waives it.
- Do not ask for or frame external red-team review while the branch has uncommitted changes, unpushed commits, unresolved findings, unanswered review comments, or failing checks.
- Do not ask for external review until the exact PR head's CI is confirmed green.
- If the only remaining local delta is a fix or cleanup already made locally, commit and push it before further review discussion instead of pausing in a half-finished state.

## Response Format

- Keep responses concise by default.
- Prefer short direct answers over broad explanations.
- Do not write long multi-paragraph replies unless the user explicitly asks for depth.
- If one short paragraph or a few flat bullets is enough, use that.

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/023-nt-order-intent-layer/plan.md`
<!-- SPECKIT END -->
