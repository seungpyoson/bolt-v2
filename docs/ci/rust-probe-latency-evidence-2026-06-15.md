# Rust Probe Latency Evidence - 2026-06-15

Scope: first live validation of the managed Rust Probe debug lane for issue #741.

- Command surface: `just rust-probe check-lib`
- Workflow: `Rust Probe`
- Mode: `check-lib`
- Runner tier: `heavy`
- Commit: `7775226f61b1a4664f458f9fe8746623f50e396b`
- Workflow run: `https://github.com/seungpyoson/bolt-v2/actions/runs/27555379512`
- Job: `https://github.com/seungpyoson/bolt-v2/actions/runs/27555379512/job/81452840219`
- Result: success
- Workflow wall time: `2026-06-15T14:59:56Z` to `2026-06-15T15:02:46Z` = 170 seconds
- Active job time: `2026-06-15T15:00:10Z` to `2026-06-15T15:02:45Z` = 155 seconds
- Run Rust Probe step time: `2026-06-15T15:00:24Z` to `2026-06-15T15:02:39Z` = 135 seconds

Evidence source: `gh run view 27555379512 --json createdAt,startedAt,updatedAt,status,conclusion,headSha,url,displayTitle,workflowName,event,databaseId` and `gh run view 27555379512 --json jobs`.

This is latency evidence for the debug lane only. Rust Probe remains debugging feedback and is not merge proof.
