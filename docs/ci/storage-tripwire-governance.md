# CI Storage Tripwire Governance

The authoritative governance record is the single repository TOML document that declares `[storage_tripwire]`.

That TOML owns the operator storage cap, cap source, fixed thresholds, owner, escalation text, issue labels, marker format, issue-matching limits, issue listing page size, workflow contract, and update cadence for the scheduled/manual storage tripwire. Do not duplicate those values in this document.

The tripwire consumes the stable `ci_storage_audit` JSON contract and only opens or updates GitHub issues matched by the configured marker. It must not delete caches or artifacts, publish commit statuses, publish check-runs, or emit the richer forecast/trend/drift/coverage report owned by #936.

Manual dispatch is default-branch only. The workflow job must skip any selected non-default ref before checkout so branch-local workflow code cannot mutate repository issues.

Breached `apply` and `apply-live` runs alert by issue and step summary, then exit successfully after issue processing. The read-only `evaluate` command returns non-zero on breach for callers that need a failing probe. Open tripwire issues are closed manually after operator review; while a breach remains active, the existing issue is refreshed with the latest snapshot instead of creating duplicates.

The workflow verifier enforces the storage tripwire workflow as a closed contract: the configured trigger set, single schedule entry, top-level permissions, concurrency, default-branch job guard, runner, single configured job, and exact checkout/run step shape must all match TOML-owned policy.
