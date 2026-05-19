# Phase 9 Current-main Audit Report

**Date**: 2026-05-14
**Branch**: `022-bolt-v3-phase9-current-main-audit`
**Worktree**: `.worktrees/022-bolt-v3-phase9-current-main-audit`
**Audit source anchor**: `23acab30b73990302765ea441550fabcbf03f570`
**Refreshed base**: `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`
**Decision**: **Blocked for tiny live order approval**. Current main is usable for audit and no-submit readiness work only. This audit did not approve live capital.

## Source Proof

- `git rev-parse HEAD main origin/main` returned `23acab30b73990302765ea441550fabcbf03f570` for all refs.
- `git log -1 --oneline --decorate` showed `23acab3 ... Merge pull request #328 from seungpyoson/020-bolt-v3-phase8-implementation`.
- `git branch --show-current` returned `022-bolt-v3-phase9-current-main-audit`.
- `src/main.rs:56-57` builds with `build_bolt_v3_live_node(&loaded)?` and runs through `run_bolt_v3_live_node(&mut node, &loaded).await?`.
- Final refresh merged current `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`; `git diff --name-status 23acab30b73990302765ea441550fabcbf03f570..origin/main` showed only `.github/workflows/stale.yml` and `.github/workflows/summary.yml`.

## PR And Review-bundle Provenance

The audit source anchor is `23acab30b73990302765ea441550fabcbf03f570`, which was `HEAD == main == origin/main` when the audit ran. Draft PR #331 then added the Phase 9 audit artifacts as commit `1cf7baae739fc8f288511cc9055d4b76adc82537`.

At initial PR head `1cf7baa`, `git diff --stat origin/main...HEAD` showed only the five `specs/003-phase9-current-main-audit/**` files. No runtime source, test, verifier, or prior roadmap doc changed between audit source anchor `23acab30` and PR/review-bundle head `1cf7baa`. Source-line findings remained anchored to current main; review-bundle findings against `1cf7baa` used the same source content plus the new audit docs.

After accepted remediation and review-comment fixes, the branch was refreshed onto `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273`. That base refresh only imported workflow-maintenance changes and did not change the Phase 9 source coverage surface.

## Coverage Matrix

| Surface | Evidence | Status |
| --- | --- | --- |
| `src/bolt_v3_*.rs` | Included in literal scan and targeted source inspection. | Covered |
| `src/bolt_v3_*/**/*.rs` | Included in literal scan by `src/bolt_v3_*`; targeted inspection covered providers, market families, validators, live node, secrets. | Covered |
| `src/strategies/binary_oracle_edge_taker.rs` | Literal scan plus policy scan; targeted lines show runtime strategy parameters are config fields at `59-75` and used at `1267`, `1304`, `1695-1722`, `1774-1813`, `1901-1908`. | Covered |
| Retired legacy runtime paths | `src/clients/**`, `src/platform/**`, `src/live_config.rs`, `src/config.rs`, `src/live_node_setup.rs`, `src/raw_capture_transport.rs`, `src/startup_validation.rs`, `src/validate.rs`, `src/bin/raw_capture.rs`, and `src/bin/render_live_config.rs` are absent from the current source tree and fenced by `tests/bolt_v3_production_entrypoint.rs::codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules`. | Retired |
| Shared runtime support | Literal scan includes `src/bin/stream_to_lake.rs`, `src/bounded_config_read.rs`, `src/execution_state.rs`, `src/lake_batch.rs`, `src/log_sweep.rs`, `src/nt_runtime_capture.rs`, `src/raw_types.rs`, `src/secrets.rs`, and `src/venue_contract.rs`. | Covered |
| Bolt-v3 tests and fixtures | Policy scan over `tests`; file inventory includes `tests/bolt_v3_*.rs` and `tests/fixtures/bolt_v3/**`. | Covered |
| Verifier scripts | Inspected and run: runtime literals, provider leaks, status-map drift, pure Rust runtime, legacy default fence, strategy policy fence, core boundary, naming. | Covered, all passed |
| Roadmap docs/specs | Policy scan over `docs` and `specs`; targeted status/spec inspection. | Covered, stale docs found |

## Proof Commands And Results

| Command | Result |
| --- | --- |
| Current-head literal coverage `rg -n '"[^"]+"|[0-9]+' ...` | Wrote `/private/tmp/bolt-v3-phase9-current-literal-coverage.txt`, 6,866 lines at head `311b9e7`. |
| Current-head policy coverage `rg -n "polymarket|chainlink|venue|strategy|provider|market_family|admission|risk|default|fallback|bypass|hardcoded|TODO|FIXME|fix later|As an AI|language model|I'm sorry|apologize|unfortunate" ...` | Wrote `/private/tmp/bolt-v3-phase9-current-policy-coverage.txt`, 5,898 lines at head `311b9e7`. |
| `python3 scripts/verify_bolt_v3_runtime_literals.py` | Passed: `OK: Bolt-v3 runtime literal audit passed.` |
| `python3 scripts/verify_bolt_v3_provider_leaks.py` | Passed: `OK: Bolt-v3 provider-leak verifier passed.` |
| `python3 scripts/verify_bolt_v3_status_map_current.py` | Passed: `OK: Bolt-v3 status map matches current entrypoint and verifier evidence.` |
| `python3 scripts/verify_bolt_v3_pure_rust_runtime.py` | Passed: `OK: Bolt-v3 pure-Rust runtime verifier passed.` |
| `python3 scripts/verify_bolt_v3_legacy_default_fence.py` | Passed: `OK: Bolt-v3 legacy default fence passed.` |
| `python3 scripts/verify_bolt_v3_strategy_policy_fence.py` | Passed: `OK: Bolt-v3 strategy policy fence passed.` |
| `python3 scripts/verify_bolt_v3_core_boundary.py` | Passed: `OK: Bolt-v3 core boundary audit passed.` |
| `python3 scripts/verify_bolt_v3_naming.py` | Passed: `OK: Bolt-v3 canonical naming audit passed.` |
| Current-head debt marker scan `rg -n "TODO|FIXME|fix later|As an AI|language model|I'm sorry|apologize|unfortunate" src tests docs specs` | Wrote `/private/tmp/bolt-v3-phase9-current-debt-marker-coverage.txt`, 5 lines. No active source/test TODO, FIXME, or AI-slop markers; remaining hits are recorded command lines plus one historical doc statement. |

## External Review Status

Phase 9 remediation follow-up opened draft PR #331 and confirmed exact-head CI green before the initial external review wave:

- Initial audit-docs PR head: `1cf7baae739fc8f288511cc9055d4b76adc82537`.
- CI run `25855655415`: detector, fmt-check, deny, test, clippy, and gate passed; build and deploy skipped.
- Claude branch-diff review `2dd43871-cf27-47d7-a945-5db14fe5926d`: completed, but selected only the five changed Phase 9 docs. Use as docs-artifact review only, not full source coverage.
- Gemini branch-diff review `0f5e53c6-ff99-4a5e-ac01-9670709b7c34`: completed, but selected only the five changed Phase 9 docs. Use as docs-artifact review only, not full source coverage.
- DeepSeek and GLM direct-API custom reviews: full required source/docs/test/verifier scope completed through `/private/tmp/bolt-v2-phase9-review-bundle` split into five shards because `src/strategies/binary_oracle_edge_taker.rs` exceeds the direct-API per-file cap. Each shard used approval-request output with `source_content_transmission: not_sent` before the approved run.
- Gemini full-bundle custom review `dd09379f-f45c-476d-ae49-5375ea99dae8`: source was sent, but the provider returned `usage_limited` before producing a review result. Treat this slot as failed, not as approval.
- Claude full-bundle custom review `1024dbe3-c076-419c-8811-11719bfb959e`: source was sent, but OAuth non-interactive inference failed with `401 Invalid authentication credentials`. Treat this slot as failed, not as approval.
- Gemini Code Assist review `4289298096`: four PR threads were reviewed. Accepted and fixed: AI-slop scan evidence, bounded config reads, and expired fair-probability fail-closed behavior.
- Greptile review `4289608086` / comment `3241081732`: accepted P2 diagnostic wording finding for oversized config reads; fixed by changing the error text from an exact-size claim to "read at least" wording.
- `no-mistakes` run `01KRKBKGXDA741HMQRRKGB9R4R` on head `29407b38`: accepted three warning findings. BV3-P9-001 caught the active schema doc/examples missing required `auto_load_debounce_milliseconds`; BV3-P9-002 caught generated-output repair failing on oversized drifted `live.toml`; BV3-P9-003 caught pure-Rust verifier scope missing runtime-capture and strategy modules.
- `no-mistakes` run `01KRKCP0BMCGWSE1503SW0K6XM` on head `97482c87`: accepted BV3-P9-VERIFY-001 and BV3-P9-CONFIG-001. The pure-Rust verifier now strips only `#[cfg(test)]` items instead of truncating production code after the first test-only item, and the 1 MiB pre-parse config-size guard is documented as a resource-exhaustion guard rather than trading policy.

