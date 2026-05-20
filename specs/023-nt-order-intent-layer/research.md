# Research: NT Order Intent Layer

## Current Anchors

- Worktree: `/Users/spson/Projects/Claude/bolt-v2/.worktrees/maker-order-replay`
- Branch: `codex/maker-order-proof-clean`
- Head at research start: `5978dc1cde84210f1a293d0b5a667aaa577945b3`
- Pinned NT checkout inspected: `/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb`
- no-mistakes: daemon running, but active run was for `refactor/386-bolt-v3-nt-vocab-alignment`, not this branch

## NT Core Evidence

- NT `OrderType` includes Market, Limit, StopMarket, StopLimit, MarketToLimit, MarketIfTouched, LimitIfTouched, TrailingStopMarket, and TrailingStopLimit at `crates/model/src/enums.rs:1542`.
- NT `TimeInForce` includes GTC, IOC, FOK, GTD, Day, AtTheOpen, and AtTheClose at `crates/model/src/enums.rs:1877`.
- NT `OrderAny` variants include limit, market, stop, touched, trailing, and market-to-limit orders at `crates/model/src/orders/any.rs:31`.
- NT `Order` trait exposes side, type, quantity, TIF, expire, price, trigger, post-only, reduce-only, quote-quantity, display quantity, offsets, emulation trigger, exec algorithm, and tags at `crates/model/src/orders/mod.rs:281`.
- NT `OrderInitialized` stores the order submit payload, including order side/type/TIF/post-only/reduce-only/quote-quantity/price/trigger/expire/display/emulation fields at `crates/model/src/events/order/initialized.rs:55`.
- NT `OrderInitialized::from(order)` copies the order trait fields into the initialized event at `crates/model/src/orders/mod.rs:530`.
- NT `SubmitOrder::from_order` carries `OrderInitialized`, `client_id`, optional `position_id`, and command metadata at `crates/common/src/messages/execution/submit.rs:76`.
- NT `Strategy::submit_order` accepts `OrderAny`, optional `position_id`, optional `client_id`, and optional params, then routes emulator/algo/risk at `crates/trading/src/strategy/mod.rs:109`.
- NT risk rejects expired GTD orders at `crates/risk/src/engine/mod.rs:743`.
- NT execution resolves the execution client, checks order venue versus client venue, checks OMS/position compatibility, checks instrument presence, and calls `ExecutionClient::submit_order` at `crates/execution/src/engine/mod.rs:1733`.

## NT Factory Evidence

- `OrderFactory::market` preserves side, quantity, TIF, reduce-only, quote quantity, exec algorithm params, tags, and optional client order id at `crates/common/src/factories/order.rs:119`.
- `OrderFactory::limit` preserves price, TIF, expire time, post-only, reduce-only, quote quantity, display quantity, emulation trigger, trigger instrument, exec params, tags, and optional client order id at `crates/common/src/factories/order.rs:164`.
- `OrderFactory` exposes stop-market at `crates/common/src/factories/order.rs:221`, stop-limit at `:278`, market-if-touched at `:339`, limit-if-touched at `:394`, and trailing-stop-market at `:459`.
- `OrderFactory` does not expose direct single-order `market_to_limit` or `trailing_stop_limit` methods in this pinned checkout. Direct constructors exist in NT model code, but using them would duplicate factory responsibilities for IDs, timestamps, exec spawn ids, and defaults.

## NT Model Invariant Evidence

- `check_time_in_force` rejects GTD without `expire_time` at `crates/model/src/orders/mod.rs:190`.
- `MarketOrder::new_checked` rejects GTD market orders at `crates/model/src/orders/market.rs:89`.
- `LimitOrder::new` wraps `new_checked` and panics on invalid model input at `crates/model/src/orders/limit.rs:183`.
- `MarketToLimitOrder::new_checked` also requires GTD expire time if GTD is used at `crates/model/src/orders/market_to_limit.rs:60`.
- `TrailingStopLimitOrder::new_checked` requires price, trigger, offsets, and GTD expiry when relevant at `crates/model/src/orders/trailing_stop_limit.rs:73`.

## Adapter Evidence

