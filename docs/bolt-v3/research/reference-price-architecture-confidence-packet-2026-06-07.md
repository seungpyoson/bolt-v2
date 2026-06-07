# Reference-Price Architecture Confidence Packet - 2026-06-07

Branch: `codex/reference-price-architecture`
Base: `origin/main` at `2c1c4fdbbb0f920c8aa2c052576b7c82a303d38a`

This packet records the pre-code gate for the reference-price architecture prompt.
It is evidence only; it does not authorize live verification. Live verification
remains gated on a green implementation PR head plus explicit operator approval.

## Baseline Checks

- `cargo test --locked --test config_parsing -- --nocapture`: 191 passed.
- `cargo test --locked --test nt_custom_data_catalog_integration -- --nocapture`: 1 passed.

## Implementation Verification Addendum

Current implementation state after the local audit:

- `[reference_current_price]` is strategy-scoped. Prompt defaults for omitted `min_valid_sources`, `enabled`, and `required` are applied through a custom TOML wire-deserializer; no `#[serde(default)]` legacy defaults are used.
- Provider credentials remain root/client scoped and SSM-only. Chainlink reference and PolyResearch reference providers are registered as data-only provider bindings.
- `ReferencePriceUpdate` uses deterministic NT custom data, not `IndexPriceUpdate`.
- Strategy handling keeps Chainlink resolution strike isolated to `price_to_beat`; selected reference-current-price custom data updates strategy current-price state, source health, and the trading spot input.
- Provider keys are metadata-owned provider strings, not concrete core enum variants, so the provider-leak fence does not expose provider-specific literals through core production code.
- Source-fence was updated for the new reference-price schema, provider keys, status codes, and protocol constants.

Verification run after implementation:

- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `cargo test --locked --all-targets`: passed after re-deriving the source-integrity digest for the final strategy source set.
- `cargo test --locked --test bolt_v3_decision_evidence -- --nocapture`: 11 passed.
- `cargo test --locked strategies::binary_oracle_edge_taker::tests::pricing -- --nocapture`: 38 selected strategy pricing tests passed.
- `cargo test --locked strategies::binary_oracle_edge_taker::tests::reference_price -- --nocapture`: 6 selected strategy reference-price tests passed.
- `cargo test --locked --test bolt_v3_reference_price --test bolt_v3_reference_price_config --test bolt_v3_reference_price_runtime --test bolt_v3_reference_provider_registration -- --nocapture`: 2 + 13 + 11 + 2 passed.
- `cargo test --locked --test bolt_v3_strategy_registration --test bolt_v3_polyresearch_auth --test bolt_v3_chainlink_registration --test config_parsing -- --nocapture`: 30 + 2 + 1 + 186 passed.
- `cargo test --locked bolt_v3_source_integrity::tests -- --nocapture`: 10 selected source-integrity tests passed after the digest update.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `just source-fence`: passed.

Source-proof interpretation:

- Retired gate/readiness/canary search returned no matches in the requested production/docs surface.
- Scoped retired quote-reference block search returned no matches in active runtime/config/current PR surfaces.
- `price_to_beat` search returned no matches in `src/bolt_v3_providers/polyresearch.rs`, `src/bolt_v3_providers/chainlink_reference.rs`, `src/bolt_v3_reference_price.rs`, or `tests/bolt_v3_reference_price.rs`.
- Focused stale-name/source-selection search returns only unrelated first-initializer logger text, not reference-price selection code.
- `apiKey` appears only in PolyResearch protocol query handling; `/bolt/polyresearch/api-key` appears only as an SSM parameter-name fixture and is intentionally not a testnet path. Secret-source search hits otherwise remain existing guard/tests/protocol strings, not new fallback paths.

TDD ledger for the final review cleanup:

- RED: added an integration assertion that strategy input evidence serializes `reference_current_price` and not the retired field name; `cargo test --locked --test bolt_v3_decision_evidence latest_entry_decision_evidence_chain_binds_snapshot_order_intent_and_admission -- --nocapture` failed with `left: Null` for `snapshot["reference_current_price"]`.
- GREEN: renamed the evidence field and strategy/test references to `reference_current_price`; the focused evidence test, full `bolt_v3_decision_evidence` integration suite, strategy pricing tests, strategy reference-price tests, source-integrity tests, and all-targets gate passed.

Remaining gates:

- No live verification has been run. It remains blocked until the exact implementation PR head is green and the operator explicitly approves the live check on a reconfirmed target host.
- The PR remains implementation-only until that approval; live checks must restore the target service state, logging overrides, and debug overrides after the short run.

## Required Evidence