| Reviewer | Coverage | Result | Disposition |
| --- | --- | --- | --- |
| Claude branch diff | Five Phase 9 docs only | Approve | Accepted only as docs-artifact review. Not source coverage. |
| Gemini branch diff | Five Phase 9 docs only | Request changes | Accepted for docs gaps around live-canary/admission/evidence coverage. Not source coverage. |
| DeepSeek shard 1 | Audit docs plus `binary_oracle_edge_taker.rs` chunks | Approve | Confirms strategy parameters are config-owned and no hidden runtime values were found in the strategy chunks. |
| DeepSeek shard 2 | Shared Polymarket client, live node, legacy validate, runtime-literal audit | Approve | Accepts F2/F4/F5/F6; confirms F4 in `src/clients/polymarket.rs` and runtime-capture test gap. |
| GLM shard 1 | Audit docs plus `binary_oracle_edge_taker.rs` chunks | Request changes | Accept SHA-provenance gap; resolved by `PR And Review-bundle Provenance`. Reject source invalidation because PR diff is audit docs only. Accept request to classify strategy order-shape policy. |
| GLM shard 2 | Shared Polymarket client, live node, legacy validate, runtime-literal audit | Approve | Accept missed verifier-scope dimension for legacy/shared defaults; track under F9. |
| GLM shard 3 | Live config, schema docs, Chainlink client, instrument-filter tests | Approve with concerns | Accept missed Chainlink reconnect-interval classification; track under F10. |
| GLM shard 4 | Provider/archetype/readiness/tiny-canary sources | Approve with concerns | Accept F2 dormant-impact refinement and F3/F5 confirmation; the archetype-to-provider fee-provider coupling is closed by routing fee-provider construction through the provider binding surface. |
| DeepSeek shard 4 | Live config, Chainlink client, adapters, schema docs | Approve | Confirms F4 for legacy/default surfaces; no new high-severity findings. |
| DeepSeek shard 5 | Config loader, registries, prior specs, fixtures, verifiers | Approve | Confirms F1/F4 and no new hardcode gaps in supplied files. |
| GLM shard 5 | Config loader, registries, prior specs, fixtures, verifiers | Approve | Accepts F1-F6, runtime-literal verifier scope gap, and remediation-order adjustment to prioritize F5 earlier. |
| Gemini Code Assist | PR inline review | Commented | Accepted and fixed substantive code findings; accepted and fixed AI-slop scan evidence wording. |
| Greptile | PR inline review | Commented | Accepted and fixed P2 diagnostic wording finding on oversized config reads. |
| no-mistakes | Local gate on Phase 9 head | Warning findings | Accepted and fixed schema-doc drift, oversized generated-output repair, pure-Rust verifier runtime/strategy-scope gap, verifier test-only stripping, and config-size guard documentation. |

Current disposition: DeepSeek and GLM direct-API bundle shards are complete and dispositioned. Claude and Gemini full-bundle slots initially failed for provider/runtime limits, so they are recorded as failed review slots rather than approval. Follow-up S1-S5 reviews at exact head `f2de98c` completed across Claude, Gemini, DeepSeek, and GLM; S5 closed the omitted Bolt-v3/runtime-surface coverage gap for `src/bolt_v3_adapters.rs`, `src/bolt_v3_client_registration.rs`, `src/bolt_v3_decision_evidence.rs`, `src/bolt_v3_live_canary_gate.rs`, `src/bolt_v3_instrument_filters.rs`, `src/bolt_v3_no_submit_readiness.rs`, `src/bolt_v3_no_submit_readiness_schema.rs`, `src/bolt_v3_readiness.rs`, `src/bolt_v3_secrets.rs`, `src/bolt_v3_strategy_registration.rs`, `src/bolt_v3_submit_admission.rs`, `src/validate.rs`, `src/nt_runtime_capture.rs`, `src/bounded_config_read.rs`, and `src/secrets.rs`. Gemini Code Assist and Greptile actionable comments are fixed and dispositioned. Follow-up T033-T040 remediation also used task-specific DeepSeek and GLM pre/post review slots; every post-remediation slot approved. Follow-up T034/T039/T040/T060/T061/T062/T063 custom review at exact head `b897dd6` completed across Gemini `46e1d661-5001-4f76-9f5b-367df876626d`, Claude `d1746da7-27d0-408b-8446-cce186e895df`, DeepSeek `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`, and GLM `job_7f550722-951f-4d8a-a0a6-85a011f7855f` with no blocking findings. Follow-up T064-T066 custom review at exact head `bf2ad6f` completed across Gemini, Claude, DeepSeek, and GLM with no blocking findings; Claude/GLM noted the T065 venue-error test gap, which is addressed with explicit `empty_venue` and `whitespace_only_venue` tests. The resulting follow-up patch at `535f973` passed exact-head CI run `25875820284`, and narrow DeepSeek `job_b555bb7b-cb56-48a8-967f-e5c1f22a0c97` plus GLM `job_1978ca21-40aa-439e-acc5-5d1ac881594d` review approved with no blockers.

