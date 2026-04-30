# Governance Proof Tooling Lane

## Purpose

This branch is the clean `main`-based lane for repo governance, proof discipline, CI evidence, and NT pointer-probe tooling work.

It exists to consolidate related proof/tooling work without mixing in trading runtime behavior or Bolt-v3 feature architecture.

## Source Branch

- Base: `origin/main`
- Initial branch: `codex/governance-proof-tooling-lane`

## In Scope

- Restore the repo-level Bolt-v2 proof matrix as a process artifact.
- Rebuild valid CI/proof work from older PRs only when it still applies to current `main`.
- Evaluate NT pointer-probe dry-run engine / evidence artifact work from `issue-163-nt-pointer-probe-engine`; port it only if it matches the current control-plane design.
- Add clean graphify hook/config integration from `issue-573-graphify-install` without generated graph artifacts or stale rule rewrites.

## Deferred From This Pass

- `issue-163-nt-pointer-probe-engine`: evaluated against current `main`, not ported. The old branch's library files compile only after moving `tempfile` into normal dependencies, and the dry-run tests then require a `dry-run` CLI subcommand plus fixture/control-plane behavior that no longer matches current `main`. That makes it a separate adaptation slice, not a safe consolidation replay.
- `issue-573-graphify-install`: partially ported. This branch keeps the hook/config integration and ignores `graphify-out/`, but does not replay generated graph output, vendored skill files, or stale edits to `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md`.

## Graphify Rule

Do not replay `issue-573-graphify-install` as-is.

That old branch mixed generated `graphify-out/` artifacts, editor/agent hook config, and stale copies of repo rules. If graphify belongs in this lane, port only the intentional integration surface after review:

- small tool config or hooks, if still wanted
- concise repo instructions, if still accurate
- no generated cache/output unless explicitly approved as tracked evidence
- no stale edits to `AGENTS.md`, `CLAUDE.md`, or other rule files

## Out Of Scope

- Bolt-v3 feature baseline work: `feat/236-bolt-v3-slice-8-1-baseline` / PR `#248`.
- Chainlink testnet/live delivery: PR `#231`.
- Polymarket capped-market execution behavior: PR `#238`.
- Polymarket depth-vs-fallback platform product decision: issue `#126`.
- Any runtime trading behavior change.

## Branch Discipline

Do not merge old branches wholesale into this lane.

For each older branch or PR, port only the current-main-valid piece, with the original branch/PR named in the commit or PR body. If a piece is obsolete, leave it out and record why.
