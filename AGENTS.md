# bolt-v2 Agent Rules

Repo governance for agents; higher-level standing instructions apply.

## Precedence & Source Of Truth

- Direct user instructions win unless they violate safety.
- `AGENTS.md` is the governance source and operational entrypoint. `CLAUDE.md`, `GEMINI.md`, SpecKit prompts, Superpowers skills, and plugin docs are adapters — if they conflict, follow `AGENTS.md` and report the drift.
- `.specify/memory/constitution.md` records SpecKit principles; update it only when governance changes those principles or gates. The active SpecKit plan is feature context, not governance.
- After a merge, `main` is authoritative; old branches, worktrees, and plan pointers become reference-only immediately after supersession or merge. Do not continue from stale work or cite it as proof that accepted scope is missing from `main`; port only proven-missing scope onto a fresh branch from `main`.

## Agent & Plugin Discipline

- Do not create per-agent policy docs unless the tool loads them and the policy cannot live in `AGENTS.md`. For tools that do not load it, pass `AGENTS.md` as read-only context; add `.specify/memory/constitution.md` only when SpecKit principles or gates matter.
- `.pr_agent.toml` mirrors the critical AI-review subset for PR-Agent, which cannot load arbitrary repo files in GitHub Actions. Keep that mirror current with this file; `scripts/verify_ai_review_governance.py` checks the mirror in CI.
- AI review deliverables must identify the reviewer source and exact configured model. Runtime source/model labels and comment markers come from `ci/ai-review.toml`; workflow and prompt text must not embed those runtime values.
- Do not patch plugin caches as durable fixes; use repo governance, SpecKit templates, verified extension/override surfaces, or regenerated adapters. Generated prompts may recommend strict TDD — use Evidence-Driven Verification unless the user, active spec, or risk requires TDD.

## Scope Discipline

- One branch or PR may cover only one declared issue, spec, task, or an explicitly named slice of one broader item; slice PRs and their review requests must name remaining accepted scope and where it is tracked.
- Reviewers must flag out-of-scope changes, hidden adjacent issue work, and missing claimed scope as findings. Do not claim a PR closes a broader issue unless the diff actually satisfies it.

## Repo Rules

1. **NO HARDCODES** — every runtime value comes from TOML config. No string literals for IDs, quantities, timeouts, or any runtime value in code.
2. **NO DUAL PATHS** — one way to do each thing: one config format, one secret source, one build path.
3. **NO DEBTS** — no TODO, no "fix later", no unpinned dependencies, no uncommitted work.
4. **NO CREDENTIAL DISPLAY** — never cat/print/log API keys, private keys, secrets.
5. **PURE RUST BINARY** — standalone Rust `LiveNode` using NT's Rust API directly. No Python layer, PyO3, maturin, or pip.
6. **SSM IS THE SINGLE SECRET SOURCE** — all credentials resolve from AWS SSM via `aws-sdk-ssm`. No AWS CLI subprocess, no 1Password CLI, no environment variable fallbacks, no other secret backends.
7. **GROUP BY CHANGE** — values that share a lifecycle live in one config section; a wallet, credential-set, or venue swap must require one edit.
8. **DO NOT REFERENCE BOLT V1** — do not read, import, or depend on `~/Projects/Claude/bolt/`. NT source is in `~/.cargo/git/checkouts/nautilus_trader-*/` or GitHub.
9. **STRATEGIES PRODUCE INTENT ONLY** — strategies emit order intent and strategy-local signal state only. Admissibility, venue rules, fillability, rounding, minimum size, fee-adjusted sizing, and submit gating live in shared NT-based execution/admission modules. Submit mechanics under `src/strategies/*` are rejected unless explicitly approved as strategy-local signal logic.
10. **CHAINLINK DATA STREAMS: TESTNET IS PRODUCTION** — for the `price_to_beat` oracle, testnet is the only final environment because mainnet credentials cannot be obtained. Treat the testnet Chainlink stream as production for this oracle; do not raise testnet-vs-mainnet as a concern or ask for reconfirmation solely because the stream is testnet. Still verify config-schema compatibility, service health, fail-closed behavior, and exact-head verification.

## Evidence-Driven Verification

