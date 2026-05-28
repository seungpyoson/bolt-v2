# T043A Data-Client Production Readiness

Status: open. The PR-enabled data-client adapters are not yet proven production-usable.

## Current Evidence

The current evidence is initial adapter binding and metadata smoke only:

- `cargo test requested_market_data_clients_map_as_data_only_and_execution_stays_config_owned`: passed.
- `cargo test nt_source_supported_rust_data_client_provider_bindings_are_registered`: passed.
- `cargo test allows_multiple_configured_client_ids_for_same_nt_venue`: passed.
- `cargo test root_example_declares_requested_nt_data_clients_for_registration`: passed.
- `cargo test live_node_registration_can_load_all_requested_data_clients_without_extra_execution_clients`: passed.
- `cargo test flags_set_provider_var_for_configured_data_only_client_without_secrets`: passed.
- `cargo test rejects_market_data_only_provider_execution_secrets_and_direct_credentials`: passed.
- `cargo test --locked data_only`: passed.
- Temporary live metadata smoke through NT public HTTP clients fetched instrument/product metadata at the time of the run:
  - Bybit: 610 instruments/products.
  - Coinbase: 922 instruments/products.
  - Deribit: 18 instruments/products.
  - OKX: 1262 instruments/products.
  - Kraken: 1863 instruments/products.
  - Binance: 1380 instruments/products.

`config/root.example.toml` now declares the requested data-client registration scope: Binance spot/USD-M/COIN-M, BitMEX, Bybit, Coinbase, Deribit, Kraken spot/futures, OKX, and Polymarket. The normal LiveNode registration boundary has been tested for that configured set. This proves config parsing, adapter mapping, provider binding, LiveNode registration, data-only boundary checks, and one-time public metadata reachability. It does not prove production readiness.

After commit `693bf2bd`, the read-only T043A source collectors were run against tracked `config/root.example.toml` into `/private/tmp/bolt-v2-t043a-693bf2bd/`:

- `data-client-readiness-source.json`: sha256 `64788af932efb980296f999cdc9343eea3740b24679de38fa4a06fefd041eab0`.
- `data-client-live-node-mapping-source.json`: sha256 `2a52543fe75032c3c2fa0f07865165f1d553feadbd6d6d07f8b39de6e9cafbeb`.
- NT source-capability artifacts were generated for all 11 configured data clients.
- `data-client-production-readiness-matrix.json`: sha256 `ed173ece80040dd3241c3be069c18aff6a27c969165f6fa86784d4d46d50d5a2`; it records 11 clients, 0 production-usable rows, and `missing_proofs = ["behavior_observation"]` for every row.

This confirms the current architectural blocker is now behavior proof, not registration or provider binding.

## Implementation Progress

