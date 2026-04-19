# Finding Canonicalization Experiment

## Objective

Test whether the process can mechanically collapse real review wording into canonical findings and mechanically reject stale-diff review artifacts.

## Corpus

- PR #183 selector-path review comments
- PR #192 selector-path review comments

## Hypothesis

If the process works, repeated wording about unbounded `join_all` fan-out should collapse into one canonical finding, and stale NT-pointer comments on PR #192 should classify as stale review rather than remaining open correctness findings.
