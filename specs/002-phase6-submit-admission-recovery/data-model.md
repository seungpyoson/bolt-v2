# Data Model: Phase 6 Submit Admission Recovery

## CurrentMainBaseline

Represents the authoritative repo state for all recovery decisions.

Fields:
- `main_sha`: full current `main` SHA.
- `verification_date`: date of inspection.
- `phase_3_5_source`: merged PR or commit that established current registration/evidence architecture.

Validation rules:
- Must be refreshed before implementation starts.
- Must not be replaced by stale branch state.

## StalePrReference

Represents old PR material that may contain useful ideas but is not mergeable.

Fields:
- `pr_number`: stale PR number.
- `head_sha`: full stale PR head SHA.
- `base_ref`: stale PR base branch or SHA.
- `merge_base_with_main`: merge-base against current `main`.
- `drift_summary`: file/diff summary showing stale overlap.

Validation rules:
- Must be marked reference-only.
- Must not be used as implementation base.

## SalvageItem

Represents one concept, file, or hunk from stale PR material.

Fields:
- `name`: short label.
- `source`: stale PR file or concept.
- `classification`: `keep`, `rewrite`, or `reject`.
- `reason`: evidence-backed explanation.
- `current_main_constraint`: current-main behavior that must be preserved.

Validation rules:
- Every Phase 6-relevant stale item must have one classification.
- `keep` means keep concept, not necessarily code.
- `rewrite` means adapt against current `main`.
- `reject` means do not port.

## RecoveryMemo

Represents the durable review artifact at `specs/002-phase6-submit-admission-recovery/recovery-review.md`.

Fields:
- `current_main_baseline`
- `stale_pr_reference`
- `salvage_items`
- `allowed_future_touch_surface`
- `review_questions`
- `implementation_stop_conditions`

Validation rules:
- Must exist before recovery-strategy review.
- Must contain exact SHAs.
- Must include keep/rewrite/reject map.
- Must define what future Phase 6 may touch.

## RecoveryStrategyReview

Represents independent pre-implementation review of recovery direction.

Fields:
- `reviewer`
- `reviewed_memo_version`
- `findings`
- `required_fixes`
- `approval_or_blocker_status`

Validation rules:
- At least two independent responses required before implementation.
- Every substantive finding must be resolved, disproved with evidence, or explicitly deferred.

## FreshPhase6Pr

Represents future implementation branch and PR.

Fields:
- `base_sha`: current `main` SHA at branch creation.
- `head_sha`: exact pushed PR head.
- `scope`: Phase 6 submit admission only.
- `verification`: targeted tests and CI.

Validation rules:
- Must be created from current `main`.
- Must not include stale Phase 3-5 churn.
- Must demonstrate evidence -> admission -> submit ordering.
