# Tasks: Dashboard

- [x] DASH-001 Define dashboard customer jobs and capability classes before product selection: trade monitor, trade investigation, optional annotation/review notes, and controlled action workflow; keep trading/runtime/credential/fund/order mutation outside this package unless separately approved.
- [x] DASH-002 Define dashboard field-source matrix, including trade explanation fields, source proof id, run purpose, proof pin reason code/detail when present, fidelity class, claim limits, warning fields, source role, data status/gap reason, and RA-owned strategy-review/promotion status source where applicable.
- [x] DASH-003 Resolve #409 `PortfolioSnapshot` dependency for PnL completeness.
- [x] DASH-004 Resolve #77 durable trade-history/PnL dependency.
- [x] DASH-005 Decide #36 redemption-realized-PnL inclusion or exclusion.
- [x] DASH-006 Record #369 as non-closure context for dashboard readiness.
- [x] DASH-007 Define freshness rules plus source-role and data-status semantics; defer final user-facing label names and legend text to the cross-project terminology/legend registry.
- [x] DASH-008 Run Grafana/Metabase/Superset/Preset/Retool/Plotly/custom product gate against specified customer jobs and read-model shape, then refresh/reference `plan.md` Product Cost Baselines.
- [x] DASH-009 Define selected product/UI no-mutation controls and any future non-trading annotation/review write controls only after explicit owner/schema/audit rules exist.
- [x] DASH-010 Implement read-only source contract validation, including rejection of proof-strength reclassification, upstream proof acceptance, accepted proof mutation, forbidden-claim weakening, historical-result relabeling after proof supersession, promotion-status inference from BTE metrics, and promotion-state mutation.
- [x] DASH-011 Add tests that dashboard field-source resolution does not branch on hardcoded venue/provider names.
- [x] DASH-012 Add tests that displayed artifact links stay under the configured S3 `artifact_root`, allow explicit artifact-local handles only for direct upstream handoffs, use committed Artifact Index snapshots for cross-run/bulk lists, reject cross-kind joins from independent latest pointers instead of manifest lineage ids, reject publish/repair/mutation of Artifact Index records, and do not create a second canonical root.
- [x] DASH-013 Add tests that dashboard cannot delete, expire, or mutate canonical artifacts.
- [x] DASH-014 Add tests for unmapped fields, stale rendering, missing PnL/exposure source gap labels, source-proof/claim-limit display without upgrade, strategy outlook displayed only with accepted source or exploratory label and never calculated as trading truth by dashboard, and mutation absence.
- [x] DASH-015 Link/update issue dependencies named in `spec.md` before implementation review. GitHub issue #733 records the dashboard source-contract scope and links #36, #77, #88, #148, #236, #369, and #409.
- [x] DASH-016 Run future branch verification checks. Evidence: PR #690 exact head `9495e29d5c85544b02abd8088a850b40975b2d10` completed CI workflow run 27614839439 with conclusion `success`.
