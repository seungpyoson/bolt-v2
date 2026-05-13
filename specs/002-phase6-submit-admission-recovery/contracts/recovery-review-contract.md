# Contract: Recovery Review Memo And Prompt

## Recovery Memo Contract

Path:
- `specs/002-phase6-submit-admission-recovery/recovery-review.md`

Required sections:
1. Current baseline
   - current `main` SHA
   - local checkout state while memo was drafted
   - merged Phase 3-5 source
   - current architecture facts that Phase 6 must preserve
2. Stale PR context
   - PR number
   - stale head SHA
   - stale base branch/SHA
   - merge-base with current `main`
   - stale-vs-current drift summary
3. Keep/rewrite/reject map
   - each Phase 6-relevant stale concept
   - classification
   - reason
   - current-main constraint
4. Allowed future touch surface
   - exact files allowed for fresh Phase 6
   - exact files disallowed unless separately justified
5. Stop conditions
   - any condition that halts implementation
   - any condition that halts recovery-strategy review
6. Review questions
   - architecture questions
   - scope questions
   - anti-slop questions

## Recovery-Strategy Review Prompt Contract

Prompt must ask reviewers to answer:
1. Is it correct to treat #317 as reference-only?
2. Does the keep/rewrite/reject map preserve current `main` architecture?
3. Are any valid Phase 6 concepts missing from the salvage map?
4. Are any stale concepts incorrectly kept?
5. Is the future Phase 6 touch surface too broad or too narrow?
6. Does the plan preserve evidence -> admission -> submit ordering?
7. Does the plan preserve gate -> arm admission -> runtime capture -> NT run ordering?
8. Is fail-closed canary budget consumption before NT submit acceptable?
9. Is relying on live-canary gate validation for root risk cap acceptable, or must submit admission re-check it?
10. Are there any hidden dual paths, hardcodes, or concrete-provider leaks?
11. What must be fixed before implementation starts?

## Code Review Contract

Code review must not be requested until:
- fresh Phase 6 branch exists from current `main`
- branch has no unrelated local changes
- targeted local verification has run
- exact-head CI is green
- recovery-strategy findings are resolved or explicitly deferred

Recovery-strategy review must not be requested until:
- planning artifacts are committed or otherwise captured as an exact immutable snapshot
- local findings from internal review are resolved
- prompt includes exact current/stale SHAs and the complete recovery memo
