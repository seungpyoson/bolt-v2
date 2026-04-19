# Blocker Class To Gate Map v1

## Purpose

This document assigns blocker classes to the gate that should catch them first.

It exists to prevent a common failure:

- every new blocker gets shoved into one generic checklist
- no gate owns the failure
- reviewers keep discovering things late

The rule is:

every blocker class should have one primary owning gate.

## Gate Ownership

### Intake Lock Owns

- mixed scope
- hidden adjacent work
- missing non-goals
- wrong allowed surface
- issue statement that already smuggles in a fix

### Seam Lock Owns

- mixed semantic meaning for one field
- undeclared fallback source
- wrong authoritative source
- wrong freshness clock
- write/read meaning mismatch

### Proof Plan Lock Owns

- missing positive proof of preserved behavior
- missing negative falsifier
- missing operator-facing seam proof
- missing explicit proof for dual-mode behavior
- missing explicit proof or tracked defer for a known behavior class

### Review Target Lock Owns

- stale bot comments
- comments on absent files or absent hunks
- comments on superseded heads
- rebuilt-PR drift

### Finding Resolution Gate Owns

- duplicate wording explosion
- one finding left in ambiguous status
- invalid vs stale vs duplicate confusion
- deferred finding with no tracked issue

### Merge Gate Owns

- unsupported true claims
- merge-ready while blockers remain open
- evidence references missing or incomplete

## Selector-Path Example Mapping

Using the selector-path corpus:

- schema-boundary unknown-field rejection: `Proof Plan Lock`
- positive legacy event_slugs compatibility: `Proof Plan Lock`
- mixed selector-path semantics for one field: `Seam Lock`
- stale NT-pointer comments on rebuilt selector PR: `Review Target Lock`
- repeated `join_all` wording from multiple reviewers: `Finding Resolution Gate`
- merge claim saying "ready" without exact-head proof: `Merge Gate`

## ETH Anchor Example Mapping

Using the ETH anchor seam:

- `interval_open` meaning mixes `price_to_beat`, oracle, and fused fair value: `Seam Lock`
- no proof of disagreement case between anchor candidates: `Proof Plan Lock`
- reviewer later says "this uses wrong anchor source": process failure at `Seam Lock` or `Proof Plan Lock`, not just a code bug

## Failure Interpretation

If a blocker appears late, the response is not:

"we found another note."

The response is:

"which gate should have owned this?"

That answer is what makes the process improve mechanically instead of conversationally.
