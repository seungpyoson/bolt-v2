# IV Engine Repository Truth

**Feature**: `specs/026-nt-backed-iv-engine/`
**Captured at**: `2026-06-07T21:43:27Z` (`2026-06-08T06:43:27+0900`)
**Capture basis**: before Phase 1 implementation edits

## Required Setup Evidence

- `git fetch --prune origin`: ran successfully after managed-filesystem escalation was required for `.git/FETCH_HEAD`.
- Local branch: `026-nt-backed-iv-engine`
- `git rev-parse HEAD`: `f994ae15198502aee9227aea5e813d12b8d5bf92`
- `git rev-parse origin/026-nt-backed-iv-engine`: `f994ae15198502aee9227aea5e813d12b8d5bf92`
- `git ls-remote origin refs/heads/026-nt-backed-iv-engine`: `f994ae15198502aee9227aea5e813d12b8d5bf92`
- `git rev-parse main`: `c1b1f7b49414008a11af11da24ebc49762debf54`
- `git rev-parse origin/main`: `c1b1f7b49414008a11af11da24ebc49762debf54`
- Local `main == origin/main`: yes
- Merge base against target base `origin/main`: `c1b1f7b49414008a11af11da24ebc49762debf54`
- Commits ahead of `origin/main`: `6`
- Working tree at capture: clean

## Active PR Evidence

- Open PR for `head:026-nt-backed-iv-engine`: none found.
- Matching prior PR: `#608` (`Add NT-backed IV engine design`) is closed, unmerged, draft, and reference-only.
- `#608` head SHA: `4481016abb19d4c4a24dc318e190957ad41fb30b`
- `#608` base SHA: `c1b1f7b49414008a11af11da24ebc49762debf54`

## Changed Files Against `origin/main`

```text
M	.specify/feature.json
M	AGENTS.md
A	specs/026-nt-backed-iv-engine/checklists/requirements.md
A	specs/026-nt-backed-iv-engine/contracts/iv-engine-api.md
A	specs/026-nt-backed-iv-engine/data-model.md
A	specs/026-nt-backed-iv-engine/plan.md
A	specs/026-nt-backed-iv-engine/quickstart.md
A	specs/026-nt-backed-iv-engine/reference/overlap-ledger.md
A	specs/026-nt-backed-iv-engine/research.md
A	specs/026-nt-backed-iv-engine/spec.md
A	specs/026-nt-backed-iv-engine/tasks.md
```

## Diffstat Against `origin/main`

```text
 .specify/feature.json                              |   2 +-
 AGENTS.md                                          |   2 +-
 .../checklists/requirements.md                     |  66 ++
 .../contracts/iv-engine-api.md                     | 175 +++++
 specs/026-nt-backed-iv-engine/data-model.md        | 760 +++++++++++++++++++++
 specs/026-nt-backed-iv-engine/plan.md              | 227 ++++++
 specs/026-nt-backed-iv-engine/quickstart.md        | 156 +++++
 .../reference/overlap-ledger.md                    |  43 ++
 specs/026-nt-backed-iv-engine/research.md          | 235 +++++++
 specs/026-nt-backed-iv-engine/spec.md              | 261 +++++++
 specs/026-nt-backed-iv-engine/tasks.md             | 339 +++++++++
 11 files changed, 2264 insertions(+), 2 deletions(-)
```

## Local Evidence Commands

- `.specify/scripts/bash/check-prerequisites.sh --json --require-tasks --include-tasks`
- `git fetch --prune origin`
- `git status --short --branch`
- `git rev-parse HEAD`
- `git rev-parse origin/main`
- `git rev-parse main`
- `git merge-base HEAD origin/main`
- `git diff --name-status origin/main...HEAD`
- `git diff --stat origin/main...HEAD`
- GitHub search for open branch PRs and prior matching PRs
