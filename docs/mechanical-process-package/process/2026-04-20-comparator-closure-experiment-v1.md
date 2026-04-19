# Comparator Closure Experiment v1

## Hypothesis H3

A small closed comparator set can absorb stage-admission logic without adding new bespoke validator branches per stage.

This experiment does not claim full harness closure.
It tests whether the gate language can carry more of the stage logic mechanically.

## Validation Date

- 2026-04-20

## Treatment

1. Keep `stage_promotion.toml` as transition metadata only.
2. Keep `promotion_gate.toml` as the sole admission exam.
3. Extend gate comparison from string-only to scalar equality:
   - `string_eq`
   - `scalar_eq`
4. Migrate one real stage-admission rule out of bespoke code:
   - `merge_candidate` no longer has a dedicated `merge_ready = true` branch
   - the gate now compares `merge_claims.toml#merge_ready` to literal `true`

## Inventory Of Remaining Bespoke Rules

| Stage | Current rule | Status |
|---|---|---|
| `review`, `merge_candidate` | `execution_target.toml` completeness and exact-head binding | partly representable |
| `review`, `merge_candidate` | `ci_surface.toml` completeness and stage-job declaration | partly representable |
| `review`, `merge_candidate` | `claim_enforcement.toml` must cover every true merge claim | not yet representable |
| CI-driven stages | `orchestration_reachability.toml` matrix coverage and set membership | not yet representable |
| `review`, `merge_candidate` | `review_rounds/` exact-round ingestion checks | partly representable |
| `merge_candidate` | `merge_ready = true` admission requirement | migrated into gate |

## Comparator Algebra v1

Currently implemented:

- `string_eq`
- `scalar_eq`

Likely next if H3 continues:

- `artifact_exists`
- `count_eq`
- `set_contains`
- `all_required_fields_nonempty`

Stop signal:

- if a new stage rule needs a one-off comparator used by only one stage, the set is no longer closing cleanly

## Evidence Set

Synthetic evidence:

- 5 stage pass cases:
  - `intake`
  - `seam_locked`
  - `proof_locked`
  - `review`
  - `merge_candidate`
- 20 shared stage fail cases:
  - missing gate artifact across 5 stages
  - multiple gates across 5 stages
  - wrong stage binding across 5 stages
  - non-pass verdict across 5 stages
- 1 migrated-stage specific fail case:
  - `merge_candidate` with `merge_ready = false` under `scalar_eq`

Regression evidence:

- `candidate-205-smoke-tag-ci` review package still passes
- `exp-finding-canonicalization` still passes
- `exp-proof-plan-selector-path` still passes with warning
- `exp-eth-anchor-semantics` still blocks

## "Statistically Significant" Metaphor

The evidence is meaningful because:

1. all 5 declared stages were exercised under the same gate model
2. the same 4 falsifier classes were replayed across all 5 stages
3. one real stage-admission rule (`merge_candidate`) moved from bespoke code to comparator logic
4. prior regression fixtures stayed stable under the stricter gate model

This is still not proof of full closure.
It is evidence that the one-gate model survives a wider stage surface without immediately fragmenting.
