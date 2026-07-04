# Predeploy exit evidence fixtures

These JSONL envelopes cover the exit evidence shape emitted before the exit
observed-input extension. They were generated from the PR merge-base serializers
and committed so compatibility tests read historical bytes verbatim.

Generation trail:

```bash
git fetch origin main
base="$(git merge-base HEAD origin/main)"
git worktree add /private/tmp/bolt-v2-pr1207-predeploy-fixtures "$base"
cd /private/tmp/bolt-v2-pr1207-predeploy-fixtures
cargo test --test bolt_v3_decision_evidence \
  exit_decision_evidence_writes_one_durable_line_and_readers_skip_it \
  exit_evaluation_evidence_round_trips_populated_and_sparse_records
```

The committed lines are the single `exit_decision` and `exit_evaluation`
envelopes produced by that checkout's serializers, preserving the absence of the
observed-input fields added by PR #1207.