| # | Required proof | Evidence and implementation consequence |
|---|---|---|
| 1 | Current config parse path for strategy config and root/shared provider config | Root config owns `clients` and `gate_providers`; strategy config owns `signal_data`, `resolution_data`, and `reference_current_price`. `ClientBlock` already separates `data`, `execution`, and `secrets`. `load_bolt_v3_config` reads the root TOML, strategy files, then validates root and strategies. Provider endpoints and SSM paths stay in root client blocks. |
| 2 | Current provider registration path | Live-node assembly resolves SSM secrets, maps adapters, registers data/execution clients, then registers strategies (`src/bolt_v3_live_node.rs:1807-1887`). Provider bindings are discovered through `binding_for_provider_key` and `PROVIDER_BINDINGS` (`src/bolt_v3_providers/mod.rs:507-718`). Client registration calls `LiveNodeBuilder.add_data_client` and/or `add_exec_client` for mapped clients (`src/bolt_v3_client_registration.rs:90-135`). Reference-price providers must enter through the same provider-binding path. |
| 3 | Current strategy subscription/update path | `on_start` subscribes reference-current-price custom data and signal quotes; `on_stop` unsubscribes them. Signal quotes still use `subscribe_quotes`; resolution strike uses `subscribe_index_prices` with `window_open_unix_seconds` params. `on_quote` routes only signal quote ticks; `on_data` routes `ReferencePriceUpdate` custom data to reference-current-price selection and trading spot state. |
| 4 | Current `price_to_beat` path | The Chainlink strike provider is documented as a point-in-time REST source that emits one `IndexPriceUpdate` for `price_to_beat`, not a continuous reference stream (`src/bolt_v3_providers/chainlink/strike_source.rs:1-20`). The source fetches a signed REST report and sends `Data::IndexPriceUpdate` (`src/bolt_v3_providers/chainlink/strike_source.rs:312-334`). Strategy `on_index_price` binds the update to `price_to_beat` only through `observe_resolution_strike` (`src/strategies/binary_oracle_edge_taker/mod.rs:744-782`, `src/strategies/binary_oracle_edge_taker/mod.rs:4148-4168`). New reference price must not update `price_to_beat`, emit `IndexPriceUpdate`, or reuse the Chainlink REST strike source. |
| 5 | Current PRR auth helper behavior and SSM-only credential path | Current PRR helper keeps endpoint and credential separate, rejects an endpoint already containing `apiKey`, and appends `apiKey` once at provider edge (`src/bolt_v3_providers/polyresearch.rs:1-28`, `tests/bolt_v3_polyresearch_auth.rs:6-36`). Secret resolution uses `SsmResolverSession` and `resolve_field`, and the production resolver calls AWS SDK SSM `get_parameter().with_decryption(true)` (`src/bolt_v3_secrets.rs:189-333`, `src/secrets.rs:126-161`). Implementation must not add env-var or CLI secret fallbacks. |
| 6 | Current Nautilus custom-data API surface | Pinned Nautilus exposes `Data::Custom`, `DataType::new`, and `CustomData::new` (`crates/model/src/data/mod.rs:97-108`, `crates/model/src/data/mod.rs:554-565`, `crates/model/src/data/custom.rs:390-412` in the pinned checkout). Actors can `subscribe_data` with a `DataType` and optional `ClientId`, and can `publish_data` on the custom-data topic (`crates/common/src/actor/data_actor.rs:1025-1038`, `crates/common/src/actor/data_actor.rs:3049-3054`, `crates/common/src/actor/data_actor.rs:3130-3159`). Data clients forward `SubscribeCustomData` (`crates/data/src/client.rs:252-259`). Repo test `nt_custom_data_catalog_integration` round-trips custom data through the local catalog. |
| 7 | Current source-fence/literal-audit rules needing updates | `just source-fence` runs runtime-literal tests/audit, provider-leak tests/audit, core-boundary, naming, dependency-direction, schema, pure-Rust, legacy-default, strategy-policy, and runtime-capture checks (`justfile:163-196`). `fmt-check` depends on runtime literals and provider leaks (`justfile:101`). Runtime literals scan `src/**/*.rs` and compare against `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` (`scripts/verify_bolt_v3_runtime_literals.py:24-25`, `scripts/verify_bolt_v3_runtime_literals.py:498-540`). Provider leak checks reject provider-key and market-family literals in core production code (`scripts/verify_bolt_v3_provider_leaks.py:61-76`, `scripts/verify_bolt_v3_provider_leaks.py:224-229`). Any new runtime strings must be either in provider-owned modules or audited with rationale. |
| 8 | Removal of the old quote path | The old strategy reference quote config path was removed. Runtime config no longer derives `reference_venue` or `reference_instrument_id`, and actor `on_quote` no longer routes reference quotes. The only reference current-price input is selected `ReferencePriceUpdate` custom data from `[reference_current_price]`. |
| 9 | Exact interval identity available in strategy code and selection reset | `CandidateMarket` carries `start_ts_ms` and `expiration_ts_ms` (`src/strategies/binary_oracle_edge_taker/selection.rs:44-56`). `ActiveMarketState` carries `interval_start_ms`, `interval_end_ms`, `interval_open`, and `price_to_beat`; `from_market` initializes these and resets reference timestamps (`src/strategies/binary_oracle_edge_taker/mod.rs:270-289`, `src/strategies/binary_oracle_edge_taker/mod.rs:585-654`). Selection replacement resets active state unless the boundary is the same (`src/strategies/binary_oracle_edge_taker/selection.rs:88-145`). Reference-price selection can reset on `interval_start_ms` and `interval_end_ms`. |
| 10 | Exact strategy-local state/update method for `reference_current_price` | Current implementation adds explicit reference-current-price state and custom-data handling. Accepted selected quotes update active state, source health, `last_reference_ts_ms`, and `TakerPricingState.fast_spot`; stale, wrong-provider, and out-of-interval frames fail closed. |
| 11 | Exact failure/status propagation path for source health | Current production code has no reference-price source health path. The only `VenueHealth` enum is test-only and contains only `Healthy` (`src/strategies/binary_oracle_edge_taker/mod.rs:220-238`). The test-only pricing path filters stale/unhealthy venue snapshots (`src/strategies/binary_oracle_edge_taker/mod.rs:462-470`, `src/strategies/binary_oracle_edge_taker/mod.rs:4920-4933`). Chainlink strike fetch failures log and leave `price_to_beat` unset, while bad interval strikes warn and leave `price_to_beat` unchanged (`src/bolt_v3_providers/chainlink/strike_source.rs:318-334`, `src/strategies/binary_oracle_edge_taker/mod.rs:744-782`). Implementation must add an explicit source-health custom-data path. |

