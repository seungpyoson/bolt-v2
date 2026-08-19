# Internal Adversarial Planning Review

**Reviewed scope**: `spec.md`, `plan.md`, `research.md`, `data-model.md`,
`contracts/`, `quickstart.md`, `tasks.md`, the supporting provider-decision
record, and repository governance.

**Review question**: Does the package preserve the approved hard-evidence
research goal, cover all 84 functional requirements and 15 success criteria,
respect Bolt/NT/provider boundaries, and define executable issue-sized work
without claiming that planning is completion?

## Verdict

**PASS for planning publication.** No unresolved critical, high, or medium
planning finding remains. This verdict does not approve implementation, a data
provider, spend, a canonical experiment, deployment, or trading.

## Findings Resolved

1. **Dependency inversion**: detector work preceded its G/D custody prerequisite.
   Tasks now execute Slice 3 before Slice 4.
2. **Premature Stage 2**: the primary report was originally deferred until after
   enrichment. Slice 5 now produces it before Slice 6.
3. **Unscoped published statements**: the primary report lacked atomic claim
   records until Slice 7. Slice 5 now implements base `episode_detected` and
   `not_proven` claims; Slice 7 extends stronger tiers.
4. **Self-asserted role risk**: the authenticated principal had no source. Non-test
   execution now uses AWS STS `GetCallerIdentity`, bound through TOML, and fails
   closed on missing/mismatched identity.
5. **Lifecycle collision**: evidence validity was conflated with Artifact Index
   hot/cold state. The model now keeps the two state machines separate.
6. **Cross-slice placeholder code**: setup pre-registered modules for all seven
   slices. Each module is now registered only in its owning slice.
7. **Unnecessary issue dependency**: the generated feature number collides with
   an unrelated closed PR, and governance does not require seven GitHub issues.
   Each PR instead names one explicit slice of this spec and its residual scope.
8. **Implicit requirements**: prior-artifact disposition, later-access/new-C,
   robustness-primary separation, E/P checkpoint order, conflict-of-interest
   review, and replay-artifact invalidation are now explicit tasks and evidence.
9. **Generated governance drift**: a manual SpecKit block had been appended to
   `AGENTS.md` despite the repository's adapter rule. It and the local feature
   pointer were removed; repository governance remains unchanged.
10. **Fixture authority leakage**: the quickstart attempted non-test registration
    with a fixture authority. Synthetic registration now runs only in the Rust
    test harness; non-test mutation remains fail closed until real user-approved
    STS and timestamp bindings exist.

## Coverage Result

- Functional requirements: 84/84 mapped to implementation phases and evidence.
- Success criteria: 15/15 mapped.
- User stories: 4/4 have an independent verification path.
- Tasks: 80, sequential IDs T001–T080, 16 explicitly parallelizable.
- Unmapped implementation tasks: none.
- Unresolved placeholders or architecture unknowns: none. Concrete experiment
  values and external services are deliberate fail-closed user inputs.

## Static Evidence

- `git diff --check`
- strict task checklist-format and sequential-ID validation
- FR/SC count and range-coverage checks
- placeholder and trailing-whitespace scan
- SpecKit prerequisite check with `tasks.md` present
- credential-material scan over all added planning/support files

## Residual Boundaries

- No market-data, onchain, or timestamp provider is selected.
- No paid or nominally free query is authorized.
- No production timestamp adapter exists yet; canonical commitments remain
  blocked until a later user-approved registered authority is implemented.
- Concrete venues, dates, thresholds, cost caps, roles, and source candidates
  remain typed user-approved experiment values.
- The plan creates no provider adapter, predictive strategy, order path,
  deployment, or live-trading authority.
