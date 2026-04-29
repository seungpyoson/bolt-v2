# Bolt-v3 Source-Grounded Status Map

Date: 2026-04-28

Status: review draft.

Purpose: make the Bolt-v3 roadmap auditable from the repo. This file is not a
product promise and not a merge checklist. A row is only marked implemented when
there is source evidence plus test/verifier evidence. Existing code that works
only for the current provider/archetype/family stack is marked partial, not done.
The row numbers are a dependency-ordered map, not a percentage-complete score:
later rows may have old or partial code before earlier architecture rows are
clean.
Citations of `src/bolt_v3_*` modules are not exhaustive; absence of a file from
this map is not evidence that the file is empty or irrelevant.
Upstream NautilusTrader history is referenced where relevant; verify upstream
commit identifiers against `https://github.com/nautechsystems/nautilus_trader.git`
before treating them as Bolt prerequisites.

## Current Read

Bolt-v3 is still in foundation work. The safest source-backed position is:

- Core TOML parsing, validation scaffolding, SSM direction, adapter mapping,
  client registration, controlled connect/disconnect, runtime capture
  (shared/legacy reuse), and pure market identity have code and tests.
- The provider-neutral architecture is not complete. The latest correction only
  removed closed provider/archetype/family identity dispatch from the first core
  layer.
- The baseline must not be treated as ready for merge to `main` as a general
  Bolt-v3 foundation until the remaining provider-specific core-adjacent
  boundaries are addressed or explicitly accepted as scoped residuals.
- This branch pins NautilusTrader to
  `56a438216442f079edf322a39cdc0d9e655ba6d8`, the audited upstream commit which
  migrates the Polymarket adapter to CLOB V2. The pin-change audit and
  compatibility probe are recorded under
  `docs/bolt-v3/research/nt-pin-change/`. This reduces the former upstream
  support blocker, but it does not prove Bolt live CLOB V2 execution readiness.

## Roadmap State