## Provider API Evidence

- Chainlink official docs list WebSocket testnet/mainnet domains, required auth headers, endpoint `/api/v1/ws`, `feedIDs`, and sample report response (`https://docs.chain.link/data-streams/reference/data-streams-api/interface-ws`, lines 119-156).
- Chainlink official Rust auth example builds `Authorization`, `X-Authorization-Timestamp`, and `X-Authorization-Signature-SHA256` headers, including for WebSocket requests (`https://docs.chain.link/data-streams/reference/data-streams-api/authentication/rust-examples`, lines 210-227 and 425-433).
- Chainlink official v3 report schema exposes `validFromTimestamp`, `observationsTimestamp`, `price`, `bid`, and `ask` (`https://docs.chain.link/data-streams/reference/report-schema-v3`, lines 123-135).
- PolyNode docs show the reference-price style Chainlink subscription type for real-time crypto prices and heartbeat/reconnect behavior (`https://docs.polynode.dev/websocket/overview`, lines 230-254 and 301-320).
- PolyNode feed docs show BTC/USD, ETH/USD, SOL/USD, BNB/USD, XRP/USD, DOGE/USD, and HYPE/USD events with `price`, `bid`, `ask`, and `timestamp`; feed filters use `{"action":"subscribe","type":"chainlink"}` (`https://docs.polynode.dev/crypto/feeds`, lines 56-210).
- PolyNode price-to-beat REST docs show the supported symbols and intervals, but this implementation scope uses the streaming feed for reference price, not REST price-to-beat (`https://docs.polynode.dev/api-reference/crypto/price`, lines 235-304).

## Hard-Stop Assessment

- Interval identity is proven through `interval_start_ms`/`interval_end_ms`.
- Nautilus custom data is proven through pinned dependency APIs and a passing repo test.
- SSM-only credential resolution is proven; no new env-var or CLI path is allowed.
- `reference_current_price` is the single reference current-price trading input; the old quote/fair-value path was removed.
- Retired canary/readiness/submission-disabling gates are out of scope and must not be reintroduced.
- #600 `resolution_data` generalization, #601 external API health, and #603 per-tick source selection are out of scope.
- Current source health is absent, not ambiguous. The architecture must create one explicit path and keep it scoped to reference-price source health.

Result: PASS for starting TDD implementation. Live verification remains blocked until after PR CI is green and the operator explicitly approves it.

## Internal Adversarial Review

1. Could this be solved by extending `resolution_data`?
No. `resolution_data` is the Chainlink strike input for `price_to_beat`; extending it would reuse the wrong data type and violate the prompt's separation requirement.

2. Could the new config be root-only?
No. Selection policy, ordered sources, interval handling, and drift behavior are strategy concerns. Provider endpoints and credentials stay root-scoped, because those share provider lifecycle.

3. Could reference-price updates reuse `IndexPriceUpdate`?
No. `IndexPriceUpdate` is already reserved by the strike path, and `on_index_price` mutates `price_to_beat`. Reference price needs a deterministic custom `DataType` and strategy downcast.

4. Is one-source-per-tick arbitration required?
No. The prompt requires first valid source per interval plus one controlled failover, with no flip-back until the next interval. Per-tick source selection is #603 scope and excluded.

5. Is provider auth sufficiently grounded?
Chainlink WS auth is grounded in official docs and the existing HMAC helper. PRR auth is grounded in current repo helper/tests and must remain endpoint-plus-secret, appended once at provider edge.

6. Does this require a permanent shutdown on source failure?
No. The prompt requires source health/provenance and no permanent shutdown. Failure should mark source health, keep strategy behavior fail-closed while requirements are unmet, and allow later recovery.

7. Does this require live credentials during implementation?
No. Implementation and CI use deterministic unit/integration tests. Live checks wait for a green PR head and explicit operator approval.
