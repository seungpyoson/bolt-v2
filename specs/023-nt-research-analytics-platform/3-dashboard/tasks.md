# Tasks: Dashboard

- [ ] DASH-001 Define dashboard field-source matrix, including source proof id, run purpose, proof pin reason code/detail when present, fidelity class, claim limits, warning fields, partial/unavailable gap labels, and RA-owned strategy-review/promotion status source where applicable.
- [ ] DASH-002 Resolve #409 `PortfolioSnapshot` dependency for PnL completeness.
- [ ] DASH-003 Resolve #77 durable trade-history/PnL dependency.
- [ ] DASH-004 Decide #36 redemption-realized-PnL inclusion or exclusion.
- [ ] DASH-005 Record #369 as non-closure context for dashboard readiness.
- [ ] DASH-006 Define freshness rules plus stale, partial, unavailable, excluded, and exploratory/non-trading-truth display behavior.
- [ ] DASH-007 Run Grafana/Metabase/Superset/Preset/Retool/Plotly/custom product gate and refresh/reference `plan.md` Product Cost Baselines.
- [ ] DASH-008 Define selected product/UI no-mutation controls.
- [ ] DASH-009 Implement read-only source contract validation, including rejection of proof-strength reclassification, upstream proof acceptance, accepted proof mutation, forbidden-claim weakening, historical-result relabeling after proof supersession, promotion-status inference from BTE metrics, and promotion-state mutation.
- [ ] DASH-010 Add tests that dashboard field-source resolution does not branch on hardcoded venue/provider names.
- [ ] DASH-011 Add tests that displayed artifact links stay under the shared S3 `artifact_root`, allow explicit artifact-local handles only for direct upstream handoffs, use committed Artifact Index snapshots for cross-run/bulk lists, reject publish/repair/mutation of Artifact Index records, and do not create a second canonical root.
- [ ] DASH-012 Add tests that dashboard cannot delete, expire, or mutate canonical artifacts.
- [ ] DASH-013 Add tests for unmapped fields, stale rendering, missing PnL/exposure source gap labels, source-proof/claim-limit display without upgrade, strategy outlook displayed only with accepted source or exploratory label and never calculated as trading truth by dashboard, and mutation absence.
- [ ] DASH-014 Link/update issue dependencies named in `spec.md` before implementation review.
- [ ] DASH-015 Run future branch verification checks.