| Task | Scope | DeepSeek reviews | GLM reviews | Disposition |
| --- | --- | --- | --- | --- |
| T033 | Polymarket debounce TOML ownership | Pre `job_f43e2757-6b32-43b5-a140-d54d2131b78a`: Approve. Post `job_abb90795-287c-4ab7-8765-e3960143c577`: Approve. | Pre `job_74dd6ae1-ca2d-42d4-b141-7c2b4d554c25`: Approve. Post `job_e116ff68-5a3a-41cb-9449-ca6d344f189d`: Approve. | Moved to TOML; provider-binding and config validation tests cover the contract. |
| T034 | One-venue-per-provider-kind architecture cap | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | Removed the global provider-kind count gate; Gemini and Claude also approved the `b897dd6` bundle with no blockers. |
| T035 | Legacy/shared default path disposition | Pre `job_8ce8eb29-eab2-4eac-b28a-23ce21663019`: Approve. Post `job_b580415e-e88f-4629-96d2-22a44037c0dd`: Approve. | Pre `job_bc06d740-de87-4002-8b8d-e8a85f100611`: Approve. Post `job_b6673cfe-d794-44c3-8ea4-c73a53863e60`: Approve. | Fenced as non-bolt-v3 production path with source-scan regression. |
| T036 | Runtime-capture notification while `node.run()` is active | Pre `job_189f1590-3dc3-4a4b-bb6b-5194ea25ccd6`: Approve. Post `job_59439e2c-7e67-487c-8326-aa5e92172c7e`: Approve. | Pre `job_284d9afe-af86-4be4-8e29-ba39c3e4b872`: Approve. Post `job_c9ff7d73-ae89-49fa-830c-1cad76822a54`: Approve. | Red/green helper regression proves the run future is awaited after capture notification. |
| T037 | Status map current-entrypoint/provider-verifier refresh | Pre `job_7ca9414e-2d48-4c76-b714-91cffcd6f175`: Request changes on stale docs. Post `job_2714c825-ffb9-4097-bc06-cd77aed21580`: Approve. | Pre `job_4f4de962-1cad-4988-b4c9-fa689505f099`: Approve. Post `job_3be3e239-9c9c-4dad-a02a-a54d9578baa1`: Approve. | Stale rows refreshed and guarded by `scripts/verify_bolt_v3_status_map_current.py`. |
| T038 | Pure Rust runtime verifier | Pre `job_75c34b8e-8d0c-423b-8ad8-8aa10d5e5b9f`: Approve with verifier gap. Post `job_9c5e3e5c-abb5-44ff-ad5b-c8f40f11b920`: Approve. | Pre `job_bfd8e832-9415-4702-ad87-7910d8742054`: Approve. Post `job_7690ddf9-f302-4cbe-92de-1027cc344691`: Approve. | Dedicated source-scan verifier added and status-map row 3 refreshed. |
| T039 | Phase 8 one-live-order cap | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | Removed the code-owned one-order constant. Preflight consumes `[live_canary].max_live_order_count`; live proof accepts a positive admitted count up to that TOML-derived cap. |
| T040 | Binary-oracle order-shape policy | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve with non-blocking raw-direct-caller note. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | Removed the hardcoded combo gate and projects TOML order shape into the NT strategy order factory path; non-blocking direct raw-config note is accepted because production projection is typed and unsupported strings fail closed. |
| T060 | Updown cadence slug-token table and shape bounds | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve with non-blocking operator-token note. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | Moved cadence slug-token ownership to TOML and removed code table, minute-divisibility gate, and 32-character underlying bound; token misconfiguration is an operator/config risk, not a code-owned table. |
| T061 | Validation dispatch seams | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | Injected validation-binding paths are implemented for provider, market-family, and strategy-archetype registries; Gemini and Claude also approved with no blockers. |
| T062 | Provider WebSocket transport backend | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve with non-blocking config-upgrade note. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | `transport_backend` is required in provider TOML and mapped into NT data/execution configs; upgrade impact is accepted and documented. |
| T063 | Strategy fast-venue spot fallback and cross-market position pricing | Post `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`: Approve. | Post `job_7f550722-951f-4d8a-a0a6-85a011f7855f`: Approve. | Removed reference-fair-value fallback from strategy spot pricing and prevents position EV from using an active-market fast spot after rotation; Gemini and Claude also approved with no blockers. |
| T064 | Strategy instrument-suffix outcome inference | S5 did not cover the strategy file; post-implementation custom review `job_6578893c-c8b8-46ef-9ffc-f9c2c5d31606` approved with no blockers. Final follow-up review `job_b555bb7b-cb56-48a8-967f-e5c1f22a0c97` approved. | Post-implementation custom review `job_587f7d8c-9476-4d4c-b792-68138d7cc0a5` approved with no blockers. Final follow-up review `job_1978ca21-40aa-439e-acc5-5d1ac881594d` approved. | Accepted. Local implementation removes suffix parsing and adds source/behavior locks; Gemini `1d2adebf-3b50-481d-8bda-fa8fcc982cec` and Claude `897d7695-b57a-4c9f-8445-94f1b62a0253` also approved the `bf2ad6f` bundle with no blockers. |
| T065 | Legacy live-local `.POLYMARKET` instrument-id pin | S5 flagged broader legacy validator architecture caps as non-blocking future gates; post-implementation custom review `job_6578893c-c8b8-46ef-9ffc-f9c2c5d31606` approved with no blockers. Final follow-up review `job_b555bb7b-cb56-48a8-967f-e5c1f22a0c97` approved the added venue tests. | S5 flagged `.POLYMARKET` suffix coupling; post-implementation custom review `job_587f7d8c-9476-4d4c-b792-68138d7cc0a5` approved with no blockers and noted missing venue-error tests. Final follow-up review `job_1978ca21-40aa-439e-acc5-5d1ac881594d` approved the added venue tests. | Accepted for the instrument-id pin. Local implementation validates generic NT `symbol.venue` shape without venue pinning; Claude/GLM's test-gap note is addressed with explicit empty-venue and whitespace-venue tests. Broader legacy Phase-1 caps remain fenced/non-Bolt-v3 residuals. |
| T066 | Active Bolt-v3 adapter clock sentinel | Post-implementation custom review `job_6578893c-c8b8-46ef-9ffc-f9c2c5d31606` approved with no blockers. Final follow-up review `job_b555bb7b-cb56-48a8-967f-e5c1f22a0c97` approved. | Post-implementation custom review `job_587f7d8c-9476-4d4c-b792-68138d7cc0a5` approved with no blockers. Final follow-up review `job_1978ca21-40aa-439e-acc5-5d1ac881594d` approved. | Accepted. Local implementation removed the code-owned sentinel; active adapter mapping now derives `InstrumentFilterConfig` from strategy TOML and passes an NT `LiveClock` timestamp source. Gemini and Claude also approved the `bf2ad6f` bundle with no blockers. |

## Value And Policy Classification

| Class | Evidence | Disposition |
| --- | --- | --- |
| Config-owned runtime values | `src/strategies/binary_oracle_edge_taker.rs:59-85` declares order shape, warmup, period, cooldown, risk, EV, volatility, forced-flat, and lead-quality inputs as strategy config fields; usage flows through `self.config` at runtime. | Accept. Config-owned. |
| Config-owned production value | `src/bolt_v3_providers/polymarket.rs` maps `auto_load_debounce_milliseconds` from `[venues.<id>.data]` into NT `auto_load_debounce_ms`; `tests/fixtures/bolt_v3/root.toml` and `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` set the value, and provider-binding tests assert the mapped field. | Accept. T033 moved the prior hardcoded residual to TOML, and BV3-P9-001 closed the schema-doc drift. |
| Configured venue-key routing | `src/bolt_v3_validate.rs` no longer groups venues by provider kind. `src/bolt_v3_client_registration.rs` registers NT clients with the configured venue id, and provider validators still run per configured venue block. | T034 remediation removes the global one-venue-per-kind architecture cap and is externally reviewed at `b897dd6` by Gemini, Claude, DeepSeek, and GLM with no blockers. |
| Runtime-contract slug/protocol labels | `src/bolt_v3_market_families/updown.rs` owns the `updown` family key and slug template, while `target.cadence_slug_token` is TOML-owned. | T060 remediation removes the hardcoded cadence-to-token table and stale allowlist rows and is externally reviewed at `b897dd6`; token misconfiguration remains an operator/config risk, not a code-owned table. |
| Diagnostic text | Error messages and log fields in live node, secrets, verifier, and strategy code. | Accept when non-secret and not policy-bearing. |
| Test fixtures | `tests/fixtures/bolt_v3/**`, `tests/bolt_v3_*.rs`, strategy unit tests. | Accept as fixtures, not production values. |
| NT/API glue | Active provider mappings no longer use NT's default transport backend; `transport_backend` is TOML-owned for Polymarket data, Polymarket execution, and Binance data clients. The remaining `transport_backend: Default::default()` occurrence is a client-registration unit-test fixture. | Accept as test fixture only; not accepted as active Bolt-v3 runtime policy. |
| Bounded internal constants | Script buffer sizes, verifier sentinel values, and test-only line guards. | Accept when not runtime trading policy. |
| Legacy/shared defaults | The prior legacy/shared default surfaces are retired from the current source tree and source-fenced by `tests/bolt_v3_production_entrypoint.rs`. | Closed; not accepted as hidden Bolt-v3 runtime defaults. |

## Additional Source Dispositions

These dispositions were added during Phase 9 remediation follow-up to close broad-glob ambiguity raised by external review:

| Surface | Evidence | Disposition |
| --- | --- | --- |
| `src/bolt_v3_archetypes/mod.rs` | `VALIDATION_BINDINGS` and `RUNTIME_BINDINGS` bind only `binary_oracle_edge_taker`; validation now has an injected-binding path covered by a fake archetype test. | Local remediation adds a validation seam so tests and future modules do not require editing production registry tables. `RUNTIME_BINDINGS` remains explicit current-slice dispatch glue and was approved in T061 post-implementation review at `b897dd6`. |
| `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` | Runtime values flow from `[parameters]` fields into the existing NT strategy config; entry/exit order-shape TOML fields are projected as nested `entry_order` and `exit_order` strategy config tables. | Local remediation removes the hardcoded order-combo policy gate and the flat intermediate order projection. The remaining literals are schema labels and enum token serialization covered by the runtime-literal audit. T040 post-implementation review approved with no blockers. |
| `src/bolt_v3_adapters.rs` | Active adapter mapping derives `InstrumentFilterConfig` from strategy TOML and passes an NT `LiveClock` timestamp source to provider bindings. Tests can still supply an explicit `InstrumentFilterConfig` and clock. | Local T066 remediation removed the accepted `0_i64` clock sentinel and the stale runtime-literal allowlist entry. The current Phase 9 follow-up removes the production path that supplied an empty `InstrumentFilterConfig` and requires adapter mapping to consume configured strategy targets. |
| `src/bolt_v3_providers/mod.rs` | `PROVIDER_BINDINGS` registers Polymarket and Binance keys and routes validation/secrets/adapter mapping through provider modules; validation now has an injected-binding path covered by a fake provider test. | Local remediation adds a validation seam so provider validation is not coupled to production-only registry edits. Concrete provider keys remain schema labels/dispatch glue and were approved in T061 post-implementation review. |
| `src/bolt_v3_providers/binance.rs` | `KEY = "binance"`, `SUPPORTED_MARKET_FAMILIES = &[]`, no-execution validation, explicit URL fields, positive `instrument_status_poll_seconds`, and TOML-owned `transport_backend`. | Local remediation removes the NT transport-backend default residual; Binance reference-data capability gates remain explicit current-slice provider policy and were approved in T062 post-implementation review. |
| `src/bolt_v3_providers/polymarket.rs` | Polymarket data/execution mapping now requires TOML `transport_backend` and passes it through to NT data/execution config structs. | Local remediation removes the NT transport-backend default residual for active Bolt-v3 Polymarket data and execution clients. T062 post-implementation review approved with no blockers. |
| `src/bolt_v3_market_families/updown.rs` | Updown target validation now requires TOML `cadence_slug_token`, accepts any positive `cadence_seconds`, and plans the configured slug token directly. | Local remediation removes the prior hardcoded cadence-to-slug table, minute alignment policy, and 32-character underlying bound. T060 post-implementation review approved with no blockers; token misconfiguration remains an operator/config risk. |
| `src/bolt_v3_market_families/mod.rs` | `VALIDATION_BINDINGS` binds configured `market_family` labels to family validators; validation now has an injected-binding path covered by a fake family test. | Local remediation adds a validation seam so target validation can accept a new family binding without editing the production table. Static production labels remain explicit current-slice dispatch glue and were approved in T061 post-implementation review. |
| `src/strategies/binary_oracle_edge_taker.rs` | Strategy config still owns thresholds and sizing; entry spot pricing now requires a selected configured fast venue from lead-quality arbitration. `last_reference_fair_value` remains observable evidence, not a fallback price path. Position EV pricing additionally requires the managed position's market id to match the active market before using active fast spot. Outcome side is preserved from pending/active market context, not inferred from `InstrumentId` text suffixes. | Local T063 remediation removes the code-owned reference-fair-value fallback, prevents cross-market position pricing after rotation, and renames the decision log field away from fallback semantics. Local T064 remediation removes the hardcoded `-UP.`/`-DOWN.` suffix parser and adds source/behavior locks. T063 and T064 reviews approved with no blockers. |
| Retired legacy validator path | `src/validate.rs` is absent from the current source tree and fenced by `tests/bolt_v3_production_entrypoint.rs::codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules`. | Local T065 remediation was superseded by retiring the legacy validator path from current source. The current Phase 9 invariant is source-fenced absence, not acceptance of a non-Bolt-v3 validator residual. |
| `src/bolt_v3_client_registration.rs` | Registers configured venue adapter configs produced by provider mapping; summaries echo configured venue keys. | Accepted registry plumbing. No independent trading policy found beyond provider mapping outputs. |
| `src/bolt_v3_strategy_registration.rs` | Iterates loaded strategy configs and matches `strategy_archetype` against injected runtime bindings. | Accepted strategy registry plumbing. Unsupported strategy archetypes fail closed. |
| `src/bolt_v3_live_canary_gate.rs` | Gate consumes `[live_canary]` values; rejects zero order count/byte cap, parses decimal caps, validates no-submit readiness report, and requires named readiness stages. Test-only helper uses fixture values at `74-85`. | Config-owned gate values plus accepted readiness-contract stage labels. No code-owned live cap found outside the config checks. |
| `src/bolt_v3_submit_admission.rs` | Counts admitted submit attempts from armed `BoltV3LiveCanaryGateReport`, rejects non-positive notional, notional cap overflow, and count cap exhaustion. | Accepted runtime guard consuming config-derived report. No hardcoded cap value found. |
| `src/bolt_v3_tiny_canary_evidence.rs` | Env names `BOLT_V3_PHASE8_*`, evidence status/reason strings, schema version, 8 KiB hash buffer, and proof checks against `Phase8CanaryEvidenceInput.max_live_order_count`. | Local remediation removed the code-owned one-live-order cap; live proof now requires a positive admitted count no greater than the TOML-derived cap. Env and evidence field names are operator-envelope/schema labels; hash buffer is bounded internal constant. T039 post-implementation review approved with no blockers. |

## Severity-ranked Findings

### F1 - High - Current roadmap/spec docs are stale against current main

Evidence:
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md:69` still says no production caller for `build_bolt_v3_live_node` and marks the bolt-v3 binary/CLI entrypoint missing.
- `src/main.rs:56-57` now calls `build_bolt_v3_live_node` and `run_bolt_v3_live_node`.
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md:111` says provider-leak verifier is missing.
- `scripts/verify_bolt_v3_provider_leaks.py` exists, is 900 lines, and passed.
- `specs/001-thin-live-canary-path/plan.md:35-37` anchors on `a5c60f2b6a4fe67fc80cf9d234f1512af09bec03`.
- `specs/002-phase7-no-submit-readiness/spec.md:113`, `plan.md:35`, and `research.md:5` anchor on `d6f55774c32b71a242dcf78b8292a7f9e537afab`.

Impact: Live readiness decisions cannot rely on these docs without a current-main refresh.

Recommendation: Supersede or refresh stale rows/spec anchors with current-main evidence. Behavior lock: doc/source consistency check against `src/main.rs`, verifier files, and `git rev-parse HEAD`.

Remediation disposition: **Closed for Phase 9 T037**. The status map now reflects the current `src/main.rs` production run path and the existing provider-leak verifier, and `scripts/verify_bolt_v3_status_map_current.py` fails if the stale entrypoint/provider-verifier claims return.

### F2 - Medium - Polymarket provider debounce remains a production runtime residual

Evidence:
- `src/bolt_v3_providers/polymarket.rs:537` sets `auto_load_debounce_ms: 100`.
- The existing literal audit marks it `accepted_current_residual` at `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml:12-13`.

Impact: The verifier passes because the residual is accepted, but Phase 9's no-hardcode bar still needs a fresh decision: TOML-owned, NT-contract residual, or remove.

Recommendation: Move to config or re-accept with a named contract and test. Behavior lock: runtime literal verifier plus provider-binding regression test.

Remediation disposition: **Closed for Phase 9 T033**. The debounce is TOML-owned as `auto_load_debounce_milliseconds`, validated as positive, and asserted through provider-binding tests. The literal audit now classifies only the TOML field label, not a production numeric residual.

