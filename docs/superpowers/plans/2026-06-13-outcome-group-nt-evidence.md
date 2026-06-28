# Outcome Group NT Evidence

Task 0 for the shared outcome-group substrate uses the machine-readable ledger
at `docs/bolt-v3/research/outcome-groups/nt-capability-ledger.toml`.

That TOML file is the source for first-slice NT reuse decisions before any
outcome-group implementation may add local execution, book, cache, or provider
mechanics. `scripts/verify_outcome_group_nt_reuse.py` validates the ledger and
the current source-code guardrails.

Live Polymarket basket mutation remains disabled for this slice. At the pinned
NT revision, the reachable Polymarket settlement/status stream depends on the
new-markets channel, while Bolt rejects `subscribe_new_markets = true` for the
controlled-connect runtime.
