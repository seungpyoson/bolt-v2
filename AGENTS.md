# bolt-v2 Agent Rules

These repo-level rules are in addition to any higher-level agent instructions.

## Scope Discipline

- Follow the shared review discipline in `/Users/spson/Projects/Claude/docs/agent-review-discipline.md`.
- One branch or PR may cover only one declared issue, spec, task, or an explicitly named slice of one broader item.
- If a branch covers only a slice of a broader issue, the PR and review request must say so explicitly and name what accepted scope remains plus where it is tracked.
- Reviewers must flag out-of-scope changes, hidden adjacent issue work, and missing claimed scope as findings.
- Do not claim a PR closes a broader issue unless the diff actually satisfies that broader issue.

## Source Of Truth

- After a merge, `main` is authoritative.
- Old feature branches or worktrees become reference-only immediately after supersession or merge.
- Do not continue implementation from a stale branch or use it as proof that accepted scope is missing from `main`.
- If stale work is consulted for forensics, port only proven missing accepted scope onto a fresh clean branch from `main`.

## Existing Repo Rules

- No hardcodes for runtime values; use TOML config.
- No dual paths for config, secrets, or build flow.
- No credential display.
- Do not reference `~/Projects/Claude/bolt/`; use the pinned Nautilus source instead.