### F3 - Medium - One-venue-per-provider-kind was code-owned policy

Original evidence before local implementation:
- `src/bolt_v3_validate.rs` said the bolt-v3 scope was one venue per provider key and rejected more than one `[venues.<id>]` block per kind.

Impact: This is fail-closed and likely intentional for the slice, but it is still embedded policy. It affects config grouping and future multi-venue activation.

Recommendation: Remove the global provider-kind count gate if configured venue ids are already the routing identity. Behavior lock: validation tests must accept multiple same-kind venue ids while preserving per-provider block validation.

Remediation disposition: **Closed for Phase 9 T034**. The code-owned architecture cap was removed. Root validation now dispatches each configured venue block to its provider validator without counting provider kinds. `accepts_more_than_one_polymarket_venue_when_keys_are_distinct` covers the removed rejection. Gemini, Claude, DeepSeek, and GLM reviewed the exact-head `b897dd6` bundle with no blockers.

### F4 - Medium - Legacy/shared default config surfaces existed in required runtime-used files

Evidence:
- The prior surfaces were `src/live_config.rs`, `src/config.rs`, `src/clients/polymarket.rs`, `src/clients/chainlink.rs`, and `src/platform/polymarket_catalog.rs`.
- Current source removes those legacy runtime paths from `src/` and removes the legacy raw-capture/render binaries.
- `tests/bolt_v3_production_entrypoint.rs::codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules` fails if those paths return or if `src/lib.rs` re-exports the old modules.

Impact: Closed for current source. If a legacy path is reintroduced, the source fence fails.

Recommendation: Keep the source fence and runtime-literal/default gates in the local and CI check path.

Remediation disposition: **Closed for Phase 9 T035**. Legacy/shared default surfaces were retired rather than accepted as a second runtime path. `bolt_v3_production_path_cannot_load_legacy_config_defaults` source-fences `src/main.rs` and `src/bolt_v3_live_node.rs` against legacy config loaders, materializers, and legacy client modules. `codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules` source-fences file/module absence.

### F5 - Medium - Runtime-capture failure path needs an integrated test

Evidence:
- `src/bolt_v3_live_node.rs:419-454` wires runtime capture, takes a failure receiver, and uses `tokio::select!` to wait for either `node.run()` or the capture failure receiver.
- `src/nt_runtime_capture.rs:117-141` latches first failure, notifies the receiver, and calls `stop_handle.stop()`.
- `src/nt_runtime_capture.rs:184-209` reports the capture failure again during shutdown.
- `src/bolt_v3_live_node.rs:549-565` preserves combined `node.run()` and capture shutdown errors.
- Existing tests cover classification and failure-state notification, but no integrated test proves the `run_bolt_v3_live_node` select branch exits and reports `RuntimeCaptureShutdown` while `node.run()` is active.

Disposition: **Closed for Phase 9 T036**. The red test first proved the missing helper seam, then the implementation added `await_live_node_run_after_capture_failure_notification` so capture-failure notification no longer drops the active `node.run()` future. The helper also avoids logging a capture failure when the notification channel closes without a signal.

Recommendation: Keep the helper regression in place; any future full `LiveNode` integration test should build on this seam instead of reintroducing a second run/shutdown path.

### F6 - Low - Pure Rust and SSM-only posture is source-backed, but the pure-Rust verifier gap remains