- TDD is allowed but not mandatory unless the user, active spec, or risk requires it; `.specify/memory/constitution.md` mirrors this principle.
- Every claim must map to evidence: tests, static checks, source-fence results, remote CI, live artifacts, direct inspection, or explicit user-approved risk acceptance that does not violate a MUST rule. Every plan or task list must name the evidence for each changed requirement or risk: production behavior needs behavior/integration/remote-CI/live artifacts (or user-approved risk acceptance that does not violate a MUST rule); trading, admission, secrets, and config also need fail-closed evidence for invalid or missing inputs plus exact-head proof before any live operation; refactors need existing tests/static checks/source-fence or documented structural-equivalence review proving behavior is unchanged; documentation, prompt, template, and policy changes require targeted text/static checks plus internal adversarial review before completion claims.
- External review: only after local findings are resolved and exact-head CI or the user-approved equivalent is green.

## Remote-First Rust Verification

- Do not run local compile-heavy Rust verification by default: no managed `just` Rust test/build/clippy recipes, no raw cargo refused by `ci/rust-verification.toml` `[local_compile_policy]`. Use local non-compile gates for fast feedback: `just fmt-check`, `just deny`, `just ci-lint-workflow`, Python verifiers, and `just source-fence-static` (public recipes only, never `*-inner`).
- Default to draft PRs while iterating; for Rust feedback, commit, push, open a draft PR, then run `just verify-remote` for exact-head Ubicloud/GitHub Actions. Draft pushes defer full-CI merge proof (clippy/deny still run) and cannot merge. For merge proof, mark the PR ready and run `just verify-remote` for the required exact-head PR gate, or use the merge queue gate (see [Operator Policy](docs/ci/ubicloud-cost-governance.md#operator-policy)). Operator break-glass is for exceptional local repro and live/operator lanes only, never a normal agent path.
- Cooperative paths are gated through `just`, `scripts/rust_verification.py`, `.no-mistakes.yaml`, and the PATH cargo shim (`scripts/cargo-shim`, `scripts/install-cargo-shim`), which reads `[local_compile_policy]`. Lanes self-serialize via `[local_lane_policy]`: verifier entry points and broad gates acquire the lane through `scripts/local_verification_gate.py` (broad gates once, competing gates fail fast), CI (`allowed_ci_env`) bypasses the lock, and coverage drift fails `source-fence-static` via `scripts/verify_lane_governance.py`. Known bypasses: absolute-path cargo, `rustup run ... cargo`, cross-repo cargo, old daemons, non-shim PATHs, startup-skipping shells, direct `rustc`.

## Rust Probe Policy

- Ready-PR or merge-queue full CI is proof; draft `workflow_dispatch` full CI is feedback; Rust Probe is debugging — use it only when cheap local checks cannot answer the question, and never to replace the required `gate`.
- Run `just rust-probe suggest` first; dispatch `just rust-probe ...` only from a clean named branch whose pushed `HEAD` SHA is used (dispatch refuses unsafe local state). Before dispatch, state changed files, suspected failure class, mode, target, and smallest-sufficient rationale. Limits: max 2 probe runs before stopping to explain root cause; full CI may run only after the slice is coherent; Rust Probe success is not merge readiness; do not run full CI just to discover ordinary compiler errors.
- Suggested integration-test probes use the Cargo `[[test]]` harness as `test_target`; when a changed file is a harness member module, the suggested `test_name` is `<member_stem>::` so nextest stays scoped to that module.

## Review Bar & Merge Mechanics

- Every unique substantive issue is a finding regardless of severity; do not downgrade real issues into notes or treat tracked as resolved unless fixed or explicitly waived.
- Before completing or merging coding work, open a PR and request review from the GitHub account with node ID `U_kgDOEZMFhA` (login-based — resolve the node ID to its current login and keep `.github/CODEOWNERS` aligned). This required-reviewer node ID is an intentional hardcoded policy constant for native merge governance: PR-editable config must not select the required reviewer.
- The `main` ruleset must require native code-owner review, stale-review dismissal, last-push approval, and review-thread resolution; if those are missing, stop and report the blocker instead of treating CI checks as merge controls. Agents must not merge, squash, rebase-merge, or otherwise land code until the PR has approval from node ID `U_kgDOEZMFhA`; if review cannot be requested, stop and report.
- Do not request external review with uncommitted changes, unpushed commits, unresolved findings, unanswered comments, or non-green exact-head CI. Reply to and resolve every applicable review thread (give the technical reason for any inapplicable one); commit and push any remaining fix before further review discussion.
- Verify each active `main` rule before merge: list with `gh api repos/{owner}/{repo}/rules/branches/main`, confirm checks green in `gh pr view <n> --json statusCheckRollup` and approvals met. On a stale block, force recompute by push, review, close/reopen, or waiting; never force past a green-but-cached block with `gh pr merge --admin`.

## Response Format

- Keep responses concise by default; prefer short direct answers over long explanations unless depth is requested.
