# Requirements Checklist: #205 Same-SHA Smoke-Tag Dedup

**Feature**: `specs/008-ci-same-sha-smoke-dedup`  
**Date**: 2026-05-15

- [x] CHK001 Does the spec preserve the full #205 issue intent instead of narrowing to only artifact download? [FR-001..FR-012]
- [x] CHK002 Is exact SHA matching required against a successful `main` push CI run? [FR-001, FR-002]
- [x] CHK003 Are all current deploy-safety lanes represented, including four `test` shards and the source-fence/gate lanes? [FR-003]
- [x] CHK004 Does missing, stale, skipped, cancelled, failed, incomplete, or malformed evidence fail closed? [FR-004]
- [x] CHK005 Is artifact reuse bound to source run ID, artifact ID, branch, and SHA? [FR-005]
- [x] CHK006 Are duplicate heavy lanes expected skipped on tag reuse, with no fallback rebuild? [FR-006, FR-007]
- [x] CHK007 Does deploy log source run ID, check suite ID, artifact ID, and source SHA? [FR-009]
- [x] CHK008 Is PR CI explicitly preserved and separated from post-merge tag topology? [FR-010]
- [x] CHK009 Do verifier/self-tests cover both positive and negative resolver behavior? [FR-011]
- [x] CHK010 Is real after-evidence required before issue closure? [FR-012]
- [x] CHK011 Are #195, #332, #344, and #340 boundaries kept out of this implementation except where current topology must be validated? [Scope]
- [x] CHK012 Are external API/action assumptions linked to primary sources? [Research]