- Polymarket limit validation rejects reduce-only, non-limit, quote quantity, missing price, unsupported TIF, invalid side, and post-only outside GTC/GTD at `crates/adapters/polymarket/src/execution/order_builder.rs:164`.
- Polymarket submit maps `expire_time` into expiration and carries `post_only` into the request at `crates/adapters/polymarket/src/execution/submitter.rs:327`.
- Binance Spot maps limit plus post-only to `LIMIT_MAKER` and rejects GTD TIF at `crates/adapters/binance/src/spot/enums.rs:103`.
- Binance Futures derives venue `position_side` from hedge mode, order side, and reduce-only at `crates/adapters/binance/src/futures/execution.rs:400`.
- Binance Futures reads submit params such as `close_position` and `price_match` from `SubmitOrder.params` at `crates/adapters/binance/src/futures/execution.rs:427`.
- Deribit supports limit, market, stop, and touched order mapping, maps GTD to `good_til_day`, and warns custom `expire_time` is ignored at `crates/adapters/deribit/src/execution.rs:169`.
- Deribit product config can include Future, Option, and Spot at `crates/adapters/deribit/src/config.rs:47`.

## Current Bolt Narrowing Evidence

- Current `OrderParams` mixes order fields and `position_side` at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:110`.
- Current archetype validation hardcodes entry tuples to buy/long limit FOK taker or buy/long limit GTC post-only at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:805`.
- Current archetype validation hardcodes exit tuples to sell/long market IOC taker or sell/long limit GTC post-only at `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:844`.
- Current runtime parser only accepts order type `limit|market` and TIF `gtc|fok|ioc` at `src/strategies/binary_oracle_edge_taker.rs:4703`.
- Current order builder calls only `core.order_factory().limit(...)` or `core.order_factory().market(...)` and passes optional NT fields as `None` at `src/strategies/binary_oracle_edge_taker.rs:4727`.
- Current decision evidence records strategy id, intent kind, instrument id, client order id, side, price, and quantity, but not the compiled order type/TIF/post-only/reduce-only/quote/expiry/trigger fields at `src/bolt_v3_decision_evidence.rs:31`.
- Existing mixed maker/taker long-side config coverage is already present in `tests/config_parsing.rs:689`; the next missing public validation behavior is coherent short-side entry/exit.

## Multi-Agent Review Findings To Carry Forward

- Minimalism review: do not use direct NT constructors; use `OrderFactory` only or add/upstream NT factory support.
- Minimalism review: delete the current dual parse path by normalizing order config once.
- Venue/market review: `position_side` is not an NT order field and must remain strategy contract metadata.
- Venue/market review: compile output cannot be just `OrderAny` if submit params, client id, or position id affect execution; use a placement plan.
- End-to-end review: GTD is not globally enabled by setting TIF to GTD; it needs expiry config and adapter-proven semantics.
- End-to-end review: passive maker exit is not a forced-flat guarantee.

## Decisions

1. Bolt will model `StrategyPositionContract`, `NtOrderTemplate`, `OrderBuildInputs`, and optional `SubmitContext` as separate concepts.
2. `NtOrderTemplate` will not contain `position_side`, `client_id`, `position_id`, or submit params.
3. Bolt will use NT `OrderFactory` for construction and will not call direct order model constructors without a separate approved design.
4. Bolt will validate NT model crash-prevention invariants locally before factory calls.
5. Bolt will not own a runtime venue capability matrix.
6. Adapter legality claims require NT adapter source evidence, no-submit smoke, or live/canary proof.
7. First implementation slice will be a TDD vertical slice that removes tuple whitelist narrowing without adding broad venue policy.

## Pre-Implementation Review Findings

Minimalism review:

- `NtOrderTemplate` must not become a Bolt order DSL. Enabled fields must be limited to the slice under test, with factory-reachable variants listed only as future evidence obligations.
- `trigger_price_source` was not NT vocabulary and has been removed from the model.
- Crash-prevention validation must stay limited to enabled variants.
- Submit context and evidence must not duplicate NT `Strategy::submit_order` or NT `OrderInitialized`.

Venue/market review:

- `client_id` must be optional because NT can route by explicit client id, venue routing map, or default client.
- Factory-supported non-limit/market variants need positive construction/admission tests before support can be claimed.
- Adapter proof cannot be hardcoded to Polymarket/Binance/Deribit; the adapter set must follow the claim.
- Concrete submit params belong to provider bindings or strategy config, not the generic order layer.

End-to-end review:

