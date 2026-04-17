# External Adversarial Review Prompts

These are user-gated and intentionally not executed in this session.

## Prompt 1: Claude Adversarial Review

```text
Review seungpyoson/bolt-v2 PR #191 as an adversary, not as a collaborator.

Scope:
- Treat this as an isolated pre-#109 experiment only.
- Do not review current mainline work outside this PR scope.
- Read PROCESS.md and planning/109/MECHANISM-MAP.md first.

Files to inspect:
- src/platform/resolution_basis.rs
- src/platform/polymarket_catalog.rs
- src/platform/ruleset.rs
- src/validate.rs
- src/validate/tests.rs
- tests/polymarket_catalog.rs
- tests/ruleset_selector.rs
- tests/platform_runtime.rs

Your job:
- Try to prove a reachable wrong action in production.
- Focus on malformed config bypass, ambiguous metadata coercion, ETH/BTC family generalization, weakening of reference-venue validation, and selector/runtime regressions.
- For each claimed finding, name the exact file, line, condition, and runtime timing.
- If you cannot prove the bug, mark it DISPROVEN and explain why.
- Do not give optional cleanup, style nits, or broad redesign advice.

Verification commands:
- cargo test --test polymarket_catalog
- cargo test --test ruleset_selector
- cargo test phase1_runtime_resolution_basis_requires_matching_reference_venue_family
- cargo test phase1_runtime_rejects_invalid_resolution_basis_format
- cargo test phase1_runtime_eth_chainlink_basis_requires_matching_reference_venue_family
- cargo test --test platform_runtime
```

## Prompt 2: Gemini Adversarial Review

```text
Perform an adversarial code review of seungpyoson/bolt-v2 PR #191.

Goal:
- Break the claim that the resolution-basis selector path is now asset-generic and fail-closed.

Required context:
- PROCESS.md
- planning/109/MECHANISM-MAP.md
- planning/109/REDTEAM.md

Focus areas:
- parser false positives from prose or URLs
- parser false negatives that would silently block intended ETH support
- mismatch between runtime validation and selector parsing rules
- cases where malformed `resolution_basis` could still reach live selection
- cases where recognized family validation semantics changed unintentionally
- runtime effects in platform_runtime tests

Output format:
- Findings only.
- Each finding must include severity, exact file:line, exploit path, and whether it proves a wrong action or only a safe halt.
- If you think a suspected issue is not real, mark it DISPROVEN with evidence.
- Ignore style feedback and optional refactors.

Verification commands:
- cargo test --test polymarket_catalog
- cargo test --test ruleset_selector
- cargo test phase1_runtime_resolution_basis_requires_matching_reference_venue_family
- cargo test phase1_runtime_rejects_invalid_resolution_basis_format
- cargo test phase1_runtime_eth_chainlink_basis_requires_matching_reference_venue_family
- cargo test --test platform_runtime
```

