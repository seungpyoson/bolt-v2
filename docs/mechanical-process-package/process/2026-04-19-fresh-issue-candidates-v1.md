# Fresh Issue Candidates v1

## Purpose

This document ranks current `bolt-v2` issues as candidates for the first full fresh-issue process trial.

The ranking is by process fitness, not urgency alone.

## Best Candidate

### Issue #205

Title:

- `CI/deploy: avoid duplicating full test+build work when smoke-tagging merged main`

Why it fits:

- one clear seam: same-SHA proof reuse vs redundant tag pipeline
- strong existing evidence in the issue body
- no need to reopen trading-strategy semantics immediately
- clean acceptance can be stated in exact workflow/job terms
- likely lower ambiguity than live-runtime or deploy-drift incidents

Why it is good for the process:

- tests `issue_contract`
- tests `proof_plan`
- tests exact CI artifact evidence
- avoids dragging in live operator hotfix chaos

## Second Candidate

### Issue #182

Title:

- `fix: authenticate NT trust-root validator fetch from private claude-config`

Why it fits:

- one clear seam: authenticated private fetch for one workflow
- strong exact failing target already named
- exact acceptance can be written mechanically

Why it is weaker than #205:

- crosses repo boundary into `claude-config`
- introduces auth/secret handling complexity
- review may be noisier because workflow security changes tend to attract adjacent concerns

## Bad Candidates For First Full Trial

### Issue #207

Why not first:

- code-red process breach
- live-state ambiguity
- process and incident facts are entangled
- too easy for the fresh trial to become another large forensics effort

### Issue #206

Why not first:

- live hotfix record, not a clean implementation seam
- source-of-truth/process questions dominate over a bounded code seam

### Issue #180

Why not first:

- too broad
- explicitly a follow-up bucket
- already carries many residuals and migrated notes

### Issue #171

Why not first:

- very broad GitHub process enforcement scope
- would create a lot of machinery at once
- bad first test for a process that is not yet proven

## Recommendation

If the next step is one real end-to-end fresh issue run, pick:

1. `#205` first
2. `#182` second

That ordering gives the process one relatively clean CI/evidence-heavy trial before it attempts a repo-crossing auth/security workflow issue.
