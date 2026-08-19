# Quickstart: Synthetic Zero-Spend Acceptance Path

This is the intended acceptance flow after the planned implementation exists.
It uses repository fixtures only. It makes no provider call, spends no credit,
and grants no live-trading authority.

## 1. Prepare the Fixture Definition

Create a strict fixture TOML from the contract in
`contracts/experiment-definition.md`. Use synthetic venue/instrument identities,
synthetic admitted observations, separated fixture roles, a fixture timestamp
verifier, and the test artifact root. Do not place credentials in the file.

## 2. Validate and Register

```bash
cargo run --manifest-path crates/backtesting-vertical-slice/Cargo.toml \
  --bin pump_research -- validate \
  --experiment config/research/pump-research-synthetic.toml

cargo run --manifest-path crates/backtesting-vertical-slice/Cargo.toml \
  --bin pump_research -- register-version \
  --experiment config/research/pump-research-synthetic.toml
```

Expected evidence: strict validation succeeds, one canonical semantic hash is
reported, and the registered artifact has the expected parent and Artifact Index
lineage. No provider adapter or network acquisition runs.

## 3. Exercise G and D

Register only the definition and non-E0 source-policy evidence needed to form G.
Verify the fixture independent timestamp receipt and commit G before exposing any
fixture E0 byte or metadata. Then let the committed custodian operation ingest
and admit the retained synthetic inputs, append every access/disclosure event,
close the required checkpoint, and commit D.

Expected evidence:

- every roster unit has exactly one declared status;
- invalid rights, missing retained bytes, unknown coverage, pending timestamps,
  shared roles, stale heads, and disclosure-budget violations fail closed; and
- no E0-derived fields beyond the G-frozen disclosure program are emitted.

## 4. Run Discovery Twice

Execute `discover` in two fresh temporary environments against the same admitted
fixture inputs.

Expected evidence: roster, episode, control, censoring, attrition, and semantic
manifest hashes match exactly. Boundary candidates remain censored, unmatched
episodes remain visible, and the fixture's null case publishes without changing
thresholds.

## 5. Exercise C and Sealed Confirmation

Commit C against a closed custody head. Run the canonical and verification roles
separately, compare normalized semantic outputs, and close the release checkpoint.

Expected evidence: equal fixture outputs release once. Mismatch, partial output,
human exposure, invalid timestamp, stale head, or exceeded retry count quarantines
the attempt and prevents release.

## 6. Prove Stage-2 Remains Gated

Attempt E, source selection, quote/query/purchase, and mechanism publication
without a distinct user-authorization receipt.

Expected evidence: every operation fails before provider access. After registering
a fixture authorization and content-neutral candidate packets, selection follows
the E-frozen ranking mechanically and still performs no provider call.

## 7. Exercise Claim and Lifecycle Rules

Publish fixture `episode_detected` and `not_proven` claims. Attempt manipulation,
causal, queue-position, and L3 claims without their required evidence, then
quarantine one source.

Expected evidence: unsupported claims fail; dependent active results and claims
become invalidated; all prior artifacts remain in the audit lineage.

Publish the fixture research report and verify that removing uncertainty,
attrition, a null result, a prior overlapping attempt, positive-unlabeled policy,
or any generalization boundary causes fail-closed validation.

## 8. Verification Evidence

Run cheap local formatting/schema checks and the targeted behavior tests for the
implemented slice. Push the exact head and use advisory remote CI for compile-
heavy Rust evidence. The review request must name the slice, exact head, tests,
remaining accepted scope, and all known residual risks.