- Submit context must be proven at the real NT submit boundary, not only documented.
- GTD needs a positive expiry path before GTD support is claimed.
- OMS/position and reduce-only behavior need concrete tests before support is claimed.
- Admission must be computed from the compiled order view, not pre-build strings.
- Forced exit needs its own task slice or explicit residual.
- No-submit proof must be a gate for execution claims or an explicit residual.

## TDD Slice 1 Evidence

- T010 baseline green: `cargo test bolt_v3_archetype_accepts_mixed_maker_taker_order_configs -- --nocapture` passed.
- T011 red: `cargo test bolt_v3_archetype_accepts_coherent_short_side_order_contract -- --nocapture` failed because startup validation still required buy/long entry and sell/long exit.
- T012/T013 green: `cargo test bolt_v3_archetype_accepts_coherent_short_side_order_contract -- --nocapture` passed after replacing long-only tuple checks with strategy position-contract validation.
- T014 focused verification: `cargo test bolt_v3_archetype_ -- --nocapture` passed 8 config parsing tests; `cargo fmt -- --check` passed after rustfmt; `git diff --check` passed.

## Mid-Implementation Multi-Agent Review Findings

- Minimalism review recommended GTD expiry for currently enabled `Limit` orders before stop/touched/trailing expansion. Evidence cited: current path already uses NT enums and `OrderFactory::limit`, current validation still blocks GTD without expiry, and NT `OrderFactory::limit` already accepts `expire_time`.
- Venue/market review recommended GTD expiry first, then `StopMarket` as the lowest-risk future factory variant. Evidence cited: GTD expiry is an NT model invariant, while adapter GTD behavior remains venue-specific and must not become a Bolt capability matrix.
- End-to-end review recommended forced-exit semantics as the highest execution-risk slice. Evidence cited: forced-flat exit currently can reuse configured passive maker exit behavior, and exit submit currently has to prove position-aware context.
- Decision: defer Phase 7 factory variant expansion until after GTD expiry and forced-exit/position gates. T027-T029 remain support-claim gates for future non-limit/market variants.

## TDD Slice 5 Evidence

- T030 red: `cargo test bolt_v3_archetype_accepts_gtd_limit_order_with_expiry -- --nocapture` and `cargo test gtd_limit_order_objects_preserve_nt_expire_time -- --nocapture` failed because the order config had no `expire_time_unix_nanos` field.
- T030 green: `cargo test bolt_v3_archetype_accepts_gtd_limit_order_with_expiry -- --nocapture` passed after adding optional TOML-owned `expire_time_unix_nanos`.
- T030 green: `cargo test gtd_limit_order_objects_preserve_nt_expire_time -- --nocapture` passed after threading `expire_time_unix_nanos` into NT `OrderFactory::limit`.
- T030 negative guard: `cargo test bolt_v3_archetype_rejects_gtd_time_in_force_until_expiry_policy_exists -- --nocapture` still passed, proving GTD without expiry remains rejected.
- T030 focused verification: `cargo test bolt_v3_archetype_ -- --nocapture` passed 9 config parsing tests.

## TDD Slice 6 Evidence

- T032 red: `cargo test forced_flat_exit_uses_market_exit_config_when_normal_exit_is_post_only -- --nocapture` failed with missing `ExitSubmissionDecision` order-semantic fields, proving forced-flat exit could not expose whether it would submit the normal post-only limit exit or the market-exit TOML fields.
- T032 green: `cargo test forced_flat_exit_uses_market_exit_config_when_normal_exit_is_post_only -- --nocapture` passed after adding a private exit execution config selected from normal exit TOML or forced-flat market-exit TOML, then using that same config for exit order construction.
- Submit-boundary guard: `cargo test binary_oracle_edge_taker_exit_submit_threads_managed_position_id_to_nt -- --nocapture` passed after exit submission threaded the managed `PositionId` through `SubmitContext` to NT `submit_order`.
- Regression guard: `cargo test post_only_exit_submission_price_uses_passive_book_price -- --nocapture` passed after preserving normal post-only exit pricing separately from forced-flat market-exit pricing.
- Adjacent forced-flat checks: `cargo test task6_exit_submission_decision_forced_flat_submits_for_open -- --nocapture` passed.
- Submit-admission checks: `cargo test submit_admission -- --nocapture` passed.

## TDD Slice 7 Evidence

