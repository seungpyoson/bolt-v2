# bolt-v2 — Polymarket Trading System on NautilusTrader

## Rules

1. **NO HARDCODES** — every runtime value comes from TOML config. No string literals for IDs, quantities, timeouts, or any runtime value in code.
2. **NO DUAL PATHS** — one way to do each thing. One config format, one secret source, one build path.
3. **NO DEBTS** — no TODO, no "fix later", no unpinned dependencies, no uncommitted work.
4. **NO CREDENTIAL DISPLAY** — never cat/print/log API keys, private keys, secrets.
5. **PURE RUST BINARY** — no Python layer. The binary is a standalone Rust `LiveNode` using NT's Rust API directly. No PyO3, no maturin, no pip.
6. **SSM IS THE SINGLE SECRET SOURCE** — all credentials resolve from AWS SSM via the Rust AWS SDK (`aws-sdk-ssm`). No AWS CLI subprocess, no 1Password CLI, no environment variable fallbacks, no other secret backends.
7. **GROUP BY CHANGE** — if swapping a wallet, credential set, or venue requires editing more than one config section, the config is wrong. All values that share a lifecycle belong in one section. Test: "if I change X, how many places do I touch?" The answer must be one.
8. **DO NOT REFERENCE BOLT V1** — `~/Projects/Claude/bolt/` is the old repo. Do not read from it, import from it, or depend on it. NT source is in the git cache at `~/.cargo/git/checkouts/nautilus_trader-*/` or on GitHub.
9. **ONE BRANCH / PR = ONE DECLARED SCOPE** — every branch or PR must implement exactly one declared issue, spec, task, or an explicitly named slice of one broader item.
10. **STATE PARTIAL SCOPE EXPLICITLY** — if a branch implements only part of a broader issue, the PR/body/review request must say that plainly and name what accepted scope remains plus where it is tracked.
11. **REVIEWERS MUST FLAG SCOPE DRIFT** — out-of-scope changes, hidden extra issue work, and missing claimed scope are review findings, not optional observations.
12. **MAIN IS AUTHORITATIVE AFTER MERGE** — once work is merged, `main` is the source of truth. Old feature branches/worktrees become reference-only and must not be treated as proof that accepted scope is still missing.
13. **CHAINLINK DATA STREAMS: TESTNET IS PRODUCTION** — for the Chainlink Data Streams resolution oracle (the `price_to_beat` strike source feeding the binary-oracle taker), testnet is the ONLY and FINAL environment; mainnet/production Chainlink Data Streams credentials CANNOT be obtained (creds: SSM `/bolt/testnet/chainlink/{api-key,api-secret}`). Treat testnet AS production. NEVER raise testnet-vs-mainnet as a concern for it, NEVER hesitate/ask/re-confirm because it is "only testnet" or "could trade real money on mainnet," and NEVER treat it as a preliminary stage. The deployed config may read `environment = "mainnet"`, but the Chainlink strike resolves off testnet because that is all that exists. Standing authorization: deploy and run the live Chainlink strike on testnet. Real deploy concerns (config-schema compatibility, service health, fail-closed verification) remain fair game; the environment label does not.

## Review Scope Discipline

- Do not let a broad issue silently collapse into a narrower PR without saying so.
- Do not claim a PR closes a broader issue unless the diff actually satisfies that broader issue.
- If a stale branch still exists after a merge, use it only for forensics. Never continue implementation from it unless you first prove the accepted scope is absent from `main`.

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/014-disk-pressure-governance/plan.md`
<!-- SPECKIT END -->
