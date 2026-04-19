# Finding Canonicalization Experiment Result

## Hypothesis

The process should collapse same-problem/different-wording review comments into one canonical finding, and it should mechanically mark wrong-target review comments as stale instead of leaving them as open correctness findings.

## Result

The corpus supports that hypothesis.

What the process was able to do:

1. Collapse the repeated `join_all` / unbounded slug fan-out comments from PR #183 and PR #192 into one canonical finding.
2. Reclassify NT-pointer comments on PR #192 as `review_target_mismatch` rather than active selector-path findings.
3. Keep the legacy `event_slugs` schema-boundary problem separate because it had a different subject, predicate, and locus.

## Process Improvement Proved By This Experiment

The process needs an explicit `review_target.toml`.

Without a frozen review target, stale-diff comments are indistinguishable from real blockers and the finding ledger cannot stay MECE.