The first T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-readiness-source --config <root.toml> --output <data-client-readiness-source.json>`.
- The collector loads the root TOML and provider registry, derives a market-identity plan, and writes a bounded JSON artifact with:
  - `record_kind = "bolt_v3.data_client_readiness_source.v1"`.
  - `config_bundle_checksum`.
  - per-client `client_key_hash`, `provider_key`, data/execution/secrets capability booleans, data-only scope, strategy-routed flag, supported market families, required secret-block classes, hashed data/execution config, field-name inventories, field-level value-kind/item-count/hash fingerprints, hashed routed target ids, and hashed client-owned readiness probe quote targets.
  - classified TOML-owned timeout, retry, freshness, reconnect, and rate-limit policy field names.
  - explicit missing behavior proof rows for metadata behavior, quote/book behavior, freshness/latency, and reconnect/rate-limit/error handling.
- The artifact marks every row `production_usable = false` with status `not_production_usable_metadata_or_config_only`; later T043A slices must add behavior/freshness/reconnect/rate-limit proof before any row can become production-usable.
- A contract test was added for the artifact shape and no-SSM-path leakage, but cargo execution is intentionally deferred to the final verification pass per operator direction.
- The data-only provider binding now rejects TOML `[data]` fields that are not present on the pinned NT config struct, both during startup validation and at the adapter-mapper boundary, so invented fields cannot be treated as readiness policy evidence.
- The readiness source and final matrix now carry only TOML-owned product/market coverage fields such as `product_types`, `instrument_types`, `contract_types`, `instrument_families`, and `load_spreads`; endpoints, credentials, transport values, and timeout values remain hashed/classified rather than printed as market coverage.

The second T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-nt-source-capability --config <root.toml> --client-key <configured-client> --nt-adapter-source <pinned-nt-source.rs> --max-source-bytes <bytes> --output <data-client-nt-source-capability.json>`.
- The collector binds the client through the loaded TOML/provider registry, reads a bounded pinned NT adapter source file, and writes a JSON artifact with:
  - `record_kind = "bolt_v3.data_client_nt_source_capability.v1"`.
  - `config_bundle_checksum`.
  - hashed client key, provider key, hashed source path, source sha256, source byte length, and source-level capability-marker booleans for metadata, quote, book, and ticker surfaces.
  - explicit unsupported-source dispositions for missing metadata, quote, book, or ticker surfaces.
- The artifact marks the row `production_usable = false` with status `nt_source_capability_only_behavior_probe_missing`; NT source markers are evidence of available upstream surfaces, not evidence that the configured LiveNode data path behaves correctly under production conditions.
- A contract test was added for configured-client binding, source/path hashing, source-surface marker capture, fail-closed unsupported disposition, and non-leakage of raw source paths. Cargo execution remains deferred to the final verification pass per operator direction.

The third T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-live-node-mapping-source --config <root.toml> --live-node-source <src/bolt_v3_live_node.rs> --adapter-mapping-source <src/bolt_v3_adapters.rs> --provider-registry-source <src/bolt_v3_providers/mod.rs> --max-source-bytes <bytes> --output <data-client-live-node-mapping-source.json>`.
- The collector binds configured clients through the loaded TOML/provider registry, hashes Bolt's LiveNode, adapter mapping, and provider-registry source files, and records source markers showing that the normal build path calls adapter mapping and dispatches provider bindings across loaded clients.
- The collector also builds the no-submit `LiveNode` path and records the registration summary for each configured client, so matrix `live_node_mapping` proof is not satisfied by source-text markers alone.
- Per-client rows record hashed client key, provider key, data/execution block presence, provider-binding registration, whether data/execution blocks flow through the normal mapping source path, and whether the client was actually registered through the `LiveNode` registration boundary.
- The artifact marks every row `production_usable = false` with status `live_node_mapping_source_only_behavior_probe_missing`; source-path proof is necessary for architecture evidence but still does not prove live data behavior, freshness, reconnect, rate-limit, or parse/error handling.
- A contract test was added for configured-client binding, source/path hashing, source-marker capture, fail-closed unsupported disposition, and non-leakage of raw source paths. Cargo execution remains deferred to the final verification pass per operator direction.

The fourth T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-behavior-observation --config <root.toml> --client-key <configured-client> --behavior-source <data-client-behavior-observation-source.json> --max-behavior-source-bytes <bytes> --output <data-client-behavior-observation.json>`.
- New read-only source materializer: `operator-artifacts collect-data-client-behavior-observation-source --config <root.toml> --client-key <configured-client> --probe-events <probe-events.jsonl> --max-probe-events-bytes <bytes> --output <data-client-behavior-observation-source.json>`.
- The source materializer consumes bounded `bolt_v3.data_client_behavior_probe_event.v1` JSONL, binds every event to the configured client hash/provider key, derives the freshness bound from TOML `[live_canary].reference_quote_max_age_seconds`, aggregates surface samples, derives freshness/latency statistics from the observed events, rejects policy-event kinds that the current source-owned collector cannot produce, and writes the canonical behavior-observation source JSON.
- The collector binds the behavior source to the loaded TOML client via hashed client key and provider key, reads a bounded JSON source file, and validates observed metadata behavior, quote/book/ticker behavior or explicit unsupported dispositions and freshness/latency bounds. Reconnect, rate-limit, and parse/error behavior remain missing proofs until source-owned policy collectors exist.
- The output hashes the source path and source bytes, preserves only the source-owned observation booleans/counts/timestamps/evidence hashes, and records whether the behavior observation is complete.
- The artifact marks `production_usable = false` with status `behavior_observation_final_matrix_missing`; behavior observations are a necessary T043A input, but the final matrix and final verification pass still decide readiness.
- A contract test was added for configured-client binding, behavior validation, complete-observation classification, fail-closed unsupported ticker disposition, source/path hashing, and non-leakage of raw source paths. Cargo execution remains deferred to the final verification pass per operator direction.

