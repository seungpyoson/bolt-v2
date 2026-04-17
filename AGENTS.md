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
6. **SSM IS THE SINGLE SECRET SOURCE** — all credentials resolve via `aws ssm get-parameter --with-decryption`. No 1Password CLI, no environment variable fallbacks, no other secret backends.
7. **GROUP BY CHANGE** — if swapping a wallet, credential set, or venue requires editing more than one config section, the config is wrong. All values that share a lifecycle belong in one section. Test: "if I change X, how many places do I touch?" The answer must be one.
8. **DO NOT REFERENCE BOLT V1** — `~/Projects/Claude/bolt/` is the old repo. Do not read from it, import from it, or depend on it. NT source is in the git cache at `~/.cargo/git/checkouts/nautilus_trader-*/` or on GitHub.
