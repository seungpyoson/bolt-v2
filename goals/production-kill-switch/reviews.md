# Production Kill Switch External Review Gate

Status: not approved.

The design approval gate is the external model quorum, not the Plannotator workflow checkpoint. The gate passes only when at least four of six external reviewers approve the current packet, and both Claude and Gemini are included in those approvals. Any blocking finding requires revising the packet and rerunning affected reviews before issue creation.

## Current Packet

Review packet:

- `goals/production-kill-switch/facts.md`
- `goals/production-kill-switch/research.md`
- `goals/production-kill-switch/design.md`
- `goals/production-kill-switch/plan.md`
- `goals/production-kill-switch/review-packet.md`
- `specs/505-nt-loss-governor/spec.md`
- `specs/505-nt-loss-governor/plan.md`
- `src/bolt_v3_submit_admission.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_strategy_registration.rs`
- `goals/production-kill-switch/source-excerpts/binary-oracle-edge-taker-submit-path.md`
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`
- `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml`
- `scripts/verify_bolt_v3_strategy_policy_fence.py`
- `Cargo.toml`

Verification metadata:

- `goals/production-kill-switch/packet-manifest.md`

Review prompt:

```text
Review goals/production-kill-switch/review-packet.md and the selected files as a design-only production kill switch approval gate. Return exactly one verdict line: APPROVE or REQUEST_CHANGES. Treat as REQUEST_CHANGES only for blocking flaws that must be fixed before creating the GitHub issue. Re-evaluate from current files only. Findings first, with file:line evidence for blockers.
```

## Provider Status

| Provider | Required For Quorum | Current Status | Counts Toward Gate |
| --- | --- | --- | --- |
| Claude | Yes | No valid current verdict. After exact user approval, the launch attempt was rejected before source transmission by sandbox review as external disclosure of private workspace data. | No |
| Gemini | Yes | No valid current verdict on the revised packet. A prior review before the latest revisions requested changes and is not approval. After exact user approval, the current launch attempt was rejected before source transmission by sandbox review as external disclosure of private workspace data. | No |
| Kimi | No | No valid current verdict. After exact user approval, the launch attempt was rejected before source transmission by sandbox review as external disclosure of private workspace data. | No |
| Grok | No | No valid current verdict. The latest launch group was interrupted before a captured verdict; no production-kill-switch Grok reviewer process is active. | No |
| DeepSeek | No | Approval-request preflight succeeded after replacing the oversized strategy file with a bounded excerpt and again after the internal adversarial-review hardening pass. Latest preflight: 15 files, 299,176 bytes, 6,122 lines; source was not sent. On 2026-06-01, after current-turn user approval to use DeepSeek, the source-bearing run with approval token was rejected before source transmission by sandbox review as external disclosure of private workspace data. | No |
| GLM | No | Approval-request preflight succeeded after replacing the oversized strategy file with a bounded excerpt and again after the internal adversarial-review hardening pass. Latest preflight: 15 files, 299,176 bytes, 6,122 lines; source was not sent. On 2026-06-01, after current-turn user approval to use GLM, the source-bearing run with approval token was rejected before source transmission by sandbox review as external disclosure of private workspace data. | No |

Current accepted approvals: 0 of 6.

Required accepted approvals before issue creation: at least 4 of 6, including Claude and Gemini.

## Blocker

Exact user approval was provided, but the sandbox reviewer still rejected source-bearing launches for Claude, Gemini, Kimi, DeepSeek, and GLM because they would disclose private workspace data to external providers. Do not route around this with indirect execution or alternate export paths.

The remaining allowed paths are:

- use a materially safer non-source-bearing review prompt, if the user accepts that it cannot satisfy the current source-based external approval gate;
- run reviews only through a trusted/allowed connector if one becomes available;
- change the goal requirement to waive or replace the external-model approval gate;
- import reviews obtained in an allowed environment using `goals/production-kill-switch/external-review-import-template.md`; or
- use `goals/production-kill-switch/external-review-handoff.md` to run the same packet from an allowed source-bearing review environment.

DeepSeek and GLM preflight is mechanically ready for source-bearing direct API review, but the actual approved source-bearing run is sandbox-policy blocked. Those approvals also could not satisfy the current quorum alone because Claude and Gemini are mandatory and remain sandbox-policy blocked.

If a trusted/allowed source-bearing review route becomes available, run the six reviews against the current packet, record provider job ids/verdicts here, revise the design for any blocking findings, and only create the GitHub issue after the quorum passes.