- NT source evidence: `OmsType` has `Unspecified`, `Netting`, and `Hedging` at `nautilus_trader/.../crates/model/src/enums.rs:1095`; NT execution accepts custom `position_id` for non-Netting and only validates the netting shape in `crates/execution/src/engine/mod.rs:2062`; NT determines Hedging position ids separately from Netting at `crates/execution/src/engine/mod.rs:2123`.
- T031 red: `cargo test strategy_core_accepts_nt_hedging_oms_type -- --nocapture` failed because runtime parsing accepted only `netting`.
- T031 red: `cargo test bolt_v3_strategy_oms_type_accepts_nt_variants -- --nocapture` failed because bolt-v3 validation rejected `Hedging`.
- T031 green: `cargo test strategy_core_accepts_nt_hedging_oms_type -- --nocapture` passed after runtime OMS parsing delegated to NT `OmsType`.
- T031 green: `cargo test bolt_v3_strategy_oms_type_accepts_nt_variants -- --nocapture` passed after removing the Bolt-only Netting validator restriction.
- T031 reduce-only guard: `cargo test forced_flat_exit_order_object_preserves_market_reduce_only_config -- --nocapture` passed, proving the forced-flat market-exit order object preserves `TimeInForce::Ioc`, `OrderType::Market`, and `is_reduce_only=true`.
- Residual: These are source/unit proofs. They do not prove live adapter-specific position behavior.

## Adapter-Proof Boundaries

- Adapter source evidence requirement: each venue/market support claim must name the exact NT adapter source path and the exact mapping for `OrderType`, `TimeInForce`, `post_only`, `reduce_only`, `position_id`, and submit params. Existing examples are Binance Futures at `crates/adapters/binance/src/futures/execution.rs`, Deribit at `crates/adapters/deribit/src/execution.rs`, Polymarket at `crates/adapters/polymarket/src/execution/mod.rs`, and OKX at `crates/adapters/okx/src/factories.rs`.
- No-submit smoke boundary: a no-submit proof may build the live node, load TOML, resolve SSM secrets, register NT clients, register strategies, warm reference data, build NT orders, record decision evidence, and stop before any exchange submit. It must not consume submit admission or call live exchange submit.
- Live/canary boundary: live/canary proof remains blocked without explicit user approval, exact branch/head, exact config checksum, submit-admission arming evidence, and post-run decision/admission/order lifecycle artifacts.
- Residual: no no-submit or live/canary artifact has been produced for this branch/head in this slice, so current claims remain source/unit-test claims only.

## Post-Implementation Review Findings

