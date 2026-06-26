# bolt-v2 Agent Rules

Repo rules for agents; higher-level instructions apply; deeper `AGENTS.md` wins for subtrees.

## Instruction Precedence And Sources

- Direct user instructions win unless they violate safety.
- `AGENTS.md` is the repo governance source and operational entrypoint.
- `.specify/memory/constitution.md` records SpecKit principles; update it only when governance changes SpecKit principles/gates.
- `CLAUDE.md`, `GEMINI.md`, SpecKit prompts, Superpowers skills, and plugin docs are tool adapters. If they conflict, follow `AGENTS.md` and report drift.
- The active SpecKit plan is feature context, not governance. After merge, `main` overrides stale branches, worktrees, and plan pointers.

## Agent And Plugin Discipline

- Do not create per-agent policy docs unless the tool loads them and the policy cannot live in `AGENTS.md`.
- For tools that do not load `AGENTS.md`, pass `AGENTS.md` as read-only context. Add `.specify/memory/constitution.md` only when SpecKit principles or gates matter.
- `.pr_agent.toml` mirrors the critical AI-review subset for PR-Agent, which cannot load arbitrary repo files in GitHub Actions. Keep that mirror current with this file; `scripts/verify_ai_review_governance.py` checks the mirror in CI.
- SpecKit and Superpowers are plugins. Generated prompts may recommend strict TDD; use Evidence-Driven Verification unless the user, active spec, or risk requires TDD.
- Do not patch plugin caches as durable fixes. Use repo governance, SpecKit templates, verified extension/override surfaces, or regenerated adapters.

## Scope Discipline

- One branch or PR may cover only one declared issue, spec, task, or an explicitly named slice of one broader item.
- Slice PRs must name remaining accepted scope and where it is tracked.
- Reviewers must flag out-of-scope changes, hidden adjacent issue work, and missing claimed scope as findings.
- Do not claim a PR closes a broader issue unless the diff actually satisfies that broader issue.

## Source Of Truth

- After a merge, `main` is authoritative.
- Old feature branches or worktrees become reference-only immediately after supersession or merge.
- Do not continue from stale branches or use them as proof that accepted scope is missing from `main`.
- If stale work is consulted, port only proven missing accepted scope onto a fresh branch from `main`.

## Repo Rules

1. **NO HARDCODES** — every runtime value comes from TOML config. No string literals for IDs, quantities, timeouts, or any runtime value in code.
2. **NO DUAL PATHS** — one way to do each thing. One config format, one secret source, one build path.
3. **NO DEBTS** — no TODO, no "fix later", no unpinned dependencies, no uncommitted work.
4. **NO CREDENTIAL DISPLAY** — never cat/print/log API keys, private keys, secrets.
5. **PURE RUST BINARY** — standalone Rust `LiveNode` using NT's Rust API directly. No Python layer, PyO3, maturin, or pip.
6. **SSM IS THE SINGLE SECRET SOURCE** — all credentials resolve from AWS SSM via the Rust AWS SDK (`aws-sdk-ssm`). No AWS CLI subprocess, no 1Password CLI, no environment variable fallbacks, no other secret backends.
7. **GROUP BY CHANGE** — values that share a lifecycle belong in one config section. Wallet, credential-set, or venue swaps must require one edit.
8. **DO NOT REFERENCE BOLT V1** — do not read, import, or depend on `~/Projects/Claude/bolt/`. NT source is in `~/.cargo/git/checkouts/nautilus_trader-*/` or GitHub.
9. **STRATEGIES PRODUCE INTENT ONLY** — strategies may produce order intent and strategy-local signal state only. Execution admissibility, venue rules, fillability, rounding, minimum size, fee-adjusted sizing, and submit gating live in shared NT-based execution/admission modules. Submit mechanics under `src/strategies/*` are rejected unless explicitly approved as strategy-local signal logic.
10. **CHAINLINK DATA STREAMS: TESTNET IS PRODUCTION** — for the Chainlink Data Streams `price_to_beat` oracle, testnet is the only final environment. Do not raise testnet-vs-mainnet as a concern solely because the stream is testnet; still verify config-schema compatibility, service health, fail-closed behavior, and exact head.

## Evidence-Driven Verification

- `AGENTS.md` owns workflow; `.specify/memory/constitution.md` mirrors the SpecKit principle.
- TDD is allowed but not mandatory unless the user, active spec, or risk requires it.
- Every claim must map to evidence: tests, static checks, source-fence results, remote CI, live artifacts, direct inspection, or explicit user-approved risk acceptance that does not violate a MUST rule.
- Every plan or task list must name evidence: behavior tests/integration/remote CI/live artifacts for production; fail-closed invalid-input plus exact-head proof for trading, admission, secrets, and config; existing tests/static checks/source-fence/structural equivalence for refactors; targeted text/static checks plus internal adversarial review for documentation, prompt, template, and policy changes.
- External review: only after local findings are resolved and exact-head CI or the user-approved equivalent is green.

