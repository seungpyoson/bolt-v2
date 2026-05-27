# External Final Review

T042 is not started. Final exact-head review must wait until T036-T041 are complete: blocker-free final packet, root TOML binding, local verification, pushed exact head, and GitHub CI.

## Pre-T042 Reviewer Disposition

- Kimi: explicitly waived by the operator on 2026-05-25 because Kimi repeatedly fails to produce useful review output. This is a waiver, not an approval.
- Claude, Gemini, DeepSeek, GLM, and Grok: no final exact-head review has been requested yet.

## Pre-T042 Shard Review At `2947546c`

This was not the T042 exact-head final review. It was an early source-shard review before the final implementation batch was committed and pushed.

- Full branch-diff source packets were too large for Claude, Gemini, Grok, DeepSeek, and GLM, so committed diff shards were generated under `/private/tmp/bolt-v2-t042-review-2947546c`.
- Shard 01 approvals with no blocking findings: Claude, Gemini, and Grok 01A.
- DeepSeek shard 01 requested changes:
  - Market-selection source evidence did not prove `price_to_beat_source` matched the TOML financial envelope.
  - Strategy-input evidence did not prove `price_to_beat_source` matched the TOML financial envelope.
  - Chainlink report decoding was bounded to i128 rather than the ABI `int192` protocol width.
- Grok 01B produced a concrete request-changes result even though the wrapper marked the slot failed:
  - Funding-margin proof collection used `rust_decimal::Decimal` parsing/comparison while the source writer used the CLOB V2 decimal-string comparator.
- Claude non-blocking observation accepted as real hardening work:
  - Chainlink report scaling used `Decimal::from_i128_with_scale`, which can panic for report decimal scales above `Decimal::MAX_SCALE`.
- GLM failed with `provider_unavailable: fetch failed`; it is not counted as an approval.

Disposition at current local patch:

- Added fail-closed source checks so market-selection and strategy-input evidence require `price_to_beat_source` to match the TOML financial envelope.
- Funding-margin proof collection now uses the same CLOB V2 decimal-string comparator as the writer.
- Chainlink ReportDataV3 benchmark decoding now validates the ABI `int192` sign extension and scales the full-width two's-complement value without routing through Rust Decimal.
- T042 remains open and must be rerun against the next pushed exact head after local verification and CI.
