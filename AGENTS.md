# bolt-v2 Agent Rules

These repo-level rules are in addition to any higher-level agent instructions.

## Instruction Precedence And Sources

- Direct user instructions for the current turn win unless they would violate safety.
- This `AGENTS.md` is the repo governance source and shared operational entrypoint for coding agents.
- `.specify/memory/constitution.md` is the SpecKit project-principles artifact. Update it when a governance change also changes SpecKit principles or gates, but do not use it as the primary agent workflow document.
- `CLAUDE.md`, `GEMINI.md`, generated SpecKit adapter prompts, Superpowers skills, and other plugin docs are lower-priority tool adapters. If they conflict with this file, follow `AGENTS.md`, then report the drift.
- The active SpecKit plan is feature context, not governance. Stale feature branches, stale worktrees, or stale plan pointers do not override `main`.

## Agent And Plugin Discipline

- Do not create new per-agent policy documents unless the target tool is verified to load them and the same policy cannot live in `AGENTS.md`.
- For tools that do not automatically load `AGENTS.md`, explicitly provide `AGENTS.md` as read-only context when launching them. Include `.specify/memory/constitution.md` only when SpecKit gates or project principles are relevant.
- SpecKit and Superpowers are plugins. Their generated prompts may recommend strict TDD; in this repo, use the evidence-driven verification policy below unless the user, active spec, or risk analysis explicitly requires TDD.
- Do not patch plugin caches as a durable repo fix. Prefer repo governance, SpecKit templates, verified extension/override surfaces, or regenerated adapters.
- Known generated-adapter drift: current SpecKit implement prompts may still say to follow TDD. Treat that as lower-priority generated guidance, not as repo policy.

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
10. **CHAINLINK DATA STREAMS: TESTNET IS PRODUCTION** — for the Chainlink Data Streams resolution oracle (`price_to_beat` for the binary-oracle taker), testnet is the only and final environment because mainnet credentials cannot be obtained. Treat the testnet Chainlink stream as production for this oracle. Do not raise testnet-vs-mainnet as a concern or ask for reconfirmation solely because the stream is testnet. Real deploy concerns still apply: config-schema compatibility, service health, fail-closed behavior, and exact-head verification.

## Evidence-Driven Verification

- Follow the evidence-driven verification rule in this file. `.specify/memory/constitution.md` records the matching SpecKit principle, but `AGENTS.md` owns the operational workflow.
- TDD is allowed and often useful, but it is not mandatory unless the user, active spec, or risk analysis requires it.
- Every claim must map to evidence: tests, static checks, source-fence results, remote CI, live artifacts, direct inspection, or explicit user-approved risk acceptance that does not violate a MUST rule.
- Documentation, prompt, template, and policy changes require targeted text/static checks plus internal adversarial review before completion claims.
- Every plan or task list must state the evidence class for each changed requirement or risk:
  - Production behavior: automated behavior test, integration proof, remote CI result, live artifact, or explicit user-approved risk acceptance that does not violate a MUST rule.
  - Trading, admission, secrets, and config changes: fail-closed evidence for invalid or missing inputs plus exact-head proof before any live operation.
  - Refactors: existing tests, static checks, source-fence checks, or documented structural equivalence review proving behavior is unchanged.
  - Documentation, prompt, template, and policy changes: targeted text/static checks and internal adversarial review.
  - External review: only after local findings are resolved and exact-head CI or the user-approved equivalent is green.
- For agents/tools that do not automatically load this file, pass `AGENTS.md` as read-only launch context rather than creating another policy document. Add the SpecKit constitution only when the task needs SpecKit principle context.

## Remote-First Rust Verification

- Do not run local compile-heavy Rust verification by default: no local managed Rust test/build/clippy recipes through `just`, and no raw cargo subcommands refused by `ci/rust-verification.toml` `[local_compile_policy]`.
- Use local non-compile gates for fast feedback: `just fmt-check`, `just deny`, `just ci-lint-workflow`, Python verifiers, and `just source-fence-static`. Use the public `just` recipes; do not invoke `*-inner` local-verification recipes directly.
- For compile/test/clippy proof: commit, push, ensure the branch has an open or draft PR, then run `just verify-remote` and use exact-head PR CI evidence from Ubicloud/GitHub Actions.
- `just verify-remote` waits for all reported PR checks on the exact head SHA, not a local subset of workflow jobs.
- Human operator break-glass exists for exceptional local repro and live/operator lanes only. Agents must not use it as a normal verification path.
- Enforcement boundary: repo tooling gates cooperative paths through `just`, `scripts/rust_verification.py`, and `.no-mistakes.yaml`; standard PATH `cargo ...` is guarded by the machine-level cargo shim, whose source and installer are tracked in this repo at `scripts/cargo-shim` and `scripts/install-cargo-shim`. The shim reads this repo's `ci/rust-verification.toml` `[local_compile_policy]`.
- CPU-heavy local verifier lanes self-serialize: every `scripts/verify_*.py` / `scripts/test_*.py` entry point acquires the per-repo machine-level lane lock declared in `ci/rust-verification.toml` `[local_lane_policy]` before doing work. Broad public local gates (`just fmt-check`, `just source-fence-static`, and `just ci-lint-workflow`) acquire that lane once through `scripts/local_verification_gate.py`; a competing public gate fails fast with the active holder instead of launching duplicate verifier work. Child verifier scripts whose holder is a process ancestor pass through. Direct lower-level verifier runs still acquire the lane themselves. CI (`allowed_ci_env`) bypasses the lock. Coverage drift is a CI failure via `scripts/verify_lane_governance.py` in `source-fence-static`.
- Residual local bypasses remain outside the accidental-use guard: absolute-path cargo invocation, `rustup run <toolchain> cargo ...`, cross-repo invocations issued outside this repo such as `cargo --manifest-path <repo>/Cargo.toml ...` or `cargo -C <repo> ...`, already-running daemons with old environments, non-no-mistakes daemon managers whose `PATH` does not include the shim directory, processes that never load shell startup files, and direct `rustc` execution.

