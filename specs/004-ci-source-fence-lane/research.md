# Research: CI Source-Fence Lane

## Decision: `test` waits for `source-fence`

**Rationale**: GitHub Actions does not automatically cancel independent jobs when one job fails. If `test` starts in parallel with `source-fence`, the stale-assertion case from run `25859831755` can still pay full test setup cost. `test needs: [detector, source-fence]` makes source-fence drift fail before full nextest setup.

**Alternatives considered**: Let both jobs run after `detector`. Rejected because it does not satisfy the early-failure intent when source-fence drift is deterministic.

## Decision: One `just source-fence` recipe

**Rationale**: Existing CI avoids raw cargo workflow commands and routes Rust checks through managed recipes. A recipe keeps local and CI execution identical and gives the workflow linter one lane command to detect.

**Alternatives considered**: Inline commands in YAML. Rejected because raw cargo workflow commands violate the existing managed build contract and duplicate command ownership.

## Decision: Add the two missing verifier scripts

**Rationale**: #342 names `verify_bolt_v3_status_map_current.py` and `verify_bolt_v3_pure_rust_runtime.py`, but the #343 baseline does not contain them. Dropping them would narrow #342. Adding narrow scripts satisfies the exact script list without turning the branch into broad architecture work.

**Alternatives considered**: Document that the scripts are absent and skip them. Rejected because the user explicitly asked not to cut requirements without a strong reason, and the scripts have clear evidence contracts in the status map.

## Decision: Document temporary duplicate source-fence test execution

**Rationale**: Until #332 shards or filters full `nextest`, `just test` will still run the source-fence tests. #342 owns the canonical early lane now; #332 owns later exclusion or explicit duplicate ownership. A workflow comment and spec note prevent silent duplicate ownership.

**Alternatives considered**: Exclude tests from current full `just test`. Rejected as #332 scope because it changes the full test lane selector before sharding work.

## Decision: Keep source-fence cache ownership separate from full nextest

**Rationale**: The new lane uses `key: source-fence-v1` so its warm runtime is measurable independently from the broader `nextest-v2` lane. Sharing target cache keys now would mix #342 lane proof with #195 cache-retention ownership and #332 sharding ownership.

**Alternatives considered**: Share restore keys between `source-fence` and `test`. Rejected for this slice because it changes cache strategy outside the source-fence contract and makes the lane's own warm-cache evidence less direct.

## Decision: Avoid new Python package installation

**Rationale**: #342 requires deterministic verifiers. Depending on ambient runner image packages or unpinned `pip install` behavior is not deterministic and violates the repo's dependency discipline.

**Alternatives considered**: Install PyYAML in CI. Rejected because it adds an unpinned Python dependency for a source-scan lane when the existing audit YAML is simple enough for a repo-local parser.
