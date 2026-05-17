# PR Run Concurrency Evidence - 2026-05-17

Issue: #355

## Static Policy

Current `.github/workflows/ci.yml` defines top-level workflow concurrency:

- PR group: `format('pr-{0}', github.event.number)`
- Non-PR group: `format('{0}-{1}', github.ref_name, github.sha)`
- Cancel policy: `${{ github.event_name == 'pull_request' }}`

Interpretation:

- Pull-request runs for the same PR cancel older in-progress heads.
- Main, tag, deploy, scheduled, and manual flows are not cancelled by this policy.
- Non-PR groups include SHA, so separate commits do not collide.

## Real PR Evidence

Source command:

```bash
gh run list --workflow CI --event pull_request --limit 80 \
  --json databaseId,headBranch,headSha,status,conclusion,createdAt,updatedAt,displayTitle,url
gh run view 25988183744 --json databaseId,headBranch,headSha,status,conclusion,createdAt,updatedAt,event,displayTitle,url,jobs
gh run view 25988258315 --json databaseId,headBranch,headSha,status,conclusion,createdAt,updatedAt,event,displayTitle,url,jobs
gh pr list --state all --head codex/ci-344-residual-minute-work \
  --json number,title,state,headRefName,headRefOid,url
```

PR:

- PR: #351
- Title: `ci: add docs pass stub for path filters`
- Branch: `codex/ci-344-residual-minute-work`
- Final PR head after merge evidence: `4e87fc01d194e5395a366c048d917081b1668cc5`

Superseded run:

- Run: `25988183744`
- URL: `https://github.com/seungpyoson/bolt-v2/actions/runs/25988183744`
- Event: `pull_request`
- Branch: `codex/ci-344-residual-minute-work`
- Head SHA: `7156317456f3d473186405e9984c5718536e7d96`
- Created: `2026-05-17T10:21:22Z`
- Updated: `2026-05-17T10:25:24Z`
- Conclusion: `cancelled`
- Observed cancellation points:
  - `build` job cancelled at `2026-05-17T10:25:14Z`.
  - `nextest archive` job cancelled at `2026-05-17T10:25:13Z`.
  - one queued `nextest shard ${{ matrix.shard }} of 4` matrix job cancelled before steps.
  - `deploy` cancelled before steps.

Newer run for same branch:

- Run: `25988258315`
- URL: `https://github.com/seungpyoson/bolt-v2/actions/runs/25988258315`
- Event: `pull_request`
- Branch: `codex/ci-344-residual-minute-work`
- Head SHA: `2d578ddc0ce47ad346121d292f9bcf7031714d1a`
- Created: `2026-05-17T10:24:58Z`
- Updated: `2026-05-17T10:35:36Z`
- Conclusion: `success`
- Required gate evidence:
  - `detector`: success
  - `fmt-check`: success
  - `deny`: success
  - `clippy`: success
  - `check-aarch64`: success
  - `source-fence`: success
  - `nextest archive`: success
  - `nextest shard 1 of 4`: success
  - `nextest shard 2 of 4`: success
  - `nextest shard 3 of 4`: success
  - `nextest shard 4 of 4`: success
  - aggregate `test`: success
  - `build`: success
  - aggregate `gate`: success
  - `deploy`: skipped on pull request

## Evidence Summary

- Same PR branch had an older run cancelled after a newer head run started.
- The newer head still ran the full required PR gate and passed.
- The stale run did not continue through full `build`, `nextest archive`, shard, or deploy work.
- This proves the current PR-only concurrency policy avoids at least one obsolete PR-head run from continuing to full CI completion.

## Remaining Proof For This PR

This document records existing real-world cancellation evidence. The #355 implementation PR still needs exact-head CI on its own head before merge.
