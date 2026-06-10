# Quickstart: Non-Live Validation

This package is a specification and gate plan. It does not authorize capital.

## Inspect the package

```bash
sed -n '1,220p' specs/032-world-cup-production-profit-gate/spec.md
sed -n '1,220p' specs/032-world-cup-production-profit-gate/plan.md
sed -n '1,220p' specs/032-world-cup-production-profit-gate/research.md
```

## Confirm guarded Spec Kit pointers remain unchanged

```bash
rg -n "specs/023-nt-order-intent-layer/plan.md" AGENTS.md
jq -r .feature_directory .specify/feature.json
```

Expected:

- `AGENTS.md` still references `specs/023-nt-order-intent-layer/plan.md`.
- `.specify/feature.json` still returns `specs/023-nt-order-intent-layer`.

## Validate static shape

```bash
rg --files specs/032-world-cup-production-profit-gate | sort
rg -n "NEEDS[ ]CLARIFICATION|T[O]DO|fix[ -]later" specs/032-world-cup-production-profit-gate
rg -n "capital authorized by this spec|skip controlled-connect allowed|skip capital-probe allowed|direct venue submit path allowed|process env secret source allowed" specs/032-world-cup-production-profit-gate --glob '!quickstart.md'
git diff --check
cargo fmt --check
just source-fence
```

Expected:

- File list contains spec, plan, research, data-model, contracts, tasks, checklist, and internal review.
- The second and third `rg` commands return no matches.
- Formatting/source-fence checks pass.

## Baseline tests

```bash
cargo test --locked --lib
```

Expected: all library tests pass. This is supporting evidence only; it does not replace real source proof, controlled-connect, or capital-probe evidence.

## Future implementation checks

After code is added, each slice must add failing-then-passing tests for:

- missing official event proof
- conflicting venue resolution rule
- stale provider capability proof
- direct source claim without direct access proof
- aggregator-sourced bookmaker odds mislabeled as direct
- lost reference quorum
- positive edge without fill/markout/settlement evidence
- disabled promotion package generation
- stale controlled-connect report
- capital-probe eligibility scoped to exact venue/account/product/config hash
