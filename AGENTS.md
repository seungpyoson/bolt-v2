# bolt-v2 Agent Rules

Repo governance for agents; higher-level standing instructions apply.

## Precedence & Source Of Truth

- Direct user instructions win unless they violate safety.
- `AGENTS.md` is the governance source and operational entrypoint. `CLAUDE.md`, `GEMINI.md`, SpecKit prompts, Superpowers skills, and plugin docs are adapters — if they conflict, follow `AGENTS.md` and report the drift.
- `.specify/memory/constitution.md` records SpecKit principles; update it only when governance changes those principles or gates. The active SpecKit plan is feature context, not governance.
- After a merge, `main` is authoritative; old branches, worktrees, and plan pointers become reference-only immediately after supersession or merge. Do not continue from stale work or cite it as proof that accepted scope is missing from `main`; port only proven-missing scope onto a fresh branch from `main`.

## Agent & Plugin Discipline

- Do not create per-agent policy docs unless the tool loads them and the policy cannot live in `AGENTS.md`. For tools that do not load it, pass `AGENTS.md` as read-only context; add `.specify/memory/constitution.md` only when SpecKit principles or gates matter.
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
6. **SSM IS THE SINGLE SECRET SOURCE** — product/runtime credentials resolve from AWS SSM via `aws-sdk-ssm`. No AWS CLI subprocess, no 1Password CLI, no environment variable fallbacks, no other secret backends. GitHub Actions repository automation may use GitHub's ephemeral `GITHUB_TOKEN` only for GitHub API operations; do not add alternate GitHub token names. `JULES_API_KEY` may live in GitHub Actions secrets only for repository code-maintenance advisory workflows; it is not a product/runtime/deploy/live/trading secret, not an alternate GitHub token, and must not be exposed to AWS, market data, order execution, runtime, deploy, or live jobs.
7. **GROUP BY CHANGE** — values that share a lifecycle live in one config section; a wallet, credential-set, or venue swap must require one edit.
8. **DO NOT REFERENCE BOLT V1** — do not read, import, or depend on `~/Projects/Claude/bolt/`. NT source is in `~/.cargo/git/checkouts/nautilus_trader-*/` or GitHub.
9. **STRATEGIES PRODUCE INTENT ONLY** — strategies emit order intent and strategy-local signal state only. Admissibility, venue rules, fillability, rounding, minimum size, fee-adjusted sizing, and submit gating live in shared NT-based execution/admission modules. Submit mechanics under `src/strategies/*` are rejected unless explicitly approved as strategy-local signal logic.
10. **CHAINLINK DATA STREAMS: TESTNET IS PRODUCTION** — for the `price_to_beat` oracle, testnet is the only final environment because mainnet credentials cannot be obtained. Treat the testnet Chainlink stream as production for this oracle; do not raise testnet-vs-mainnet as a concern or ask for reconfirmation solely because the stream is testnet. Still verify config-schema compatibility, service health, fail-closed behavior, and exact-head verification.
11. **PROVIDER/RUNTIME BOUNDARY EVIDENCE IS REGISTERED** — every deploy/readiness feeder that depends on provider runtime bytes or metadata must be represented in the authoritative boundary registry and guarded by source-fence evidence or an issue-bound, expiring non-WebSocket deferral. WebSocket-frame evidence must not be deferred.

## Evidence-Driven Verification

- TDD is allowed but not mandatory unless the user, active spec, or risk requires it; `.specify/memory/constitution.md` mirrors this principle.
- Agents do not wait on CI: push, report the head SHA, detach — verifying results at head belongs to the reviewer.
- Every claim must map to evidence: tests, static checks, source-fence results, remote CI, live artifacts, direct inspection, or explicit user-approved risk acceptance that does not violate a MUST rule. Every plan or task list must name the evidence for each changed requirement or risk: production behavior needs behavior/integration/remote-CI/live artifacts (or user-approved risk acceptance that does not violate a MUST rule); trading, admission, secrets, and config also need fail-closed evidence for invalid or missing inputs plus exact-head proof before any live operation; refactors need existing tests/static checks/source-fence or documented structural-equivalence review proving behavior is unchanged; documentation, prompt, template, and policy changes require targeted text/static checks plus internal adversarial review before completion claims.
- External review follows resolved local findings. Remote results are evidence to adjudicate, never merge authority.

## Approved Lean-CI End State And Cutover Boundary

