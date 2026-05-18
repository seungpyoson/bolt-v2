# Data Model: Disk Pressure Governance

## DiskPressureIssueTrack

- `issue`: GitHub issue number.
- `title`: Current issue title.
- `category`: `epic`, `investigation`, `implementation`, `closed-investigation`, or `out-of-repo-anchor`.
- `owned_surfaces`: Disk surfaces this track owns.
- `status`: Open, closed, or blocked by research gate.
- `forward_owner`: Implementation issue or external repo issue that owns fixes.
- `pr_mapping`: One planned PR per implementation issue.
- `residual_scope`: Work accepted as remaining.

## DiskSurface

- `path_family`: Exact path or glob family.
- `source_actor`: Cargo, rust-verification, Claude/Codex, developer tool, bolt-v3 runtime, CI/test, cargo registry, or unknown.
- `growth_shape`: Single rolling file, task output, target tree, registry tree, session transcript set, or unknown tree.
- `current_evidence`: Measured size, date, and source issue/comment.
- `owner_issue`: One owning issue.
- `policy`: RetentionPolicy reference.

## RetentionPolicy

- `scope`: Path family and owner.
- `report_command`: Dry-run/status command.
- `protected_items`: Active processes, pinned toolchains, current sessions, hot cache classes, or retrieval artifacts.
- `limit_source`: Config or operator policy source.
- `apply_mode`: Explicit apply command or external approval requirement.
- `failure_behavior`: Refuse, warn with override, or route to out-of-scope owner.

## VerificationLane

- `lane`: Targeted local, full local, CI shard/archive, artifact/evidence, no-mistakes, or external review.
- `entrypoint`: `just`, rust-verification wrapper, GitHub Actions, no-mistakes, or plugin command.
- `disk_risk`: Low, medium, high.
- `allowed_when`: Preconditions such as disk preflight pass and routing proof.
- `not_allowed_when`: Known unsafe conditions.
- `ci_relationship`: Whether the lane is authoritative broad proof, narrow local debug/TDD, or duplicate local work to avoid.

## ImplementationSlice

- `issue`: Owning issue.
- `branch`: Planned branch.
- `red_test`: Failing test/verifier required before code.
- `green_check`: Minimal command proving the slice.
- `review_gates`: no-mistakes, Claude, Gemini, DeepSeek, GLM, Kimi, CI.
- `pr_scope`: Exact accepted scope and residuals.

## CargoRoutingEvidence

- `entrypoint`: `just`, rust-verification wrapper, no-mistakes, shell, CI, or agent.
- `observed_command`: Command text or logged command family.
- `observed_target_dir`: Managed target path or unmanaged target path.
- `evidence_source`: Config file, command output, log path, or issue comment.
- `status`: Managed, unmanaged, unknown, or excluded.
