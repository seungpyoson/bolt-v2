# Automated Review Resolution Log

## Greptile

- Status: DISPROVEN
- Evidence:
  - PR `#191` created at `2026-04-17T09:40:10Z`
  - `fetch_pr_comments`, `list_pull_request_reviews`, and `list_pull_request_review_threads` returned no Greptile activity immediately after creation
  - the same three fetches returned no Greptile activity again at `2026-04-17T09:41:14Z`
  - after the final head update, the same three fetches still returned no Greptile activity at `2026-04-17T09:42:39Z`
- Resolution: no Greptile automated comments existed in the session window, so there was nothing to address

## Gemini Code Assist

### G1: `LINK` / `CHAIN` stopword collision

- Status: FIXED
- Review source: comment `3099274926` on `src/platform/resolution_basis.rs`
- Finding: treating `link` and `chain` as stopwords blocks legitimate assets such as `LINK/USD`
- Resolution:
  - removed `chain` and `link` from `SYMBOL_STOPWORDS` in `src/platform/resolution_basis.rs:16-42`
  - added regression coverage in `tests/polymarket_catalog.rs:197-205`

### G2: multiple distinct symbol pairs should halt

- Status: FIXED
- Review source: comment `3099274947` on `src/platform/resolution_basis.rs`
- Finding: returning the last matching pair violates the fail-closed ambiguity rule when metadata contains multiple distinct pairs
- Resolution:
  - changed `extract_symbol_pair` to collect distinct candidates and return `None` unless exactly one pair exists in `src/platform/resolution_basis.rs:139-170`
  - added regression coverage in `tests/polymarket_catalog.rs:278-287`
