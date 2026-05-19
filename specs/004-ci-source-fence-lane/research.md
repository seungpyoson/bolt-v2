# Research: CI Source-Fence Lane

> **Superseded in part by #400** (PR #401): the "`test` waits for `source-fence`" decision below is no longer the active topology. After #332 sharded `test` into `test-archive` -> `test-shards` -> aggregate `test`, the carry-forward dep moved to `test-archive needs: [detector, source-fence]`. #400 removed that dep so the two lanes run in parallel. Merge enforcement now lives only in `gate.needs` (which still requires `source-fence` and aggregate `test` to succeed). The historical decision text is preserved below for forensics; the live invariant is the verifier rule `test-archive must not need source-fence` and the gate result check on `needs.source-fence.result == "success"`.

## Decision: `test` waits for `source-fence`

**Rationale (historical, superseded by #400)**: GitHub Actions does not automatically cancel independent jobs when one job fails. If `test` starts in parallel with `source-fence`, the stale-assertion case from run `25859831755` can still pay full test setup cost. `test needs: [detector, source-fence]` makes source-fence drift fail before full nextest setup.

**Alternatives considered**: Let both jobs run after `detector`. Rejected at the time because it did not satisfy the early-failure intent when source-fence drift is deterministic. **Adopted by #400** once the cost of a partial-restore rebuild (smoke evidence: ~30s clippy / ~1m28s source-fence / ~46s test-archive on warm cache) made the fail-fast saving negligible against the parallel-lane wall-clock gain.

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

## Decision: Pin PyYAML for the naming verifier

**Rationale**: #342 requires deterministic verifiers, and the naming audit is YAML. A hashed source-fence requirement keeps CI dependency resolution deterministic while preserving PyYAML's complete parser behavior for future audit-file edits.

**Alternatives considered**: Keep a repo-local YAML subset parser. Rejected because it creates parser maintenance risk and can fail future valid YAML edits for parse-shape reasons unrelated to the audit data.
