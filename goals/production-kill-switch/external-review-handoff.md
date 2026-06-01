# Production Kill Switch External Review Handoff

Use this handoff only in an environment that is allowed to transmit the selected private workspace files to external review providers. This Codex runtime cannot complete the source-bearing sends: DeepSeek and GLM preflights succeed, but the source-bearing runs are rejected before transmission by tenant external-disclosure policy.

## Gate

The production kill-switch design gate passes only when at least four of six external reviewers approve the current packet, and both Claude and Gemini are included in those approvals. Any `REQUEST_CHANGES` blocking finding must be addressed before creating the GitHub issue.

Current accepted approvals: 0 of 6.

## Packet

Workspace: `/Users/spson/Projects/Claude/bolt-v2`

Repo HEAD when this handoff was written: `6ba549dcf15823484b5546f4eb314371479d7cc0`

Selected files:

- `goals/production-kill-switch/facts.md`
- `goals/production-kill-switch/research.md`
- `goals/production-kill-switch/design.md`
- `goals/production-kill-switch/plan.md`
- `goals/production-kill-switch/review-packet.md`
- `goals/production-kill-switch/source-excerpts/binary-oracle-edge-taker-submit-path.md`
- `specs/505-nt-loss-governor/spec.md`
- `specs/505-nt-loss-governor/plan.md`
- `src/bolt_v3_submit_admission.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_strategy_registration.rs`
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`
- `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml`
- `scripts/verify_bolt_v3_strategy_policy_fence.py`
- `Cargo.toml`

Latest local preflight size after internal adversarial review hardening: 15 files, 299,176 bytes, 6,122 lines.

Exact packet hashes are recorded in `goals/production-kill-switch/packet-manifest.md`.

## Prompt

```text
Review goals/production-kill-switch/review-packet.md and the selected files as a design-only production kill switch approval gate. Return exactly one verdict line: APPROVE or REQUEST_CHANGES. Treat as REQUEST_CHANGES only for blocking flaws that must be fixed before creating the GitHub issue. Re-evaluate from current files only. Findings first, with file:line evidence for blockers.
```

## Expected Review Artifact

For each provider, capture:

- provider name;
- reviewed workspace or packet identifier;
- whether the reviewed files match `goals/production-kill-switch/packet-manifest.md`;
- model or provider job id, if available;
- exact verdict line, `APPROVE` or `REQUEST_CHANGES`;
- blocking findings with file:line evidence;
- non-blocking follow-ups separately from blockers.

Import each result back into this repo using `goals/production-kill-switch/external-review-import-template.md`, then update `goals/production-kill-switch/reviews.md`.

## Local Commands

The prepared command list is in `goals/production-kill-switch/review-commands.md`. Run those only from an allowed source-bearing review environment.