## Remote-First Rust Verification

- Do not run local compile-heavy Rust verification by default: no local managed Rust test/build/clippy recipes through `just`, and no raw cargo subcommands refused by `ci/rust-verification.toml` `[local_compile_policy]`.
- Use local non-compile gates for fast feedback: `just fmt-check`, `just deny`, `just ci-lint-workflow`, Python verifiers, and `just source-fence-static`. Use the public `just` recipes; do not invoke `*-inner` local-verification recipes directly.
- For draft PR Rust feedback, commit, push, open a draft PR, then run `just verify-remote` for exact-head Ubicloud/GitHub Actions feedback.
- For merge proof, mark the PR ready and run `just verify-remote` for the required exact-head PR gate, or use the merge queue gate.
- Default to draft PRs while iterating; mark ready only for the merge candidate. Draft pushes defer full-CI merge proof and cannot merge.
- Human operator break-glass is only for exceptional local repro and live/operator lanes. Agents must not use it as a normal verification path.
- Cooperative paths are gated through `just`, `scripts/rust_verification.py`, `.no-mistakes.yaml`, and the PATH cargo shim in `scripts/cargo-shim` and `scripts/install-cargo-shim`.
- CPU-heavy local verifier lanes self-serialize via `ci/rust-verification.toml` `[local_lane_policy]`; broad gates acquire the lane once, competing gates fail fast, CI bypasses the lock, and coverage drift fails `source-fence-static`.
- Known bypasses: absolute-path cargo, `rustup run ... cargo`, cross-repo cargo, old daemons, non-shim PATHs, startup-skipping shells, direct `rustc`.

## Rust Probe Policy

- Ready PR or merge-queue full CI is proof; draft workflow_dispatch full CI is feedback; Rust Probe is debugging.
- Agents may use Rust Probe only when cheap local checks cannot answer the question.
- Use `just rust-probe suggest` before dispatching a probe to choose the smallest targeted remote Rust debugging command.
- Dispatch `just rust-probe ...` only from a clean named branch whose `HEAD` is pushed upstream; dispatch modes use the exact pushed SHA and refuse unsafe local state.
- Before Rust Probe, state changed files, suspected failure class, selected mode, selected target/name, and smallest-sufficient rationale.
- Limits: max 2 Rust Probe runs before stopping to explain root cause; full CI may run only after the slice is coherent; Rust Probe success is not merge readiness; Rust Probe must not replace the required `gate`; do not run full CI just to discover ordinary compiler errors.

## Review Bar

- Every unique substantive issue counts as a finding regardless of severity. Do not downgrade real issues into notes or treat tracked as resolved unless fixed or explicitly waived.
- Before marking coding work complete or attempting to merge, coding agents must open a PR and request review from the GitHub account with node ID `U_kgDOEZMFhA`.
- GitHub review requests are login-based; resolve node ID `U_kgDOEZMFhA` to the account's current login before requesting review.
- The required reviewer node ID is an intentional hardcoded policy constant for native merge governance because PR-editable config must not select the required reviewer.
- `.github/CODEOWNERS` is login-based; keep it aligned with the current login for node ID `U_kgDOEZMFhA`.
- The `main` ruleset must require native code-owner review, stale-review dismissal, last-push approval, and review-thread resolution. If those controls are missing, stop and report the blocker instead of treating CI checks as merge controls.
- Agents must not merge, squash, rebase-merge, or otherwise land code until the PR has approval from GitHub node ID `U_kgDOEZMFhA`.
- If review from GitHub node ID `U_kgDOEZMFhA` cannot be requested, stop and report the blocker.
- Before marking review feedback addressed, reply to and resolve every applicable GitHub review thread; give the technical reason for any inapplicable thread.
- Do not request or frame external review with uncommitted changes, unpushed commits, unresolved findings, unanswered review comments, or failing checks.
- Do not ask for external review until the exact PR head's CI is green.
- If the only remaining local delta is an already-made fix or cleanup, commit and push before further review discussion.

## Merge Mechanics

- List active `main` rules with `gh api repos/{owner}/{repo}/rules/branches/main`.
- Before merging, verify each active rule is satisfied: checks green in `gh pr view <n> --json statusCheckRollup` and approvals met.
- If rules pass but GitHub reports a stale block, force recompute by push, review, close/reopen, or waiting; then retry.
- Never force past a green-but-cached block with `gh pr merge --admin`; that bypasses required checks.

<!-- SPECKIT START -->
`specs/026-nt-backed-iv-engine/plan.md`
<!-- SPECKIT END -->