## Rust Probe Policy

- Full CI is proof. Rust Probe is debugging.
- Agents may use Rust Probe only when cheap local checks cannot answer the question.
- Use `just rust-probe suggest` before dispatching a probe to choose the smallest targeted remote Rust debugging command.
- Use dispatching `just rust-probe ...` modes only from a clean named branch whose `HEAD` is pushed to its upstream. Those modes dispatch the exact pushed SHA to GitHub Actions/Ubicloud and refuse unsafe local state.
- Before running Rust Probe, state: (1) changed files, (2) suspected failure class, (3) selected mode, (4) selected test target/name, (5) why this is the smallest sufficient probe.
- Limits: max 2 Rust Probe runs before stopping to explain root cause; full CI may run only after the slice is coherent; Rust Probe success is not merge readiness; Rust Probe must not replace the required `gate`; do not run full CI just to discover ordinary compiler errors.

## Review Bar

- Every unique substantive issue counts as a finding regardless of severity. Do not downgrade real issues into “just notes” or treat “tracked” as “resolved” unless the finding is actually fixed or the user explicitly waives it.
- Before marking coding work complete or attempting to merge, coding agents must open a PR and request review from the GitHub account with node ID `U_kgDOEZMFhA`.
- GitHub review requests are login-based; resolve node ID `U_kgDOEZMFhA` to the account's current login before requesting review.
- The required reviewer node ID is an intentional hardcoded policy constant in the approval gate because PR-editable config must not select the required reviewer.
- `.github/CODEOWNERS` is login-based; keep it aligned with the current login for node ID `U_kgDOEZMFhA`.
- The `main` ruleset must require the `required reviewer approved` commit-status context, code-owner review, stale-review dismissal, last-push approval, and native review-thread resolution. If those controls are missing, stop and report the blocker instead of treating the CI checks as merge controls.
- Agents must not merge, squash, rebase-merge, or otherwise land code until the PR has approval from GitHub node ID `U_kgDOEZMFhA`.
- If review from GitHub node ID `U_kgDOEZMFhA` cannot be requested, stop and report the blocker.
- Before marking review feedback addressed, coding agents must reply to and resolve every applicable GitHub review thread. If a thread is not applicable, reply with the technical reason before marking work complete.
- Do not ask for or frame external red-team review while the branch has uncommitted changes, unpushed commits, unresolved findings, unanswered review comments, or failing checks.
- Do not ask for external review until the exact PR head's CI is confirmed green.
- If the only remaining local delta is a fix or cleanup already made locally, commit and push it before further review discussion instead of pausing in a half-finished state.

## Merge Mechanics

- A repo ruleset on `main` (`required_status_checks` plus any review rules) gates merges. A merge refused with "base branch policy prohibits the merge" while every active rule actually passes is usually GitHub's stale merge-state cache, not a real block: GitHub recomputes a PR's mergeability on PR events (push, review, close/reopen) and a periodic background pass, not immediately when a ruleset is edited — so a just-fixed rule can keep serving the old BLOCKED verdict.
- List the active rules with `gh api repos/{owner}/{repo}/rules/branches/main` (`gh` fills `{owner}`/`{repo}` from the current repo), then verify each is actually satisfied — required checks green in the PR status rollup (`gh pr view <n> --json statusCheckRollup`) and any required approvals met. If they are, force a recompute (push, a review, or close/reopen) or wait for GitHub's background pass, then retry the merge.
- Never force past a green-but-cached block with `gh pr merge --admin`; that bypasses the required checks.

## Response Format

- Keep responses concise by default.
- Prefer short direct answers over broad explanations.
- Do not write long multi-paragraph replies unless the user explicitly asks for depth.
- If one short paragraph or a few flat bullets is enough, use that.

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/026-nt-backed-iv-engine/plan.md`
<!-- SPECKIT END -->