Evidence:
- `src/main.rs:89-91` creates `SsmResolverSession` and calls `resolve_bolt_v3_secrets`.
- `src/bolt_v3_live_node.rs:394-405` rejects forbidden credential env vars and resolves secrets through `SsmResolverSession`.
- `src/bolt_v3_secrets.rs:70-76` only uses `std::env::var_os` for the forbidden-env check, not as a secret fallback.
- `src/secrets.rs:186-203` includes a test proving the production resolver does not shell out to AWS CLI and does use `aws_sdk_ssm`.
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md:65` still says there is no dedicated pure-Rust verifier.

Impact: Source evidence supports the rule, but the status map is correct that there is no dedicated standalone verifier for the whole pure-Rust runtime rule.

Recommendation: Add a dedicated verifier only if this gate remains required for release decisions.

Remediation disposition: **Closed for Phase 9 T038**. `scripts/verify_bolt_v3_pure_rust_runtime.py` checks Cargo dependencies and Bolt-v3 runtime source paths for PyO3, maturin, Python runtime subprocesses, and AWS CLI subprocesses, and verifies that status-map row 3 no longer claims the gate is missing.

### F7 - Medium - Phase 8 tiny-canary evidence hardcoded one admitted live order

Original evidence before local implementation:
- `src/bolt_v3_tiny_canary_evidence.rs` set `PHASE8_REQUIRED_LIVE_ORDER_CAP: u32 = 1`.
- Preflight blocked unless `[live_canary].max_live_order_count` equaled that constant.
- Live proof rejected unless `admitted_order_count` equaled that constant.

Impact: This is aligned with the current "one tiny live canary" phase, but it is still code-owned policy. It can silently outlive Phase 8 unless named as a temporary contract.

Recommendation: Re-accept with a named Phase 8 one-order contract and retirement condition, or move the proof expectation to a TOML/config-evidence field. Behavior lock: focused Phase 8 evidence tests plus literal verifier classification.

Remediation disposition: **Closed for Phase 9 T039**. The code-owned cap was removed. Preflight now uses the TOML-derived `[live_canary].max_live_order_count`; live proof accepts a positive admitted submit count up to that cap and rejects zero or above-cap evidence. Behavior is covered by `preflight_accepts_toml_owned_live_order_count_before_build`, `live_canary_evidence_accepts_admitted_count_below_configured_cap`, `live_canary_evidence_rejects_zero_submit_admission_count`, and `live_canary_evidence_rejects_admitted_count_above_configured_cap`.

### F8 - Medium - Strategy archetype order-shape policy was code-owned

Original evidence before local implementation:
- `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` only allowed entry `order_type=limit`, `time_in_force=fok`, and exit `order_type=market`, `time_in_force=ioc`, with boolean flags fixed false.
- The restricted fields were already present in the TOML-owned `OrderParams` shape.

Impact: This is fail-closed and likely intentional for the current strategy, but Phase 9's policy-hardcode definition includes strategy-specific branches and fixed order-shape behavior.

Recommendation: Treat as accepted current-archetype policy unless strategy-generalization is in scope. Do not remove speculatively; if changed, first add validation tests that prove the intended config contract.

Remediation disposition: **Closed for Phase 9 T040**. The hardcoded entry/exit combo rejection was removed. The archetype still owns TOML shape parsing, but order shape now projects into nested `entry_order`/`exit_order` config tables consumed by the existing `binary_oracle_edge_taker` NT strategy config and order factory path. Tests cover validation acceptance, raw config projection, nested strategy parsing, entry limit order construction, and exit market order construction. Gemini, Claude, DeepSeek, and GLM reviewed the exact-head `b897dd6` bundle with no blockers; raw/direct strategy callers remain fail-closed if they bypass the archetype projection.

### F9 - Medium - Legacy/shared default surfaces were outside the runtime-literal verifier scope

Evidence:
- `scripts/verify_bolt_v3_runtime_literals.py` now scans the current production source universe, including Bolt-v3 root/module files, strategy modules, and runtime support modules.
- A full `src/**/*.rs` literal scan after the update reported zero unclassified literals.
- F4 legacy/shared paths are retired and source-fenced.

Impact: Closed for current source. Literal regressions in current runtime-support files fail the verifier; legacy path reintroduction fails the production-entrypoint source fence.

Recommendation: Keep both gates: runtime-literal coverage for current files and production-entrypoint absence fence for retired paths.

### F10 - Low - Chainlink reconnect interval constants were not explicitly classified

Evidence:
- The prior constants lived in retired `src/clients/chainlink.rs`.
- Current source removes `src/clients/chainlink.rs`.

Impact: Closed by retirement of the legacy Chainlink client path. Future Chainlink provider work must enter through Bolt-v3 provider bindings and TOML-owned runtime config.

Recommendation: Keep the retired-path source fence; do not reintroduce Chainlink reconnect policy outside a provider binding and TOML config.

### F11 - Low - Archetype-to-provider fee-provider coupling

Evidence:
- Earlier exact heads imported `bolt_v3_providers::polymarket` from `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` and called `polymarket::build_fee_provider(...)` directly.
- Current follow-up moves fee-provider construction onto `bolt_v3_providers::ProviderBinding::build_fee_provider`; `binary_oracle_edge_taker` now resolves the provider binding by configured `venue.kind` and invokes that binding without naming a concrete provider.
- The concrete Polymarket fee provider now lives under `src/bolt_v3_providers/polymarket/fees.rs` and implements the generic strategy `FeeProvider` trait directly, without importing the legacy `clients::polymarket` module.
- `fee_cache_ttl_seconds` is TOML-owned under `[venues.<id>.execution]`, validated as a positive execution value, and classified in the runtime-literal audit.
- `tests/bolt_v3_provider_binding.rs` now guards both sides: the strategy registry and archetype cannot import/name the concrete Polymarket fee provider, while the provider root cannot import the concrete Polymarket fee client type.

Impact: The strategy archetype no longer owns concrete provider dispatch. The provider root still registers provider keys and binding functions, which is the intended provider-binding surface for this slice.

Remediation disposition: **Accepted and fixed in the follow-up implementation after external review.** Behavior lock: keep provider-specific fee-client imports inside the concrete provider module and keep archetypes calling the provider binding surface rather than concrete provider modules.

### F12 - High - Config loaders used unbounded reads for user-configurable TOML files

Evidence:
- Gemini Code Assist flagged legacy config loaders that have since been retired from current source.
- Follow-up inspection found the active Bolt-v3 loader also used unbounded reads before root and strategy TOML parsing.

Disposition: **Accepted and fixed**. `src/bounded_config_read.rs` now centralizes a bounded config-file read helper; `src/bolt_v3_config.rs` uses it for root and strategy TOML. Oversized root and strategy config files fail closed before TOML parsing. BV3-P9-CONFIG-001 is dispositioned by documenting the 1 MiB pre-parse operator-config size guard as a resource-exhaustion guard, not a trading-policy parameter.

Behavior locks:
- `load_bolt_v3_config_rejects_oversized_root_before_parse`
- `load_bolt_v3_config_rejects_oversized_strategy_before_parse`

### F13 - High - Expired markets produced a step-function fair probability

Evidence:
- Gemini Code Assist flagged `src/strategies/binary_oracle_edge_taker.rs:3971-3977`: `compute_fair_probability_up` returned `Some(1.0)`, `Some(0.0)`, or `Some(0.5)` when `seconds_to_expiry == 0`.

Impact: Entry evaluation could receive an apparently valid probability for an expired market instead of treating fair probability as unavailable.

Disposition: **Accepted and fixed**. `compute_fair_probability_up` now returns `None` when `time_to_expiry_years <= 0.0`.

Behavior lock:
- `fair_probability_helper_fails_closed_when_expired`

## Remediation Verification

Reran after refreshing the branch onto `origin/main` `fde50d3452859a51f7f27b807913b1f12697b273` and updating Phase 9 artifacts.

| Command | Result |
| --- | --- |
| `cargo test oversized -- --nocapture` | Passed; covers oversized Bolt-v3 root, oversized Bolt-v3 strategy, oversized legacy runtime config, and oversized live-local materialization input. |
| ~~`cargo test --test render_live_config materialize_live_config_updates_oversized_drifted_output -- --nocapture`~~ | **SUPERSEDED at current head `9fb1a239`** — `tests/render_live_config.rs` and `src/bin/render_live_config.rs` retired under T068; named test is unreachable. Oversized fail-closed property preserved by `cargo test oversized` above. |
| ~~`cargo test --test render_live_config -- --nocapture`~~ | **SUPERSEDED at current head `9fb1a239`** — test binary retired under T068. |
| `cargo test fair_probability_helper_fails_closed_when_expired -- --nocapture` | Passed. |
| `git diff --check` | Passed. |
| `cargo fmt --check` | Passed. |
| `just fmt-check` | Passed; now runs runtime-literal, provider-leak, status-map-current, and pure-Rust-runtime verifiers before `cargo fmt --check`. |
| `python3 scripts/verify_bolt_v3_runtime_literals.py` | Passed. |
| `python3 scripts/verify_bolt_v3_provider_leaks.py` | Passed. |
| `python3 scripts/verify_bolt_v3_core_boundary.py` | Passed. |
| `python3 scripts/verify_bolt_v3_naming.py` | Passed. |
| `python3 scripts/verify_bolt_v3_status_map_current.py` | Passed. |
| `python3 scripts/test_verify_bolt_v3_pure_rust_runtime.py` | Passed; proves `#[cfg(test)]` stripping does not truncate later production code. |
| `python3 scripts/verify_bolt_v3_pure_rust_runtime.py` | Passed; includes runtime-capture and strategy modules. |
| `cargo test --test bolt_v3_provider_binding -- --nocapture` | Passed; 7 tests. |
| `cargo test --test config_parsing -- --nocapture` | Passed; 43 tests. |
| `cargo test --test bolt_v3_production_entrypoint -- --nocapture` | Passed; 3 tests. |
| `cargo test --lib capture_failure_notification_waits_for_active_live_node_run_result -- --nocapture` | Passed. |
| `cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture` | Passed; 17 tests. |
| `cargo test --test bolt_v3_controlled_connect -- --nocapture` | Passed; 11 tests. |
| `cargo clippy --all-targets -- -D warnings` | Passed after rerun outside sandbox; first attempt was blocked by Cargo target-cache lock permissions before analysis. |
| `cargo test --lib validation_can_use_injected -- --nocapture` | Passed locally for T061; fake provider, market-family, and archetype validation bindings work without editing production registry tables. |
| `cargo test --lib builder_accepts_nested_order_shape_without_flat_order_projection -- --nocapture` | Passed locally for T040; nested `entry_order`/`exit_order` config is accepted without flat order projection fields. |
| `cargo test --test bolt_v3_strategy_registration -- --nocapture` | Passed locally after T040 nested order projection update. |
| ~~`cargo test --test config_schema -- --nocapture`~~ | **SUPERSEDED at current head `9fb1a239`** — `tests/config_schema.rs` retired under T069. Order-template acceptance covered by `tests/config_parsing.rs`. |
| ~~`cargo test --test render_live_config -- --nocapture`~~ | **SUPERSEDED at current head `9fb1a239`** — test binary retired under T068. Order-render acceptance covered by `tests/bolt_v3_strategy_registration.rs`. |
| `cargo test --test binary_oracle_edge_taker_runtime -- --test-threads=1 --nocapture` | Passed locally after T040 nested order runtime update; entry and exit order construction still works through NT runtime tests. |
| `cargo test --test bolt_v3_adapter_mapping -- --nocapture` | Passed locally after T062; configured `transport_backend` reaches Polymarket data, Polymarket execution, and Binance data NT config structs. |
| `cargo test --test config_parsing -- --nocapture` | Passed locally after T062 required `transport_backend` schema update. |
| `cargo test --lib pricing_state_requires_fast_spot_for_pricing_and_keeps_reference_separate -- --nocapture` | Red first, then passed locally after T063; reference fair value is retained separately but no longer prices entries without a selected fast venue. |
| `cargo test --lib entry_evaluation_log_fields_fail_closed_without_fast_spot -- --nocapture` | Red first, then passed locally after T063; entry evaluation reports `SpotPriceMissing` when no fast venue is selected even if reference fair value exists. |
| `cargo test --lib pricing_state_applies_lead_quality_thresholds -- --nocapture` | Passed locally after T063; lead-quality rejection clears fast spot and pricing fails closed. |
| `cargo test --lib pricing_state_clears_fast_spot_when_no_fast_venue_remains -- --nocapture` | Passed locally after T063; removing fast venue availability clears pricing rather than falling back. |
| `cargo test --lib task6_entry_evaluation_blocks_when_realized_vol_is_not_ready -- --nocapture` | Passed locally after T063 to recheck unchanged fast-spot entry blocking behavior. |
| `cargo test --lib exit_evaluation_log_fields_use_position_context_after_rotation -- --nocapture` | Passed locally after T063; rotated position log fields do not use the new active market's fast spot for old position EV. |
| `cargo test --test binary_oracle_edge_taker_runtime binary_oracle_edge_taker_runtime_keeps_exit_path_for_market_a_position_after_rotation_to_market_b -- --test-threads=1 --nocapture` | Passed locally after T063; after rotation the old market position exits fail-closed without cross-market hold-EV pricing. |
| `cargo test --lib strategies::binary_oracle_edge_taker::tests -- --nocapture` | Passed locally after T063; 117 strategy unit tests. |
| `cargo test --test binary_oracle_edge_taker_runtime -- --test-threads=1 --nocapture` | Passed locally after T063; 10 runtime integration tests. |