The fifth T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-production-readiness-matrix --config <root.toml> --readiness-source <data-client-readiness-source.json> --live-node-mapping-source <data-client-live-node-mapping-source.json> --nt-source-capability <data-client-nt-source-capability.json> --behavior-observation <data-client-behavior-observation.json> --max-source-bytes <bytes> --output <data-client-production-readiness-matrix.json>`.
- The collector binds all input artifacts to the current config bundle, hashes the input artifact files, and writes one row per configured client with config inventory, normal LiveNode mapping, NT source capability, behavior observation, and missing-proof status.
- Matrix rows only mark `production_usable = true` when the configured data client has every required T043A proof present. Missing source artifacts, missing runtime registration summary, or incomplete behavior observations produce explicit `missing_proofs` entries.
- A contract test was added for combining the source artifacts into a per-client matrix row without introducing venue, market, token, symbol, or cadence defaults. Cargo execution remains deferred to the final verification pass per operator direction.

The sixth T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-behavior-probe-events-source --config <root.toml> --client-key <configured-client> --output <probe-events.jsonl>`.
- New root TOML schema under each client: `[clients.<id>.readiness_probe.quote_targets.<target_id>] instrument_id = "<instrument.venue>"`.
- The collector scopes the no-submit `LiveNode` build to the selected configured data client, so unrelated configured clients cannot mask that client's behavior proof. It then runs metadata and configured quote probes for the selected client's `readiness_probe.quote_targets`, using NT actor subscription APIs, and writes bounded `bolt_v3.data_client_behavior_probe_event.v1` JSONL for the selected configured client.
- Strategy `reference_data` is no longer accepted as a fallback data-client behavior probe path; probe targets are client-owned so adding/removing a data-only client and its proof target happens in the same client section.
- The JSONL records hashed client identity, provider key, quote-surface observation, freshness age, latency, and an evidence hash. It does not print raw client ids, instrument ids, prices, paths, or credentials.
- The behavior source materializer now accepts partial source-owned probe sets and records missing reconnect, rate-limit, and parse-error proofs as non-observed policy rows instead of pretending they are proven. Final behavior/matrix artifacts still mark those rows non-production-usable until all required proofs are present.
- This closes the gap that probe events were previously an external input, but it does not close T043A: the current source-owned collector covers configured quote evidence only.

The architecture plan was challenged with Claude adversarial review on 2026-05-29, job `326dba0f-91b4-47e3-b4bb-a76d40606815`. The useful findings were that behavior policy proofs could be represented by operator-authored JSONL and that live-node mapping could be satisfied by source-text markers alone. The current local hardening rejects unowned policy probe events, keeps reconnect/rate-limit/parse-error proofs missing until source-owned collectors exist, carries the `LiveNode` registration summary on the runtime, and requires runtime data-client registration in the matrix mapping proof. The review slot itself was not counted as a clean external approval because the plugin marked it `review_quality_failed:not_reviewed`.

After commit `b360e3ba`, the hardened collector was run against ignored operational `config/live.local.toml` into `/private/tmp/bolt-v2-t043a-b360e3ba/`:

