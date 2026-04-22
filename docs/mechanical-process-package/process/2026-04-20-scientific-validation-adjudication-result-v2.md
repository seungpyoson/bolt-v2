# Scientific Validation Adjudication Result v2

OVERALL
- verdict: NOT_ESTABLISHED
- subject_eligible_under_protocol: no
- benchmark_corpus_frozen_enough: no
- benchmark_execution_adequate: no
- evidence_overstated: yes
- self_graded_residue: yes

FINDINGS
- finding 1
  - kind: Subject Ineligibility / Circular Logic Replay
  - locus: `SV-ENTRY-1` in `docs/mechanical-process-package/process/2026-04-20-future-scientific-validation-entry-gates-v1.toml` vs `[subject]` in `docs/mechanical-process-package/validation/subjects/2026-04-20-post-protocol-subject-registration-v1.toml`
  - why: The entry gate `SV-ENTRY-1` requires the subject logic to postdate the protocol (`logic_postdates_protocol`). The subject registration admits `harness_logic_delta = "replayed_validator_logic_and_fixtures_after_protocol_freeze"`. Replaying pre-existing baseline logic onto a new branch to obtain a new `first_logic_commit_at` timestamp is purely a metadata trick. The logic itself still predates the protocol, making the preregistration entirely circular and violating the predicate.
  - proof: The registration file states `harness_logic_delta = "replayed_validator_logic_and_fixtures_after_protocol_freeze"`, openly proving the logic is not newly written against the protocol.
  - severity: critical

- finding 2
  - kind: Benchmark Independence Violation / Self-Graded Corpus
  - locus: `SV-ENTRY-3` in `2026-04-20-future-scientific-validation-entry-gates-v1.toml` vs mutation seeds in `B4` through `B7` descriptors
  - why: `SV-ENTRY-3` enforces `benchmark_independence`. While the benchmark fixtures and runners were mechanically moved to "protocol_owned" artifacts, the actual mutation seeds (e.g., `promotion_gate.toml#gates[0].comparator_kind = "join_exists"`) exactly mirror the specific builder-authored unit tests already present in the replayed subject logic (`tests/delivery_validator_cli.rs`). The subject logic was explicitly shaped to pass these exact "held-out" seeds before the protocol was frozen, so the corpus is not independent.
  - proof: `tests/delivery_validator_cli.rs` in the exact subject head `867d824ffdbe3063fb2bf2eb9993e6077427269d` still contains explicit builder-authored tests for `"join_exists"` and `"graph_walk"`, exactly matching the mutation targets in `B4-unsupported-comparator-kinds.toml`.
  - severity: critical

- finding 3
  - kind: Protocol Artifact Violation / Mixed Results
  - locus: `SV-ENTRY-4` in `2026-04-20-future-scientific-validation-entry-gates-v1.toml` vs `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v2.toml`
  - why: The protocol `SV-ENTRY-4` pass rule mandates: "final scientific verdict must be recorded separately after external adjudication", and the gate explicitly fails closed on "single artifact mixes local completion with final establishment". The `results-v2.toml` artifact violates this by combining `[local_execution]` results with a `[scientific_validation]` block asserting `subject_eligible_under_protocol = true` and `benchmark_corpus_frozen_enough = true` within the exact same file.
  - proof: The file `2026-04-20-scientific-validation-results-v2.toml` contains both `[local_execution]` with `status = "completed"` and `[scientific_validation]` asserting establishment booleans like `benchmark_execution_adequate = true`, directly violating the required structural separation.
  - severity: critical

Recommendation: scientific validation not established.