- Gemini custom-review job `6959020f-dcbc-4934-93ff-0dbf94d80c24` completed with `Verdict: APPROVE`. Source was sent for exactly five files: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`, `src/strategies/binary_oracle_edge_taker.rs`, `src/bolt_v3_validate.rs`, `tests/config_parsing.rs`, and `tests/bolt_v3_decision_evidence.rs`. Audit manifest recorded head `5978dc1cde84210f1a293d0b5a667aaa577945b3`, 5 files, 603207 bytes, and 15710 lines.
- Claude custom-review job `8ec493b3-f150-46a6-b5e6-aa54bff13dcc` ran with subscription/OAuth mode, completed with `Verdict: REQUEST_CHANGES`, and sent the same five scoped files at head `5978dc1cde84210f1a293d0b5a667aaa577945b3`.
- Claude blocking finding: a resting maker/GTD exit could partially fill, receive a `PositionChanged` residual open-position event, then have the remainder expire/cancel. The previous state machine kept `ExitPending` because `fill_received=true`, leaving an open residual position with no working exit order and blocking future exits.
- Resolution: `PendingExitState` now distinguishes terminal order events from authoritative residual open-position events. A filled exit remains pending when a stale terminal event races before `PositionClosed`, but after NT reports an open residual position and the exit order is terminal, exposure returns to `Managed` with the residual position.
- Kimi custom-review job `2c2bce72-c0c6-48f4-b84f-50de593e2b21` was skipped per the user timeout rule. Source was sent, but no review result arrived after more than 15 minutes. The plugin result still reported `status: running`; a later `ps -p 37734 -o pid,ppid,command` check returned no process row.
- Claude post-fix custom-review job `f5707fda-418c-4035-a1a4-52839289f32e` ran with subscription/OAuth mode against `src/strategies/binary_oracle_edge_taker.rs` at head `5978dc1cde84210f1a293d0b5a667aaa577945b3` and returned `Verdict: APPROVE`. It found no blocking issues and confirmed the partial-exit wedge is fixed without regressing stale full-fill terminal handling. Non-blocking comments were the load-bearing NT event-order invariant, duplicated transition logic around `on_position_closed`, and a pre-existing concern if NT emitted `PositionChanged` after `PositionClosed`.
- Gemini post-fix custom-review job `8026b59a-2412-4462-b0cb-6f5cc2cbccae` reviewed the same one-file scope at head `5978dc1cde84210f1a293d0b5a667aaa577945b3` and returned `Verdict: APPROVE` with no blocking issues. It agreed the state machine fix is conservative: if the residual position event never arrives, the strategy stays `ExitPending` rather than falsely flattening.
- Post-fix resolution: the event-order invariant is now documented in code at the point where `PositionChanged` after an exit fill is treated as authoritative residual exposure. The duplicated transition helper and post-close `PositionChanged` concern are recorded residual maintenance risks, not current blockers.

## TDD Slice 8 Evidence

- T044 RED: `cargo test partial_exit_fill_then_expire_restores_managed_residual_position -- --nocapture` failed before the fix with `assertion failed: pending_exit_ref(&strategy).is_none()`, proving the partial-exit expiry wedge.
- T045 GREEN: the same command passed after tracking exit terminal receipt and residual open-position observation separately.
- Post-review comment-only follow-up: after documenting the NT event-order invariant, `cargo fmt -- --check`, `git diff --check`, `cargo test partial_exit_fill_then_expire_restores_managed_residual_position -- --nocapture`, and `cargo test exit_pending -- --nocapture` passed.
- Regression guard: `cargo test exit_pending -- --nocapture` passed 5 tests, including stale filled-exit cancel/reject/expire behavior and normal cancel/reject/expire recovery.
- Formatting and diff checks: `cargo fmt -- --check` passed; `git diff --check` passed.
- Full verification: `cargo test` passed. The library test target ran 260 tests; `tests/config_parsing.rs` ran 62 tests; doc-tests ran 0 tests.

## No-Mistakes Evidence

- Prior TDD Slice 8 worktree branch: `codex/maker-order-proof-clean`.
- Prior TDD Slice 8 worktree head: `5978dc1cde84210f1a293d0b5a667aaa577945b3`.
- `no-mistakes status` is not proof for this branch/head. It reported active run `01KS04Q4BJ3HVN9T8580MK9N9E` on branch `refactor/386-bolt-v3-nt-vocab-alignment`, head `4d6f4ab0`, status `running`.
- `no-mistakes rerun` is also not proof. It failed with `fatal: ambiguous argument 'refs/heads/codex/maker-order-proof-clean^{commit}'`, so the gate repo could not resolve the current worktree branch ref.

## TDD Slice 9 Evidence

- T048 RED: PR #434 exact pushed head `6ef656139b4f96275ac604b4ef535f417673fd98` had `fmt-check` and `source-fence` failures. Local `just fmt-check` and `just source-fence` reproduced the runtime-literal allowlist failure in `scripts/test_verify_bolt_v3_runtime_literals.py::test_allowlist_exactness`.
- T048 RED follow-up: after the runtime-literal audit update, `just source-fence` reproduced the legacy-default fence failure on `src/strategies/binary_oracle_edge_taker.rs:114` because `SubmitContext` still derived `Default`.
- T049 GREEN: runtime-literal classifications now cover the NT order-intent schema fields and GTD positive-expiry invariants, and `SubmitContext` no longer derives unused `Default`.
- T050 verification: `python3 scripts/test_verify_bolt_v3_runtime_literals.py` passed; `python3 scripts/verify_bolt_v3_runtime_literals.py` passed; `just fmt-check` passed after rerunning outside the sandbox cache-lock restriction; `just source-fence` passed after rerunning outside the sandbox cache-lock restriction; `cargo test bolt_v3_archetype_accepts_mixed_maker_taker_order_configs -- --nocapture` passed; `cargo test bolt_v3_archetype_accepts_gtd_limit_order_with_expiry -- --nocapture` passed; `cargo test partial_exit_fill_then_expire_restores_managed_residual_position -- --nocapture` passed; `git diff --check` passed; full `cargo test` passed.
- T051 no-mistakes state before this follow-up commit: `no-mistakes status` reported daemon running with active unrelated run `01KS2TFBX0R3XRM64Y027T9HZR` on branch `codex/374-t013-t014-red-tests`, head `9a504fa8`, status `running`. This is not proof for `codex/maker-order-proof-clean`; post-commit gate proof must use a new run for the pushed follow-up head.

## TDD Slice 10 Evidence

- NT source evidence: pinned NT exposes `OrderFactory::stop_market` at `crates/common/src/factories/order.rs:221`; the `StopMarket` model requires a positive `trigger_price` and GTD expiry when `time_in_force=Gtd` in `crates/model/src/orders/stop_market.rs:76-105`; its `Order::price()` returns optional `protection_price` at `stop_market.rs:310`; its `Order::trigger_price()` returns `Some(trigger_price)` at `stop_market.rs:314`.
- T052 RED: `cargo test stop_market_order_objects_preserve_nt_trigger_price_and_admission -- --nocapture` failed before implementation with `no field 'trigger_price' on type 'BinaryOracleEdgeTakerOrderConfig'`, proving the runtime order config had no TOML-owned trigger-price path.
- T052/T053 GREEN: the same command passed after adding optional `trigger_price` to entry/exit order config, validating it as a positive finite price for `order_type=stop_market`, and constructing the NT order through `OrderFactory::stop_market`.
- T054 RED: `cargo test bolt_v3_archetype_accepts_stop_market_entry_with_trigger_price -- --nocapture` failed before archetype support with `unknown field 'trigger_price'` in `entry_order`.
- T054 GREEN: the same command passed after adding `trigger_price` to the public archetype order schema, runtime table projection, and builder table validation.
- T055 verification: `cargo test stop_market_order_objects_preserve_nt_trigger_price_and_admission -- --nocapture` passed; `cargo test bolt_v3_archetype_accepts_stop_market_entry_with_trigger_price -- --nocapture` passed; `python3 scripts/test_verify_bolt_v3_runtime_literals.py` passed; `python3 scripts/verify_bolt_v3_runtime_literals.py` passed; `cargo fmt -- --check` passed; `git diff --check` passed; full `cargo test` passed with 261 library tests, 63 `tests/config_parsing.rs` tests, and 0 doc-tests. After the evidence docs were updated, `just fmt-check` and `just source-fence` each reproduced the known sandbox cache-lock failure on `/Users/spson/.cache/rust-verification/bolt-v2/cache.lock`, then passed when rerun outside the sandbox cache-lock restriction.
- Exact-head gate state before this StopMarket commit: PR #434 pushed head `473fd898cd820315d82f6bb18eb13ff4f413fe84` had green GitHub checks, but that is historical only. The StopMarket changes were still local when this evidence was recorded, so a new post-commit/push gate is required.
- no-mistakes state before this StopMarket commit: `no-mistakes status` reported active unrelated run `01KS2W6GSACEQ7TW434MS1FQTJ` on branch `codex/374-t013-t014-red-tests`, head `49f88fd1`, status `running`. This is not proof for `codex/maker-order-proof-clean`; post-commit gate proof must use a new run for the pushed StopMarket head.
- Residual unsupported NT factory variants: `StopLimit`, `MarketIfTouched`, `LimitIfTouched`, and `TrailingStopMarket` still need one TDD variant slice each before support can be claimed. `MarketToLimit` and `TrailingStopLimit` still need separate approval/upstream factory support because the pinned NT `OrderFactory` does not expose single-order factory methods for them.
- Live/canary residual: this slice is source/unit proof only. It does not prove adapter-specific legality, no-submit startup, live exchange submission, or canary execution for StopMarket.

## TDD Slice 11 Evidence

- External review: Gemini job `28a8f7db-12ed-4647-91ec-d7d6ad21c903` reviewed base `473fd898cd820315d82f6bb18eb13ff4f413fe84` to head `25080b7f381f1d8f4e78fe5d6d0e130c6e019090`, sent 6 files, and returned `Verdict: REQUEST_CHANGES`. Blocking finding: StopMarket admission fell back to pre-trigger `intent.price` because NT `StopMarket::price()` is `None`, underestimating notional when trigger price is worse than the current book price.
- External review: Claude job `0b15f602-8e7e-4513-bba0-c70918ba24f8` ran with subscription/OAuth mode, reviewed the same base/head and 6 files, and returned `Verdict: REQUEST_CHANGES`. Blocking finding: archetype projection used Debug-lowercase enum serialization, emitting `stopmarket` instead of NT's serde/display-compatible `stop_market`, so validation could pass while runtime strategy parsing failed.
- T057 RED: `cargo test stop_market_entry_submission_price_uses_trigger_price_for_notional_sizing -- --nocapture` failed with left `Some(0.41)` and right `Some(0.52)`, proving entry sizing still used pre-trigger book price. `cargo test stop_market_order_objects_preserve_nt_trigger_price_and_admission -- --nocapture` failed with left `0.8000` and right `1.040`, proving admission notional ignored the trigger price.
- T058 GREEN: StopMarket entry pricing now uses the TOML-owned positive trigger price for sizing/submission price, and submit admission falls back to NT `Order::trigger_price()` when `Order::price()` is absent. `cargo test stop_market -- --nocapture` passed after the fix.
- T059 RED/GREEN: `cargo test binary_oracle_runtime_mapping_preserves_stop_market_entry_order_round_trip -- --nocapture` first failed with left `Some("stopmarket")` and right `Some("stop_market")`. The archetype enum projection now uses the NT/strum display form lowercased instead of Debug-lowercase, and the same test passed, including `BinaryOracleEdgeTakerBuilder::build` from the raw runtime table.
- T060 verification: `cargo fmt -- --check` passed; `git diff --check` passed; `python3 scripts/test_verify_bolt_v3_runtime_literals.py` passed; `python3 scripts/verify_bolt_v3_runtime_literals.py` passed; full `cargo test` passed with 262 library tests, 63 `tests/config_parsing.rs` tests, 9 `tests/bolt_v3_strategy_registration.rs` tests, and 0 doc-tests; `just fmt-check` passed with cache-lock escalation; `just source-fence` passed with cache-lock escalation.
- Exact-head gate state: head `25080b7f381f1d8f4e78fe5d6d0e130c6e019090` is superseded by this local post-review fix. Its previous GitHub/no-mistakes state is historical only; a new post-commit/push exact-head gate is required before claiming the review blockers are resolved on the PR head.
- Residual non-blocking review item: Claude noted that archetype validation still permits stray `trigger_price` on non-StopMarket combinations and runtime validation rejects it later. This is fail-closed and is not a StopMarket blocker, but it remains a validation-quality cleanup candidate.

## TDD Slice 12 Evidence

- Pre-slice internal NT-source review compared `StopLimit`, `MarketIfTouched`, `LimitIfTouched`, and `TrailingStopMarket` in pinned NT checkout `38b912a8b0fe14e4046773973ff46a3b798b1e3e`. Evidence: `OrderFactory::stop_limit` accepts runtime `price`, TOML-owned `trigger_price`, optional TIF/expiry, post-only, reduce-only, and quote-quantity at `crates/common/src/factories/order.rs:276-326`; `StopLimitOrder::new_checked` validates positive quantity, display quantity, and GTD expiry at `crates/model/src/orders/stop_limit.rs:73-104`; `Order::price()` and `Order::trigger_price()` return `Some(price)` and `Some(trigger_price)` at `crates/model/src/orders/stop_limit.rs:313-318`; NT test coverage rejects GTD StopLimit without expiry at `crates/model/src/orders/stop_limit.rs:689-700`.
- Pre-slice internal Bolt-path review recommended `StopLimit` over `MarketIfTouched` for this slice because Bolt already has both required inputs: strategy-computed runtime limit `price` and TOML-owned `trigger_price`. `MarketIfTouched` is still source-supported by NT but would require a separate triggered-market sizing decision because its `Order::price()` is absent.
- Decision: implement `StopLimit` next. This advances the factory-variant support gate without a venue capability matrix and without changing `StopMarket`/`MarketIfTouched` sizing semantics.
- T064 RED: `cargo test stop_limit_order_objects_preserve_nt_price_trigger_and_admission -- --nocapture` failed before runtime support with `entry_order_type supports \`limit\`, \`market\`, or \`stop_market\`, got \`StopLimit\``.
- T064 GREEN: the same test passed after adding StopLimit NT-model validation and constructing the order through `OrderFactory::stop_limit`. The regression proves the compiled NT `StopLimit` preserves `price`, `trigger_price`, TIF, GTD `expire_time`, post-only/reduce-only/quote flags, and submit admission notional from `Order::price()`.
- T065 RED: `cargo test bolt_v3_archetype_accepts_stop_limit_entry_with_trigger_price -- --nocapture` failed at archetype validation because StopLimit was not an allowed `binary_oracle_edge_taker` order combination.
- T065 GREEN: the same command passed after allowing `order_type=stop_limit` with positive `trigger_price`, valid GTD expiry when needed, and strategy-scope `is_reduce_only=false` / `is_quote_quantity=false`. `is_post_only` remains TOML-owned and is not forced off for StopLimit because NT supports post-only on `OrderFactory::stop_limit`.
- T066 focused coverage: `cargo test stop_limit -- --nocapture` passed, including `binary_oracle_runtime_mapping_preserves_stop_limit_entry_order_round_trip`, which proves archetype projection emits `order_type="stop_limit"`, preserves `trigger_price`, preserves `is_post_only=true`, validates the raw runtime table, and builds the strategy config.
- T067 local verification: `cargo fmt -- --check` passed; `git diff --check` passed; `python3 scripts/test_verify_bolt_v3_runtime_literals.py` passed; `python3 scripts/verify_bolt_v3_runtime_literals.py` passed after updating the runtime literal audit for the shared GTD-expiry helper and StopLimit diagnostic string; full `cargo test` passed with 263 library tests, 64 `tests/config_parsing.rs` tests, 10 `tests/bolt_v3_strategy_registration.rs` tests, and 0 doc-tests; `just fmt-check` passed with cache-lock escalation; `just source-fence` passed with cache-lock escalation.
- T068 external review: Gemini job `f914a457-2cad-414c-ba97-d7e7e624b724` reviewed base `e28605e01ce722a14e09196da4ce3a9fa820333d` to head `35ac5faf9e17aa1924fba2d1b13956a04e483684`, sent 8 files, and returned `Verdict: APPROVE` with non-blocking regression gaps for exit-side StopLimit coverage. Claude job `d805de4e-7363-452b-9fe9-3471d48775bf` used subscription/OAuth mode and sent the same source, but the review slot failed quality audit with `review_quality_failed:not_reviewed` because it could not independently verify the diff boundary; its advisory output also listed missing GTD, post-only, negative archetype, and exit StopLimit tests.
- T069/T070/T071 regression coverage: the post-review patch adds GTD-expiry and post-only assertions to the real NT StopLimit construction test, adds a StopLimit GTD-without-expiry rejection to the pre-factory validation test, adds public exit StopLimit validation coverage, adds raw exit StopLimit runtime round-trip/build coverage, and adds negative archetype coverage for missing trigger, zero trigger, `is_reduce_only=true`, and `is_quote_quantity=true`.
- T072 local verification before commit: `cargo test stop_limit -- --nocapture` passed, including one library StopLimit factory/admission test, entry/exit runtime round-trip tests, and five config parsing/archetype StopLimit tests. `cargo test configured_order_build_rejects_nt_model_invalid_tif_before_factory -- --nocapture` passed, covering the new StopLimit GTD-missing-expiry rejection. `cargo fmt -- --check` passed; `git diff --check` passed; `python3 scripts/test_verify_bolt_v3_runtime_literals.py` passed; `python3 scripts/verify_bolt_v3_runtime_literals.py` passed; full `cargo test` passed with 263 library tests, 68 `tests/config_parsing.rs` tests, 11 `tests/bolt_v3_strategy_registration.rs` tests, and 0 doc-tests; `just fmt-check` passed with cache-lock escalation; `just source-fence` passed with cache-lock escalation.
- Exact-head gate state: the initial StopLimit commit `35ac5faf9e17aa1924fba2d1b13956a04e483684` was pushed and had required GitHub checks green; Greptile remained pending and non-required. This post-review regression patch is local and unpushed when this evidence was recorded, so a new post-commit/push exact-head gate is required before claiming StopLimit support on the PR head.
- Residual unsupported NT factory variants after this slice: `MarketIfTouched`, `LimitIfTouched`, and `TrailingStopMarket` still need one TDD variant slice each before support can be claimed. `MarketToLimit` and `TrailingStopLimit` still need separate approval/upstream factory support because the pinned NT `OrderFactory` does not expose single-order factory methods for them.
- Live/canary residual: this slice is source/unit proof only. It does not prove adapter-specific legality, no-submit startup, live exchange submission, or canary execution for StopLimit.
