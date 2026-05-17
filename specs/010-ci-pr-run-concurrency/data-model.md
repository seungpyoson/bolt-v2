# Data Model: CI PR Run Concurrency

## PrConcurrencyPolicy

Fields:

- `scope`: top-level workflow `concurrency` block in `.github/workflows/ci.yml`.
- `event_branch`: expression branch for `github.event_name == 'pull_request'`.
- `pr_group`: `format('pr-{0}', github.event.number)`.
- `non_pr_group`: `format('{0}-{1}', github.ref_name, github.sha)`.
- `cancel_policy`: `${{ github.event_name == 'pull_request' }}`.

Validation:

- Missing top-level block fails.
- Missing PR event branch fails.
- Missing PR-number grouping fails.
- Missing ref+SHA non-PR grouping fails.
- All-event cancellation fails.

## WorkflowHygieneVerifier

Fields:

- `workflow_text`: CI workflow text.
- `concurrency_block`: top-level `concurrency` block extracted before `jobs`.
- `errors`: actionable verifier errors.

Validation:

- Uses standard-library parsing only.
- Reports exact concurrency invariant failures.
- Preserves existing workflow topology, tool-install, cache, gate, deploy, and nextest checks.

## SupersededRunEvidence

Fields:

- `pr_number`
- `branch`
- `old_run_id`
- `old_head_sha`
- `old_status`
- `old_conclusion`
- `new_run_id`
- `new_head_sha`
- `new_status`
- `new_conclusion`
- `newest_required_checks`

Validation:

- Old run belongs to same PR branch as new run.
- Old run is cancelled after or during supersession, or documented as completed before supersession.
- Newest head runs required gate and is valid merge evidence.
