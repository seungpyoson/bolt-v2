# CI Storage Tripwire Governance

The authoritative governance record is the single repository TOML document that declares `[storage_tripwire]`.

That TOML owns the operator storage cap, cap source, fixed thresholds, owner, escalation text, issue labels, marker format, issue-matching limits, workflow contract, and update cadence for the scheduled/manual storage tripwire. Do not duplicate those values in this document.

The tripwire consumes the stable `ci_storage_audit` JSON contract and only opens or updates GitHub issues matched by the configured marker. It must not delete caches or artifacts, publish commit statuses, publish check-runs, or emit the richer forecast/trend/drift/coverage report owned by #936.