| # | Area | Status | Source Evidence | Test / Verifier Evidence | Gap / Next Decision |
|---:|---|---|---|---|---|
| 1 | Rust NT-native LiveNode boundary | Partial | `src/bolt_v3_live_node.rs`, `src/bolt_v3_client_registration.rs` | `tests/live_node_run.rs`, `tests/bolt_v3_client_registration.rs` | Build/register boundaries exist, but strategy construction, readiness, and execution are not present. |
| 2 | TOML-owned runtime configuration | Partial | `src/bolt_v3_config.rs` carries root, venue, strategy, timeout, persistence, risk, complete explicit NT data-engine fields, complete explicit NT exec-engine fields, and complete explicit NT risk-engine fields; `src/bolt_v3_live_node.rs` explicitly fixes `LoggerConfig` residuals and the remaining top-level `LiveNodeConfig` residuals to accepted default or disabled/empty current behavior | `tests/config_parsing.rs`; `src/bolt_v3_live_node.rs::live_node_config_maps_explicit_nt_runtime_defaults_from_v3_root`; `src/bolt_v3_live_node.rs::live_node_config_maps_explicit_logger_residuals_in_builder_path`; Rust struct literal exhaustiveness for `LiveNodeConfig` | Full runtime hardcode audit is still missing, including non-v3 runtime paths. |
| 3 | No Python runtime layer | Missing verifier | Repo rule requires pure Rust binary | No dedicated verifier found | Add a CI-verifiable check for no PyO3/maturin/Python runtime layer, or stop tracking this as a proven fact. |
| 4 | Config schema and parser | Implemented for current schema | `src/bolt_v3_config.rs` defines root/strategy config and `load_bolt_v3_config` | `tests/config_parsing.rs` | Schema can evolve, but current parser/validator exists. |
| 5 | Runtime values come from TOML | Partial | `src/bolt_v3_config.rs` carries timeout, persistence, risk, venue, strategy, complete explicit NT data-engine fields, complete explicit NT exec-engine fields, and complete explicit NT risk-engine fields; `LoggerConfig` and top-level `LiveNodeConfig` residuals are explicit accepted default or disabled/empty settings instead of inherited defaults; `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` classifies current candidate runtime literals in production `src/bolt_v3_*.rs` | Config parsing tests cover selected parser/validator cases; `src/bolt_v3_live_node.rs::live_node_config_maps_explicit_nt_runtime_defaults_from_v3_root` asserts the NT mapping; `src/bolt_v3_live_node.rs::live_node_config_maps_explicit_logger_residuals_in_builder_path` asserts logger residuals; `scripts/verify_bolt_v3_runtime_literals.py` runs through `just verify-bolt-v3-runtime-literals` and `just fmt-check` | Literal verifier exists for production `src/bolt_v3_*.rs`; status remains partial because non-v3 runtime paths and accepted provider-boundary residual literals are still tracked separately. Single-value enums (`RuntimeMode::Live`, `OmsType::Netting`, `CatalogFsProtocol::File`, `RotationKind::None`) are scope constraints that must be widened or explicitly accepted before dry-run/shadow modes. `RuntimeMode::Live`-only blocks rows 40 and 42 at the config level. |
| 6 | Canonical `just check` validation path | Missing | `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` requires one structural/live validation entrypoint | No `just check` implementation found in `justfile`; `just live-check` exists but validates secret-config completeness only | Build this before claiming operator readiness. |
| 7 | Bolt-v3 binary / CLI entrypoint | Missing | `Cargo.toml` `[package].name = "bolt-v2"` with no `[[bin]]` for Bolt-v3; `src/bolt_v3_live_node.rs` exposes library functions only | No production caller for `build_bolt_v3_live_node` found outside tests | Required before any canary path. Current `just live` runs `bolt-v2 -- run`; a Bolt-v3 binary entrypoint must also update the operator launch path. |
| 8 | SSM-only secret source | Partial | `src/bolt_v3_secrets.rs`, `src/bolt_v3_providers/polymarket.rs`, `src/bolt_v3_providers/binance.rs`, `src/secrets.rs` shared SSM resolver session | Unit tests in `src/bolt_v3_secrets.rs`, config parsing tests | Secret code is provider-shaped across resolver plus per-provider validation modules. `ResolvedBoltV3VenueSecrets` is still a closed provider enum; provider-owned secret binding remains open. |
| 9 | Core provider identity is configured key, not closed enum | Implemented in first correction slice | `src/bolt_v3_config.rs` uses `ProviderKey` in the verified Bolt-v3 core boundary | `scripts/verify_bolt_v3_core_boundary.py` | No `VenueKind` enum remains in the verified Bolt-v3 core boundary. Legacy venue-kind enums remain in non-v3 modules. Residual closed dispatch remains in adapter/secrets/registration paths; see rows 14-17. |
| 10 | Core archetype identity is configured key, not closed enum | Implemented in first correction slice | `src/bolt_v3_config.rs` uses `StrategyArchetypeKey` in the verified Bolt-v3 core boundary | `scripts/verify_bolt_v3_core_boundary.py` | No `StrategyArchetype` enum remains in the verified Bolt-v3 core boundary. Archetype construction is still not done. |
| 11 | Provider validation dispatch | Partial | `src/bolt_v3_providers/mod.rs`, `src/bolt_v3_providers/polymarket.rs`, `src/bolt_v3_providers/binance.rs` | `tests/config_parsing.rs`, `tests/bolt_v3_provider_binding.rs` | Per-provider validation modules exist; provider list is still statically registered in `mod.rs`. |
| 12 | Archetype validation dispatch | Partial | `src/bolt_v3_archetypes/mod.rs` dispatches through `ArchetypeValidationBinding` | `tests/config_parsing.rs` | Strategy runtime construction is not implemented. |
| 13 | Market-family validation dispatch | Partial | `src/bolt_v3_market_families/mod.rs` dispatches through `MarketFamilyValidationBinding` | `tests/bolt_v3_market_identity.rs` | Only validation/planning is covered; live readiness is not. |
| 14 | Provider-specific adapter mapping behind provider modules | Partial | `src/bolt_v3_adapters.rs` imports NT Polymarket/Binance configs and `MarketSlugFilter` directly | `tests/bolt_v3_adapter_mapping.rs`, `tests/bolt_v3_provider_binding.rs` | Working code exists, but architectural placement is still wrong. Per-provider modules are the likely destination boundary; adapter mapping has not moved there. Extending to a new provider requires adding a variant to the closed `BoltV3VenueAdapterConfig` enum. |
| 15 | Provider-specific secret handling behind provider modules | Partial | `src/bolt_v3_secrets.rs` owns provider blocklists and resolved-secret enum variants; provider modules own some secret-path validation | Unit tests in `src/bolt_v3_secrets.rs` | Working code exists, but architectural placement is still wrong. Move blocklists/resolved-secret projection behind provider-owned binding modules. |
| 16 | Provider-specific client factory registration behind provider modules | Partial | `src/bolt_v3_client_registration.rs` imports concrete Polymarket/Binance factories | `tests/bolt_v3_client_registration.rs` | Working code exists, but architectural placement is still wrong. Move factory registration behind provider-owned binding modules. Extending to a new provider requires adding a variant to the closed `BoltV3RegisteredVenue` enum. |
| 17 | Provider-specific credential log suppression behind provider modules | Partial | `src/bolt_v3_live_node.rs` hardcodes NT credential module paths | `tests/bolt_v3_credential_log_suppression.rs` covers current Polymarket/Binance entries; it would not detect a missing entry for a new provider | Adding a provider requires editing `NT_CREDENTIAL_LOG_MODULES`; move provider log-suppression metadata behind provider-owned bindings. |
| 18 | LiveNode assembly without running trading | Partial | `src/bolt_v3_live_node.rs` builds `LiveNode` and documents no runner loop | `tests/live_node_run.rs::builds_bolt_v3_livenode_without_running_event_loop`, `tests/bolt_v3_client_registration.rs` | `LiveNodeBuilder::build` is not purely passive: it parses Polymarket private key material into an NT signer and performs internal NT engine/message-bus subscriptions. It opens no network socket and does not run the event loop. |
| 19 | Controlled connect/disconnect boundary | Partial | `src/bolt_v3_live_node.rs` exposes bounded connect/disconnect functions | `tests/bolt_v3_controlled_connect.rs` mock-client tests | Boundary exists and mock tests pass. No real-adapter connect test proves production sockets; this does not prove market/instrument readiness. |
| 20 | Runtime capture off hot path | Partial (shared module) | `src/nt_runtime_capture.rs` is shared runtime-capture code reused by Bolt-v3 | `tests/nt_runtime_capture.rs`, `python3 scripts/verify_runtime_capture_yaml.py` | Capture scope is separate from trading readiness/execution. This is not a new Bolt-v3 architecture module. |
| 21 | Activated-scope broad NT evidence capture | Missing | `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` requires capture of NT data, execution, order, position, account, report, and lifecycle streams for activated scope | No activated-scope end-to-end evidence test found | Required before canary so local evidence can reconstruct live behavior. |
| 22 | Local evidence / decision-event catalog round-trip | Missing | `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` requires registered decision-event custom-data round trip and local catalog readiness | No Bolt-v3 decision-event catalog readiness test found | Must be proven before order-submission events can gate submit. |
| 23 | Pure market identity | Partial | `src/bolt_v3_market_identity.rs`, `src/bolt_v3_market_families/updown.rs` | `tests/bolt_v3_market_identity.rs` | Current family implementation is one family binding, not generic live selection. |
| 24 | Provider discovery/filter binding | Partial | `src/bolt_v3_adapters.rs` installs provider filter logic for current family/provider path | `tests/bolt_v3_provider_binding.rs` | Still located in adapter mapper; provider ownership boundary is unresolved. |
| 25 | NT instrument request/readiness | Missing | Runtime contract requires instrument/cache readiness through NT state | No Bolt-v3 readiness verifier/test found | Define readiness from NT cache/provider state before live market selection. |
| 26 | Selected live market target stack | Missing | Runtime contract defines `selected_market`, `updown_selected_market_facts`, `market_selection_result`, and `updown_market_mechanical_result` | No accepted Bolt-v3 target-stack implementation/test found | Required before strategy edge, sizing, or order work. |
| 27 | `event_page_slug` mapping and Gamma `priceToBeat` readiness | Missing blocker | Runtime contract says the mapping rule is unset and live order readiness is blocked | No mapping/readiness implementation found | Must be resolved before current updown live orders. |
| 28 | Reference data through NT subscriptions | Missing for Bolt-v3 | Runtime contract says no bolt-owned reference actor; strategies subscribe through NT data clients | Old `tests/reference_actor.rs` / `tests/reference_pipeline.rs` are not Bolt-v3 completion evidence | Decide how reference providers bind into Bolt-v3 after market identity/readiness. Pre-v3 `src/platform/audit.rs` and `src/platform/reference.rs` still hold legacy venue/provider logic; disposition is out of scope for this map. |
| 29 | Strategy instance scaling from TOML | Partial parser support only | `src/bolt_v3_config.rs` supports `strategy_files: Vec<String>` and duplicate checks | `tests/config_parsing.rs` | Loading many files is not the same as constructing/registering many NT strategies. |
| 30 | Concrete NT strategy construction | Missing | No Bolt-v3 archetype builder that returns/registers concrete NT `Strategy` found | No Bolt-v3 strategy-construction test found | Need archetype-owned construction boundary. |
| 31 | Strategy data subscriptions through NT | Missing | No Bolt-v3 strategy runtime subscription path accepted | No v3 strategy subscription tests found | Must happen after strategy construction and readiness. |
| 32 | Decision math and structured decision events | Missing for Bolt-v3 | Runtime contract defines decision-event shapes; current archetype validation is not runtime decision logic | No Bolt-v3 decision-event/local-evidence tests found | Implement as archetype-owned, testable logic after data/readiness surfaces are stable. |
| 33 | Risk and sizing policy | Missing / validation only | `src/bolt_v3_validate.rs` validates risk decimal syntax; `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` compares archetype notional fields | Happy-path notional comparison is exercised indirectly through config-parsing fixtures; no over-cap rejection test exercises the error branch | Config-time notional comparison exists. No NT-wired runtime risk engine; need NT/Bolt split decision for runtime risk enforcement versus config-time validation. |
| 34 | Order/fill lifecycle handling | Missing | Runtime contract assigns order/fill lifecycle truth to NT order/execution events | No Bolt-v3 order/fill lifecycle tests found | Required before live order submission. |
| 35 | Position and balance reconciliation | Missing | Runtime contract assigns account, position, balance, average-price, and exposure truth to NT Portfolio/cache/venue-confirmed state | No Bolt-v3 reconciliation tests found | Required before sizing and exit decisions can be trusted. |
| 36 | Post-restart reconciliation | Missing | Runtime contract requires recovery authority from NT state reconciliation and venue-confirmed state | No Bolt-v3 restart-reconciliation tests found | Required before canary/live restart safety. |
| 37 | Per-market rate/cooldown limits | Missing | Runtime contract defines retry/block timing concepts but no accepted runtime implementation was found | No Bolt-v3 per-market cooldown/rate-cap tests found | Needed before live retry loops can run safely. |
| 38 | Live-path observability / metrics | Missing | Runtime contract says NT-native observability first plus minimal structured decision events | No Bolt-v3 live-path observability tests found | Required for canary diagnosis and operator safety. |
| 39 | Order construction using NT-native IDs/types | Missing | No Bolt-v3 order construction path accepted | No Bolt-v3 order construction tests found | Must remain no-submit until explicit execution gate. |
| 40 | Dry-run / no-trade audit mode | Missing | Runtime capture exists, but not strategy dry-run audit | No dry-run strategy tests found | Define before execution gate. |
| 41 | Execution gate / kill switch | Missing | No Bolt-v3 execution gate accepted | No execution-gate tests found | Required before any live order submission. |
| 42 | Paper/shadow mode on live data | Missing | No accepted Bolt-v3 live data shadow runner found | No shadow-mode tests found | Comes after readiness, strategy construction, and dry-run audit. |
| 43 | CI compile/test gating | Partial | `.github/workflows/ci.yml` runs `just fmt-check`, `just deny`, `just clippy`, `just test`, and conditional `just build`; `justfile` defines those recipes | Existing checks do not exercise live trading, readiness, or production sockets | Track what CI proves separately from roadmap completeness. |
| 44 | Release identity and deploy trust | Missing | Contract ledger marks release identity/deploy trust as accepted contract requiring evidence | No deploy-trust evidence found in this status pass | Required before canary or production. |
| 45 | Panic gate and service policy | Missing | Contract ledger marks panic gate/systemd policy as requiring issue evidence | No panic-gate evidence found in this status pass | Required before live capital. |
| 46 | Polymarket CLOB V2 readiness gate | Partial; live gate still blocked | Contract ledger marks CLOB V2 readiness as blocker; this branch pins NT to audited candidate `56a438216442f079edf322a39cdc0d9e655ba6d8`, which contains upstream Polymarket CLOB V2 migration support | Pin-change audit: `docs/bolt-v3/research/nt-pin-change/2026-04-28-clob-v2-pin-audit.md`; compatibility probe: `docs/bolt-v3/research/nt-pin-change/2026-04-28-clob-v2-pin-probe.md`; focused compile/tests passed in the probe | Upstream support blocker is reduced, not closed. Bolt still needs live CLOB V2 signing/order/fill/fee validation, runtime-contract updates, dependency review, and explicit production approval before this gate can close. |
| 47 | Tiny live canary trade | Missing | No live trade path accepted | No live trade verification found | Only after readiness, reconciliation, audit/local-evidence, execution-gate, deploy-trust, panic-gate, provider-signing, and explicit user approval. |
| 48 | Production live trading | Missing | No production deployment path accepted for Bolt-v3 | No production verification found | Not in scope until canary/reconciliation/audit are proven. |

## What This Means

The project is not at a numbered completion point like "85 of 100." The source
evidence says Bolt-v3 has useful foundation pieces, but the accepted path to live
trading is still before strategy construction, readiness, risk/sizing, and
execution.

The most accurate status is:

- Source/test evidence exists for TOML parsing, SSM-only direction, NT adapter
  config mapping, NT client registration, controlled connect, runtime capture,
  and pure market identity.
- Architecture-clean provider neutrality is incomplete.
- The next implementation work should continue moving narrow verifier or boundary
  slices forward, not a new trading feature.

## Immediate Review Questions

Reviewers should not score this document on optimism. They should check:

1. Does every "implemented" row have source evidence and test/verifier evidence?
2. Does any "partial" row overclaim completion?
3. Does any "missing" row actually have accepted Bolt-v3 code/tests in the repo?
4. Are provider/product/archetype-specific current paths named as residuals rather
   than hidden?
5. Is the recommended next move narrow enough to avoid another broad rewrite?
