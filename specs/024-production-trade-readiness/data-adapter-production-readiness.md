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

## Missing Production Proof

T043A remains open until a venue-neutral matrix proves the following for every PR-enabled data client, including Polymarket and each data-only NT venue binding:

- The client is selected from TOML/provider registry data, with no venue, asset, market, token, symbol, cadence, endpoint, or product hardcode treated as canonical.
- The Bolt `LiveNode` build path includes the data client through the normal adapter mapping path.
- Data-only clients reject `[execution]`, `[secrets]`, and direct credential fields unless a future explicit SSM-backed provider binding is added.
- NT data behavior is proven beyond metadata-only smoke: quote/book/ticker/subscription behavior is verified where upstream supports it, and unsupported paths have a recorded fail-closed disposition.
- Freshness, latency bound, reconnect, rate-limit, and parse/error behavior are verified under configured values.
- The matrix records which markets/product types each client can actually cover, without implying a global Binance, BTC, 5-minute, or Polymarket-only default.
- Focused tests and source-fence/hardcode checks pass after the matrix implementation.

## Boundary

T043A is a production-readiness gate for the data-client adapter additions. It is separate from the T044 Polymarket tiny-capital canary, but it must complete before PR #480 claims multi-venue data-client production usability or final production-readiness closeout.