Earlier T064/T065/T066 local batch verification on committed base `c2d05fda5a5704c0270a081e7ce8976a2a05c427` plus the then-uncommitted T066 diff:

| Command | Result |
| --- | --- |
| `git merge-base --is-ancestor origin/main HEAD` | Passed. |
| `git diff --check` | Passed. |
| `python3 scripts/test_verify_bolt_v3_runtime_literals.py` | Passed: `OK: Bolt-v3 runtime literal verifier self-tests passed.` |
| `python3 scripts/verify_bolt_v3_runtime_literals.py` | Passed: `OK: Bolt-v3 runtime literal audit passed.` |
| `python3 scripts/verify_bolt_v3_provider_leaks.py` | Passed: `OK: Bolt-v3 provider-leak verifier passed.` |
| `python3 scripts/verify_bolt_v3_core_boundary.py` | Passed: `OK: Bolt-v3 core boundary audit passed.` |
| `python3 scripts/verify_bolt_v3_naming.py` | Passed: `OK: Bolt-v3 canonical naming audit passed.` |
| `python3 scripts/verify_bolt_v3_pure_rust_runtime.py` | Passed: `OK: Bolt-v3 pure-Rust runtime verifier passed.` |
| `python3 scripts/verify_bolt_v3_status_map_current.py` | Passed: `OK: Bolt-v3 status map matches current entrypoint and verifier evidence.` |
| `cargo fmt --check` | Passed. |
| `cargo test --lib bolt_v3_adapters::tests::no_identity_adapter_mapping_source_does_not_install_zero_clock_sentinel -- --nocapture` | Red first against the `0_i64` sentinel, then passed after T066. |
| `cargo test --lib bolt_v3_adapters::tests -- --nocapture` | Passed; 11 tests. |
| `cargo test --test bolt_v3_adapter_mapping -- --nocapture` | Passed; 7 tests. |
| `cargo test --test bolt_v3_provider_binding -- --nocapture` | Passed; 7 tests. |
| `cargo test --lib validate::tests::instrument_id -- --nocapture` | Passed after Claude/GLM follow-up; 6 tests, including empty and whitespace venue suffix rejection. |
| `cargo test --lib strategies::binary_oracle_edge_taker::tests -- --nocapture` | Passed; 118 strategy unit tests. |
| `cargo test --test binary_oracle_edge_taker_runtime binary_oracle_edge_taker_runtime_exits_recovered_numeric_down_position_by_selling_held_down_at_best_bid -- --test-threads=1 --nocapture` | Passed; recovered DOWN position still exits by selling held DOWN at best bid. |
| `cargo clippy --all-targets -- -D warnings` | Passed after rerun outside sandbox; first attempt was blocked by Cargo target-cache lock permissions before analysis. |

## Strategy And Feed Assumptions

- `binary_oracle_edge_taker` runtime strategy values are mostly config-owned through the config macro at `src/strategies/binary_oracle_edge_taker.rs:59-75`.
- Runtime decisions use `self.config` for warmup, lead quality, forced flat, theta decay, EV threshold, risk sizing, and impact cap.
- T063 removes the code-owned fallback from selected fast venue to reference fair value. Entry pricing now fails closed with missing spot price unless lead-quality arbitration selects a configured fast venue; managed-position EV also refuses to use active fast spot after market rotation unless the position market id still matches the active market. Reference fair value remains logged as separate context.
- T064 removes outcome-side inference from hardcoded instrument-id suffixes; recovery/bootstrap positions stay fail-closed until pending-entry or active-market context supplies side.
- T066 removes the adapter clock sentinel; rotating-market filters require an injected real clock through `map_bolt_v3_adapters_with_instrument_filters`, and active adapter mapping derives filters from strategy TOML.
- No tiny live order approval should rely on strategy math or feed assumptions without a separate current-head no-submit and strategy-input safety proof.

## Live Ops Readiness

Current main has stronger build and verifier evidence than older docs claimed. Phase 9 T033 and T035-T038 remediation closed the specific audit scope for stale entrypoint/provider-verifier docs, the Polymarket debounce residual, legacy default path ambiguity, runtime-capture notification coverage, and pure-Rust verifier coverage. T034/T039/T040/T060/T061/T062/T063 now have local implementations for the one-venue architecture cap, Phase 8 live-order cap, binary-oracle order-shape policy, updown cadence slug-token table, injected validation dispatch seams, TOML-owned provider WebSocket transport backend, removal of the strategy fast-venue fallback path, and prevention of cross-market position pricing after rotation. T064/T065 additionally remove the accepted suffix-parser and `.POLYMARKET` legacy-validator hardcodes. T066 removes the active Bolt-v3 adapter clock sentinel. Follow-up T064-T066 external review at `bf2ad6f` completed with no blockers; the accepted T065 venue-error tests and review-disposition evidence passed exact-head CI at `535f973` and narrow DeepSeek/GLM follow-up review with no blockers.

Live order remains blocked by:

- No approved live capital action.
- No current-head proof of no-submit real SSM/venue readiness in this audit.
- Remaining status-map gaps outside T033-T040, including reconciliation, order lifecycle, restart recovery, execution gate, deploy trust, panic gate, live canary, and production operation.

## Cleanup Candidates With Behavior Locks

| Candidate | Status | Behavior lock |
| --- | --- | --- |
| Refresh stale docs/specs to current main. | Closed for T037. | `scripts/verify_bolt_v3_status_map_current.py`. |
| Resolve Polymarket debounce residual. | Closed for T033. | Runtime literal verifier, provider-binding test, and zero-value config validation test. |
| Remove one-venue-per-kind architecture cap. | Closed for T034; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers. | Config validation accepts multiple same-kind venue ids while provider validation still runs per key. |
| Fence or retire legacy config defaults. | Closed for T035. | Source fence proving bolt-v3 production run cannot load legacy config/default paths. |
| Add runtime-capture integrated test. | Closed for T036 helper seam. | Red/green helper regression for capture notification while the run future is active. |
| Add pure-Rust runtime verifier. | Closed for T038 and BV3-P9-003. | Source scan for Python runtime layer, PyO3, maturin, AWS CLI subprocess, non-Rust secret path, runtime-capture, and strategy modules. |
| Move Phase 8 one-live-order cap. | Closed for T039; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers. | `preflight_accepts_toml_owned_live_order_count_before_build`, updated live-proof count test, and runtime literal verifier. |
| Generalize binary-oracle order-shape policy. | Closed for T040; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers. | Config validation acceptance tests, raw config projection test, and NT runtime entry/exit order-construction tests. |
| Move updown cadence slug-token table to TOML. | Closed for T060; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers; token misconfiguration remains an operator/config risk. | TOML `cadence_slug_token`, configured-token instrument-filter test, provider-binding order test, and runtime literal verifier. |
| Generalize validation dispatch seams. | Closed for T061; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers. | Injected provider/family/archetype validation-binding tests without production registry edits. |
| Move provider WebSocket transport backend to TOML. | Closed for T062; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers; required config upgrade is accepted and documented. | Adapter mapping tests assert configured Polymarket data, Polymarket execution, and Binance data backend values reach NT config structs. |
| Remove strategy fast-venue fallback pricing path. | Closed for T063; exact-head Gemini, Claude, DeepSeek, and GLM review approved with no blockers. | Red/green tests prove reference fair value is logged separately but no longer used as spot price without selected fast venue, and rotated positions do not use the new active market's fast spot for old position EV. |
| Remove strategy instrument-suffix side inference. | Local implementation, four-provider external review, exact-head CI, and final narrow follow-up review complete. | Source fence rejects production `-UP.`/`-DOWN.` suffix parsing and a no-context position event remains side-unknown. |
| Remove legacy live-local `.POLYMARKET` instrument-id pin. | Local implementation, four-provider external review, exact-head CI, and final narrow follow-up review complete; Claude/GLM venue-test gap addressed. | `instrument_id_accepts_any_non_empty_nt_venue_suffix` plus missing, empty, and whitespace component rejection tests. |
| Remove adapter clock sentinel. | Local implementation, four-provider external review, exact-head CI, and final narrow follow-up review complete. | Active adapter mapping derives `InstrumentFilterConfig` from strategy TOML and passes an NT `LiveClock` timestamp source; runtime-literal audit rejects stale sentinel allowance. |

