# Issue-208 Branch Map v1

## Purpose

This file explains the difference among the saved `issue-208` branches and tags.

Use it when you need to know:

- which branch is the main archive
- which branch is only a baseline reference
- which branch is the post-protocol subject
- which refs are branches versus tags

## Canonical Branches

### 1. `issue-208-validation-protocol`

Role:

- the main archive branch for the `#208` work

What it contains:

- the protocol artifacts
- the benchmark artifacts
- the adjudication artifacts
- the handoff prompts
- the archive inventory

Use this branch if you want:

- the full saved documentation set
- the protocol/control-plane history
- the cleanest archive entry point

### 2. `issue-208-process-validator`

Role:

- the prototype baseline

What it contains:

- the original validator/prototype state that later protocol work was judged against

Use this branch if you want:

- the baseline snapshot before the protocol/archive split
- the exact prototype reference point

### 3. `issue-208-scientific-validation-post-protocol`

Role:

- the post-protocol subject branch

What it contains:

- the replayed subject-under-test state
- the post-protocol subject registration

Use this branch if you want:

- the subject that was used for the later local scientific-validation attempt
- the exact subject-side state, separate from the protocol branch

## Archived Tags

These tags preserve important exact states even if branch structure changes later.

### Protocol archive tag

- `issue-208-validation-protocol-b6f24d1`

Meaning:

- pins the protocol branch at the earlier archival point `b6f24d1`

### Post-protocol subject archive tag

- `issue-208-scientific-validation-post-protocol-6b36d16`

Meaning:

- pins the post-protocol subject branch at `6b36d16`

### Prototype baseline archive tag

- `issue-208-process-validator-c4d182d`

Meaning:

- pins the prototype baseline at `c4d182d`

### Invalid first-subject archive tag

- `issue-208-scientific-validation-subject-50c0fca`

Meaning:

- preserves the first invalid scientific-validation subject attempt
- this is tag-only archival state, not an active branch

## Practical Meaning

If you only want one branch to read:

- use `issue-208-validation-protocol`

If you want to compare protocol vs. baseline:

- compare `issue-208-validation-protocol` against `issue-208-process-validator`

If you want to inspect the subject that was actually exercised later:

- inspect `issue-208-scientific-validation-post-protocol`

If you want the invalid first attempt only for history:

- use the tag `issue-208-scientific-validation-subject-50c0fca`

## Recommended Mental Model

Think of the saved refs like this:

- `issue-208-process-validator` = baseline
- `issue-208-validation-protocol` = protocol archive
- `issue-208-scientific-validation-post-protocol` = subject under test
- tags = frozen historical checkpoints

## Ref Rule

For long-lived docs:

- use branch names for moving archive entry points
- use tags for immutable checkpoints
- avoid embedding mutable branch head SHAs in explanatory docs