- The selected end state has zero required CI status contexts. CI is visible evidence, not merge authority; merge authority is native code-owner approval, enforced by the `main` branch ruleset.
- `root-artifact` is a manual `workflow_dispatch` producer for exact-current-`main`. It runs no Cargo tests and grants no merge, install, launch, readiness, or trading authority; its build and verification steps live in the workflow, not here.
- The repository accepts that an approved merge may temporarily leave `main` red or broken. That accepted repository risk is never deploy or trading permission.
- Live arming belongs to one manifest-bound content-addressed immutable executable running its own finite `ops launch` pre-arm phase. Only complete success constructs the opaque, non-serializable, non-cloneable, one-use Rust `LiveReadinessPermit` that the sole Start entrypoint consumes by value. Installation and audit receipts are inert; every systemd start or restart obtains a fresh in-process permit.
- The trusted-App/protected-base verifier, precursor/activation/freeze ceremony, replay/tombstone control plane, and App-qualified merge authority previously proposed for #1016 are superseded and must not merge. #1016 owns architecture only; deletion and implementation slices require their own owning GitHub issue.
- No fallback, compatibility adapter, inherited result, alternate installer, mutable-copy route, tag/same-SHA/prior-artifact substitution, cache-as-proof, persisted authority, external readiness publisher, or dual path is allowed. Rollback is a pause or forward fix, never restoration of retired authority.
- Repository admission is limited to identity, pull-request state and mergeability, required-reviewer approval, and Mergify queue routing. CI workflows remain visible advisory evidence; `.mergify.yml` and the `main` ruleset own queue and branch-protection behavior. The live ruleset mutation is a separate operator action, verified directly after any governance change.

## Verification Flow

- Push the exact branch head with a plain `git push` (no credentials in push URLs) and open a PR. Advisory CI runs automatically; results are evidence to adjudicate, never merge authority. Do not wait on CI — push, report the head SHA, and detach; verifying at head is the reviewer's job.
- The CI jobs and their commands live in `.github/workflows/` and the justfile; the merge queue lives in `.mergify.yml`; branch protection lives in the `main` ruleset. Those files are the single source of truth — this document does not restate them, and nothing in the repo may mirror, parse, or validate `.mergify.yml`.
- Rust verification is remote-first as an economic default, not a fence: prefer advisory CI evidence over local compile-heavy runs; the commands are plain cargo and run anywhere.

## Evidence Fallback

- If GitHub Actions is unavailable, run the justfile's `fmt`, `clippy`, `test`, and `build` recipes on the designated EC2 host and post the raw command and output to the PR. No other fallback path.

## Test Rule (owner-ratified)

- Tests verify what Bolt does, never what source code looks like — no new source-scanning/structure tests; prefer compiler-enforced designs, then behavior tests.

## Rust Probe Policy

- Rust Probe (`rust-probe.yml`) is diagnostic only: use it when cheap local checks cannot answer a focused question. It never replaces advisory CI evidence and never authorizes or vetoes merge. Its dispatch inputs are defined in the workflow — this document does not restate them.
- Before dispatch, state changed files, suspected failure class, mode, target, and smallest-sufficient rationale. Limits: max 2 probe runs before stopping to explain root cause; Rust Probe success is not merge readiness; do not run full CI just to discover ordinary compiler errors.

## Review Bar & Merge Mechanics

- Every unique substantive issue is a finding regardless of severity; do not downgrade real issues into notes or treat tracked as resolved unless fixed or explicitly waived.
- Keep PR bodies stable: use them for lasting scope/behavior disclosures and timeless merge requirements, not the current head SHA, transient CI/check status, or head-specific review/verification receipts. Put exact-head evidence in review requests, comments/records, or check runs; do not rewrite the body as heads move. This PR-body status rule does not restrict immutable release artifacts or spec anchors.
- Before completing or merging coding work, open a PR and request review from the GitHub account with node ID `U_kgDOEZMFhA` (login-based — resolve the node ID to its current login and keep `.github/CODEOWNERS` aligned). This required-reviewer node ID is an intentional hardcoded policy constant for native merge governance: PR-editable config must not select the required reviewer.
- Do not merge, squash, or rebase-merge until the PR has approval from node ID `U_kgDOEZMFhA`; if review cannot be requested, stop and report. Branch protection is enforced by the `main` ruleset — the source of truth; verify it live rather than restating its settings here, and if a required control is missing, stop and report instead of treating CI checks as merge controls.
- Do not request review with uncommitted changes, unpushed commits, unresolved findings, or unanswered comments. Reply to and resolve every applicable review thread; commit and push any fix before further review discussion. Advisory failures are evidence to adjudicate rather than automatic review vetoes.
- Never bypass native merge controls with `gh pr merge --admin`. On a stale native-control block, force recompute by push, review, close/reopen, or waiting.

## Response Format

- Keep responses concise by default; prefer short direct answers over long explanations unless depth is requested.