## Final Exact-head Follow-up Verification

Reviewed implementation baseline before final status/follow-up work: `0e4e4a7e8b148819be4fc685f6f3df7ceb18297c`.

| Gate | Result |
| --- | --- |
| GitHub Actions run `25950963809` | Green for exact head: `fmt-check`, `deny`, `clippy`, `test`, `build`, `gate`, CodeQL, detector, Analyze actions, and Analyze rust passed. |
| `python3 scripts/test_verify_runtime_capture_yaml.py` | Passed; 5 tests. |
| `cargo test --test config_parsing rejects_polymarket_execution_max_retries_above_nt_u32_at_startup_validation -- --nocapture` | Passed. |
| `cargo test --test config_parsing -- --nocapture` | Passed; 49 tests. |
| `just fmt-check` | Passed; includes runtime-literal, provider-leak, status-map-current, pure-Rust-runtime, legacy-default, strategy-policy, and runtime-capture YAML self-test gates. |
| `just clippy` | Passed. |
| `git diff --check` | Passed. |
| Final exact-head external review | Grok, Gemini, GLM, and Claude approved the final delta with no blocking findings. Kimi timed out. DeepSeek auth remained HTTP 401. |
| Status-sync head `d606a57d552bbb63d54f803992ceb6b7c613f50a` | Exact-head CI run `25951771631` passed `fmt-check`, `deny`, `clippy`, `test`, `build`, `gate`, CodeQL, detector, Analyze actions, and Analyze rust. |
| GLM exact-head shard review after `d606a57` | GLM approved three custom shards: production code/config `job_1ff49032-d375-4cc0-9615-d1b6d6efa5a4`, tests `job_04d3bee3-8f26-49b1-9f1e-33d9e54a7f17`, and verifiers/docs/status `job_0043e70e-24dc-43c7-92de-06a69ed44202`. No blocking findings. |
| GLM SSM raw-value concern | Accepted. Production `SsmResolverSession::resolve` trimmed SSM values before `bolt_v3_secrets::resolve_field` could reject leading/trailing whitespace. Follow-up commit `b31bfee` removes the trim and adds a source guard test. |
| DeepSeek retry after provider reset | Source-free doctor still failed with HTTP 401 `auth_rejected`; no source was sent. |
| SSM raw-value local verification | Red/green targeted test, `cargo test --lib`, `cargo fmt --check`, `git diff --check`, and all Phase 9 verifiers passed locally. |

## Current-head Re-anchor (2026-05-17)

The branch was force-pushed after the External Review Status section above was last updated. **Production-code head at this section's authoring: `9fb1a239cfc046f8446b10a5724aa343b7f86c2a`.** Docs-only fix commits added after this section may have advanced the literal HEAD without changing code behavior — see the PR body's "Current pushed head" line for the authoritative current literal HEAD SHA. Prior head `fc7e081e254a56d4578cf471c00842a63c1eb778` is superseded — `fc7e081` and `9fb1a239` share merge-base `cece0f22c6b0e2a0c9141fd7325f720bff452911` (pre-Phase-9 main) and are divergent branches.

| Item | Status |
| --- | --- |
| Exact-head CI run `25972314453` at `9fb1a239` | Green: `fmt-check`, `deny`, `clippy`, `test`, `build`, `gate`, CodeQL, detector, Analyze actions, Analyze rust, source-fence, nextest 1/4–4/4 all passed. `dependabot` skipped. |
| External-review approvals logged in this report | **Cover superseded SHA `fc7e081` only.** All Grok/Gemini/GLM/Claude/Kimi/DeepSeek and shard-review SHAs cited above (`fc7e081`, `d606a57`, `b31bfee`, `b897dd6`, `bf2ad6f`, `535f973`, intermediate fcXXX/cXXX SHAs) precede the current head. Current head delta contains the additional commits `7dbb45c4`, `580cd417`, `ee2b2ae9`, `32006923`, `5373f4a7`, `79a4f543`, `7b24b990`, `317644c4`, `ddd96829`, `6a1c063f`, `9fb1a239`. |
| SSM raw-value preservation | Code property preserved at current head: `src/secrets.rs::SsmResolverSession::resolve` retains the no-trim guard test `ssm_resolver_session_does_not_trim_resolved_secret_values`; `src/bolt_v3_secrets.rs::resolve_field` retains `rejects_whitespace_padded_resolved_secret_values_without_trimming`. The originating commit `b31bfeea` is no longer in the branch history (force-pushed); the code property was reapplied in the head commits. |
| Pending external review (T074) | Re-run Claude/Gemini/Kimi/GLM/DeepSeek wave at the current PR head as published in the PR body's "Current pushed head" line. Production-code SHA at task authoring: `9fb1a239`; docs-only commits since do not change code behavior, but the review SHA MUST match the current literal HEAD before approvals can close T074. No external approval currently covers the actual PR head. |
| Retrospective scope reconciliation (T067–T073) | See `tasks.md` Retrospective Scope Reconciliation section: documents `src/platform/**` retirement, capture/render-binary retirement, legacy validation retirement, `src/bolt_v3_market_identity.rs` retirement, new operator example configs, Polymarket fee-provider extraction, and shared-runtime alignment fallout. Closes P0-C/D/G/H/I traceability gaps from MECE review packet P0. |
| Merge gate (FR-007) | This PR must not be merged while Phase 9 audit/remediation is open per FR-007 ("Audit/remediation MUST NOT ... merge"). State: OPEN, intentionally awaiting at-head external review + explicit merge approval; not "ready to merge" despite mergeable posture. |

## Decision

**Current status: Phase 9 hardcode/dual-path implementation is final for PR #331 after the SSM raw-value follow-up.** The older T033-T040 and T060-T066 baseline was externally reviewed through `b897dd6`/`535f973`; the final follow-up replaced stale prior vocabulary with NT/config vocabulary, removed the production empty-`InstrumentFilterConfig` path, decoupled strategy fee-provider wiring from Polymarket, corrected stale status-map rows that underreported current provider, strategy construction, selection, risk, order construction, and canary-gate evidence, added final review-driven cleanup for runtime-capture verifier wiring plus Polymarket `max_retries` startup validation, and preserved raw SSM secret values so the bolt-v3 secret resolver owns whitespace rejection.

This does not approve live capital. No-submit-only work can continue as audit/readiness and controlled local evidence work. Real no-submit SSM/venue readiness and any live-capital action require separate explicit approval, exact command, exact current head, and redacted evidence capture.
