# Production Kill Switch Review Commands

Run these only after explicit approval to send the selected source packet to the named external providers.

Current environment note: these source-bearing commands were attempted after exact user approval, but sandbox review rejected Claude, Gemini, and Kimi launches before source transmission as external disclosure of private workspace data. Do not rerun them through indirect paths or alternate export mechanisms. Run them only if a trusted/allowed source-bearing route becomes available or the gate requirement is changed.

Required approval text:

```text
I approve sending the selected kill-switch review packet and selected repo source files to Relay Claude, Gemini, Kimi, Grok, DeepSeek, and GLM for external review.
```

Scope paths:

```text
goals/production-kill-switch/facts.md,goals/production-kill-switch/research.md,goals/production-kill-switch/design.md,goals/production-kill-switch/plan.md,goals/production-kill-switch/review-packet.md,goals/production-kill-switch/source-excerpts/binary-oracle-edge-taker-submit-path.md,specs/505-nt-loss-governor/spec.md,specs/505-nt-loss-governor/plan.md,src/bolt_v3_submit_admission.rs,src/bolt_v3_live_node.rs,src/bolt_v3_strategy_registration.rs,docs/bolt-v3/2026-04-28-source-grounded-status-map.md,docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml,scripts/verify_bolt_v3_strategy_policy_fence.py,Cargo.toml
```

Prompt:

```text
Review goals/production-kill-switch/review-packet.md and the selected files as a design-only production kill switch approval gate. Return exactly one verdict line: APPROVE or REQUEST_CHANGES. Treat as REQUEST_CHANGES only for blocking flaws that must be fixed before creating the GitHub issue. Re-evaluate from current files only. Findings first, with file:line evidence for blockers.
```

## Relay Claude

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-claude/0.1.0/scripts/claude-companion.mjs run --mode=custom-review --auth-mode subscription --foreground --lifecycle-events markdown --cwd /Users/spson/Projects/Claude/bolt-v2 --scope-paths "$SCOPE_PATHS" -- "$PROMPT"
```

## Relay Gemini

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-gemini/0.1.0/scripts/gemini-companion.mjs run --mode=custom-review --foreground --lifecycle-events markdown --cwd /Users/spson/Projects/Claude/bolt-v2 --scope-paths "$SCOPE_PATHS" -- "$PROMPT"
```

## Relay Kimi

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-kimi/0.1.0/scripts/kimi-companion.mjs run --mode=custom-review --foreground --lifecycle-events markdown --cwd /Users/spson/Projects/Claude/bolt-v2 --scope-paths "$SCOPE_PATHS" -- "$PROMPT"
```

## Relay Grok

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-grok/0.1.0/scripts/grok-companion.mjs run --mode custom-review --scope custom --scope-paths "$SCOPE_PATHS" --foreground --lifecycle-events markdown --prompt "$PROMPT"
```

## Relay DeepSeek

First request an approval token:

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-deepseek/0.1.0/scripts/api-reviewer.mjs approval-request --provider deepseek --mode custom-review --scope custom --scope-paths "$SCOPE_PATHS" --prompt "$PROMPT"
```

Then run with the emitted token:

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-deepseek/0.1.0/scripts/api-reviewer.mjs run --provider deepseek --mode custom-review --scope custom --scope-paths "$SCOPE_PATHS" --approval-token "<approval_token.value>" --lifecycle-events markdown --prompt "$PROMPT"
```

## Relay GLM

First request an approval token:

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-glm/0.1.0/scripts/api-reviewer.mjs approval-request --provider glm --mode custom-review --scope custom --scope-paths "$SCOPE_PATHS" --prompt "$PROMPT"
```

Then run with the emitted token:

```bash
node /Users/spson/.codex/plugins/cache/relay-for-codex/relay-glm/0.1.0/scripts/api-reviewer.mjs run --provider glm --mode custom-review --scope custom --scope-paths "$SCOPE_PATHS" --approval-token "<approval_token.value>" --lifecycle-events markdown --prompt "$PROMPT"
```