- `data-client-readiness-source.json`: sha256 `a8b88446b7c771fefd781fd91d83e344b33c9d857e5977640b2addd5303a2526`.
- `data-client-live-node-mapping-source.json`: sha256 `73111cbb33e6d7bd44d9252fa2ff2cf6c30bcc5e220986c3688fa2e09c17c193`; it records 11 clients, 11 runtime-registered data rows, 11 mapping rows, and no unsupported mapping dispositions.
- `probe-events-bybit.jsonl`: sha256 `110f2d05e564faeb40a9fa616edb92f7e61f9ce843d3f6affb9d0e4cf8a06889`; the scoped no-submit `LiveNode` registered and connected only `bybit_data` and produced one metadata event.
- `data-client-behavior-observation-source-bybit.json`: sha256 `f7044f7d7b7eb199f504e29ab2de711a47a42654b374ac976b3702f601099307`.
- `data-client-behavior-observation-bybit.json`: sha256 `b225c8295f0441f76576a88eaac8fc5a1ec1d741455ac81e6ea759b111bea0d3`; it remains incomplete with missing `quote_or_book_or_ticker_behavior`, `reconnect_behavior`, `rate_limit_behavior`, and `parse_error_behavior`.

This proves the per-client isolation fix removes the earlier all-client startup coupling for Bybit metadata behavior. It does not close T043A because the live config has no `bybit_data.readiness_probe.quote_targets` yet and no source-owned policy behavior collector exists.

## Missing Production Proof

T043A remains open until a venue-neutral matrix proves the following for every PR-enabled data client, including Polymarket and each data-only NT venue binding:

- The client is selected from TOML/provider registry data, with no venue, asset, market, token, symbol, cadence, endpoint, or product hardcode treated as canonical.
- The Bolt `LiveNode` build path includes the data client through the normal adapter mapping path.
- Data-only clients reject `[execution]`, `[secrets]`, and direct credential fields unless the provider has an explicit SSM-backed credential binding. Binance is credentialed through its provider-owned SSM binding; the other added exchange data-only bindings remain non-credentialed.
- NT data behavior is proven beyond metadata-only smoke: quote/book/ticker/subscription behavior is verified where upstream supports it, and unsupported paths have a recorded fail-closed disposition. The current source-owned probe-event collector proves only client-owned configured quote observations.
- Freshness, latency bound, reconnect, rate-limit, and parse/error behavior are verified under configured values. Current partial probe sources intentionally mark missing policy/error proofs as missing, not production-usable.
- The matrix records which markets/product types each client can actually cover, without implying a global Binance, BTC, 5-minute, or Polymarket-only default.
- Focused tests and source-fence/hardcode checks pass after the matrix implementation.

## Current Matrix

