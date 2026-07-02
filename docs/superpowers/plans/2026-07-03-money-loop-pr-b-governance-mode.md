# Money Loop PR-B Explicit Governance Mode Implementation Plan

Part of #1179.

## Constraints

- Do not add limits, caps, thresholds, counts, windows, or strategy-local gates.
- Keep the fix at live-node boot/config admission; do not change strategy submit mechanics.
- Preserve per-submit admission behavior for existing test and shadow callers.
- Production config must declare supervised deposit-capped mode explicitly when all exposure controls are absent.

## Tasks

- [x] Add failing live-node tests for ungoverned submit-capable boot rejection and explicit supervised deposit-capped declaration acceptance.
- [x] Add typed `risk.live_submit_governance.mode` config with the single accepted declaration.
- [x] Add live-node boot validation before submit admission construction.
- [x] Wire the tracked production overlay/config composition so the active supervised pilot declares the mode.
- [x] Run focused Rust tests and allowed static checks before PR.
