# T043A Data-Client Production Readiness

Status: open. The PR-enabled data-client adapters are not yet proven production-usable.

## Current Evidence

The current evidence is initial adapter binding and metadata smoke only:

- `cargo test requested_market_data_clients_map_as_data_only_and_polymarket_remains_data_execution`: passed.
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

This proves basic config parsing, adapter mapping, data-only boundary checks, and one-time public metadata reachability. It does not prove production readiness.

## Implementation Progress

The first T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-readiness-source --config <root.toml> --output <data-client-readiness-source.json>`.
- The collector loads the root TOML and provider registry, derives a market-identity plan, and writes a bounded JSON artifact with:
  - `record_kind = "bolt_v3.data_client_readiness_source.v1"`.
  - `config_bundle_checksum`.
  - per-client `client_key_hash`, `provider_key`, data/execution/secrets capability booleans, data-only scope, strategy-routed flag, supported market families, required secret-block classes, hashed data/execution config, field-name inventories, and hashed routed target ids.
  - classified TOML-owned timeout, retry, freshness, reconnect, and rate-limit policy field names.
  - explicit missing behavior proof rows for metadata behavior, quote/book behavior, freshness/latency, and reconnect/rate-limit/error handling.
- The artifact marks every row `production_usable = false` with status `not_production_usable_metadata_or_config_only`; later T043A slices must add behavior/freshness/reconnect/rate-limit proof before any row can become production-usable.
- A contract test was added for the artifact shape and no-SSM-path leakage, but cargo execution is intentionally deferred to the final verification pass per operator direction.

The second T043A source-owned proof primitive has been added but not yet run through final cargo/CI verification:

- New read-only CLI: `operator-artifacts collect-data-client-nt-source-capability --config <root.toml> --client-key <configured-client> --nt-adapter-source <pinned-nt-source.rs> --max-source-bytes <bytes> --output <data-client-nt-source-capability.json>`.
- The collector binds the client through the loaded TOML/provider registry, reads a bounded pinned NT adapter source file, and writes a JSON artifact with:
  - `record_kind = "bolt_v3.data_client_nt_source_capability.v1"`.
  - `config_bundle_checksum`.
  - hashed client key, provider key, hashed source path, source sha256, source byte length, and source-level capability-marker booleans for metadata, quote, book, and ticker surfaces.
  - explicit unsupported-source dispositions for missing metadata, quote, book, or ticker surfaces.
- The artifact marks the row `production_usable = false` with status `nt_source_capability_only_behavior_probe_missing`; NT source markers are evidence of available upstream surfaces, not evidence that the configured LiveNode data path behaves correctly under production conditions.
- A contract test was added for configured-client binding, source/path hashing, source-surface marker capture, fail-closed unsupported disposition, and non-leakage of raw source paths. Cargo execution remains deferred to the final verification pass per operator direction.

## Missing Production Proof

T043A remains open until a venue-neutral matrix proves the following for every PR-enabled data client, including Polymarket and each data-only NT venue binding:

- The client is selected from TOML/provider registry data, with no venue, asset, market, token, symbol, cadence, endpoint, or product hardcode treated as canonical.
- The Bolt `LiveNode` build path includes the data client through the normal adapter mapping path.
- Data-only clients reject `[execution]`, `[secrets]`, and direct credential fields unless a future explicit SSM-backed provider binding is added.
- NT data behavior is proven beyond metadata-only smoke: quote/book/ticker/subscription behavior is verified where upstream supports it, and unsupported paths have a recorded fail-closed disposition.
- Freshness, latency bound, reconnect, rate-limit, and parse/error behavior are verified under configured values.
- The matrix records which markets/product types each client can actually cover, without implying a global Binance, BTC, 5-minute, or Polymarket-only default.
- Focused tests and source-fence/hardcode checks pass after the matrix implementation.

## Current Matrix

| Client/provider | Current source-owned proof | Production gaps | Current disposition |
| --- | --- | --- | --- |
| Polymarket | Provider binding supports data and execution; the T043 no-submit run built the LiveNode, connected, reconciled account state, observed zero orders/fills/positions, and disconnected cleanly. | T044 still has no successful tiny-capital submit artifact. Multi-venue data-client readiness is not implied by the Polymarket canary path. | Usable only for the already-scoped Polymarket T043/T044 path after renewed operator approval. |
| Binance | Existing credentialed data client adapter maps through the provider registry; live config currently does not declare a Binance client. | It is not configured in the current live root TOML, and no current LiveNode data-path proof covers Binance for production trading inputs. | Not production-usable for this PR without a T043A matrix row proving configured data behavior. |
| Bybit | Thin data-only binding exists in `src/bolt_v3_providers/market_data.rs`; it rejects `[execution]`, `[secrets]`, and direct credential fields; one-time NT public HTTP metadata smoke fetched instruments/products. | No LiveNode data-path proof, no quote/book/ticker/subscription proof, no freshness/reconnect/rate-limit/error proof, and no market-coverage matrix. | Open T043A item. |
| Coinbase | Thin data-only binding exists; one-time NT public HTTP metadata smoke fetched instruments/products. | Same missing production proofs as Bybit. | Open T043A item. |
| Deribit | Thin data-only binding exists; one-time NT public HTTP metadata smoke fetched instruments/products. | Same missing production proofs as Bybit; Deribit/index readiness-provider vocabulary does not prove this NT data-client adapter. | Open T043A item. |
| OKX | Thin data-only binding exists; one-time NT public HTTP metadata smoke fetched instruments/products. | Same missing production proofs as Bybit. | Open T043A item. |
| Kraken | Thin data-only binding exists; one-time NT public HTTP metadata smoke fetched instruments/products; validation also calls NT Kraken config validation when parsing succeeds. | Same missing production proofs as Bybit. | Open T043A item. |

## Implementation Checklist

T043A should be closed by a source-owned proof path, not by prose or a transient smoke script:

- Add a read-only operator-artifact collector or equivalent checked artifact that enumerates configured data clients from the loaded root TOML and provider registry.
- For each configured client, record provider key, client key hash, data/execution/secrets capability classification, configured product/market coverage summary, and whether the client is strategy-routed or reference-data-only.
- Prove the normal `build_bolt_v3_live_node` or no-submit LiveNode build path includes each configured data client through `map_bolt_v3_adapters`; do not instantiate a second raw-adapter path as production evidence.
- For public market-data behavior, collect bounded evidence through the pinned NT client surface for supported metadata and quote/book/ticker/subscription behavior; for unsupported surfaces, write an explicit fail-closed unsupported-path disposition.
- Record freshness, timeout, retry, reconnect, rate-limit, and parse/error behavior from TOML-owned config and observed bounded read-only probes.
- Keep data-only clients non-credentialed in this PR unless a future explicit SSM-backed secret binding is added; do not use environment variables or direct credential fields.
- Add hardcode/source-fence checks that prevent BTC, Binance, Polymarket, 5-minute cadence, or any single venue/product from becoming a canonical default for the matrix.
- Run the focused tests and final source-fence/CI only at the final verification pass, per the current operator direction to defer cargo/CI churn.

## Boundary

T043A is a production-readiness gate for the data-client adapter additions. It is separate from the T044 Polymarket tiny-capital canary, but it must complete before PR #480 claims multi-venue data-client production usability or final production-readiness closeout.