| Client/provider | Current source-owned proof | Production gaps | Current disposition |
| --- | --- | --- | --- |
| Polymarket | Provider binding supports data and execution; the T043 no-submit run built the LiveNode, connected, reconciled account state, observed zero orders/fills/positions, and disconnected cleanly. | T044 still has no successful tiny-capital submit artifact. Multi-venue data-client readiness is not implied by the Polymarket canary path. | Usable only for the already-scoped Polymarket T043/T044 path after renewed operator approval. |
| Binance | Credentialed provider binding maps through the registry and `config/root.example.toml` declares separate `binance_spot_data`, `binance_usdm_data`, and `binance_coinm_data` clients because pinned NT's Binance factory constructs one data client from one product type. LiveNode registration covers all three. | Current configured coverage is spot, USD-M, and COIN-M only. Pinned NT's Binance data factory rejects margin/options; no current LiveNode data-path proof covers Binance quotes/books/tickers/freshness/reconnect/rate-limit behavior for production trading inputs. | Open T043A item. Configured and registered, not production-usable yet. |
| BitMEX | Thin data-only binding exists and `config/root.example.toml` declares `bitmex_data`; LiveNode registration covers it. | No quote/book/ticker/subscription proof, no freshness/reconnect/rate-limit/error proof, and no production matrix completion. | Open T043A item. Configured and registered, not production-usable yet. |
| Bybit | Thin data-only binding exists and `config/root.example.toml` declares spot, linear, inverse, and option product types; it rejects `[execution]`, `[secrets]`, and direct credential fields; one-time NT public HTTP metadata smoke fetched instruments/products; LiveNode registration covers it. | No quote/book/ticker/subscription proof, no freshness/reconnect/rate-limit/error proof, and no production matrix completion. | Open T043A item. Configured and registered, not production-usable yet. |
| Coinbase | Thin data-only binding exists; `config/root.example.toml` declares it; one-time NT public HTTP metadata smoke fetched instruments/products; LiveNode registration covers it. | Same missing production proofs as Bybit. | Open T043A item. Configured and registered, not production-usable yet. |
| Deribit | Thin data-only binding exists; `config/root.example.toml` declares future, option, spot, future-combo, and option-combo product types; one-time NT public HTTP metadata smoke fetched instruments/products; LiveNode registration covers it. | Same missing production proofs as Bybit; Deribit/index readiness-provider vocabulary does not prove this NT data-client adapter. | Open T043A item. Configured and registered, not production-usable yet. |
| OKX | Thin data-only binding exists; `config/root.example.toml` declares SPOT, MARGIN, SWAP, FUTURES, EVENTS, linear/inverse contract types, and spreads; one-time NT public HTTP metadata smoke fetched instruments/products; LiveNode registration covers it. | Same missing production proofs as Bybit. OKX OPTION is not configured because pinned NT requires explicit `instrument_families`; no hardcoded BTC/ETH option families were added. | Open T043A item. Configured and registered, not production-usable yet. |
| Kraken | Thin data-only binding exists; `config/root.example.toml` declares separate spot and futures clients because pinned NT's Kraken factory selects one product type; one-time NT public HTTP metadata smoke fetched instruments/products; validation also calls NT Kraken config validation when parsing succeeds; LiveNode registration covers both. | Same missing production proofs as Bybit. | Open T043A item. Configured and registered, not production-usable yet. |

## Implementation Checklist

T043A should be closed by a source-owned proof path, not by prose or a transient smoke script:

- Add a read-only operator-artifact collector or equivalent checked artifact that enumerates configured data clients from the loaded root TOML and provider registry.
- For each configured client, record provider key, client key hash, data/execution/secrets capability classification, configured product/market coverage summary, and whether the client is strategy-routed or has client-owned readiness probe targets.
- Prove the normal `build_bolt_v3_live_node` or no-submit LiveNode build path includes each configured data client through `map_bolt_v3_adapters`; do not instantiate a second raw-adapter path as production evidence.
- For public market-data behavior, collect bounded evidence through the pinned NT client surface for supported metadata and quote/book/ticker/subscription behavior; for unsupported surfaces, write an explicit fail-closed unsupported-path disposition.
- Record freshness, timeout, retry, reconnect, rate-limit, and parse/error behavior from TOML-owned config and observed bounded read-only probes.
- Keep uncredentialed data-only clients free of `[secrets]` and direct credential fields. Credentialed data providers must use explicit provider-owned SSM bindings only; do not use environment variables, 1Password, or direct credential fields.
- Add hardcode/source-fence checks that prevent BTC, Binance, Polymarket, 5-minute cadence, or any single venue/product from becoming a canonical default for the matrix.
- Run the focused tests and final source-fence/CI only at the final verification pass, per the current operator direction to defer cargo/CI churn.

## Boundary

T043A is a production-readiness gate for the data-client adapter additions. It is separate from the T044 Polymarket tiny-capital canary, but it must complete before PR #480 claims multi-venue data-client production usability or final production-readiness closeout.
