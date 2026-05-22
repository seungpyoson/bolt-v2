# Final-Head Maker Order Audit Packet

Exact PR head under audit: `4c214d6875e4fb9f75224f0d9649b0145f2dbf4a`

PR: `https://github.com/seungpyoson/bolt-v2/pull/434`

Scope: post-implementation review of maker-order enablement for the `binary_oracle_edge_taker` bolt-v3 archetype. This packet exists because the full strategy source file is too large for some direct-review provider caps.

## Remaining Speckit Tasks At Audit Start

- T024: post-implementation Claude audit after exact PR head CI is green.
- T025: post-implementation Gemini audit after exact PR head CI is green.
- T026: post-implementation Kimi audit after exact PR head CI is green.
- T027: post-implementation DeepSeek audit after exact PR head CI is green.
- T028: post-implementation GLM audit after exact PR head CI is green.
- T029: resolve or document every audit finding before merge/readiness completion.
- T048: re-run required verification, commit follow-up changes, push, and confirm exact PR-head CI before closing T024-T029.

## Key Behavioral Claim

Bolt-v3 can now configure maker entry and maker exit independently through TOML-owned `[parameters.entry_order]` and `[parameters.exit_order]` rows:

- maker entry: `side=buy`, `position_side=long`, `order_type=limit`, `time_in_force=gtc`, `is_post_only=true`, `is_reduce_only=false`, `is_quote_quantity=false`
- taker entry remains allowed: `side=buy`, `position_side=long`, `order_type=limit`, `time_in_force=fok`, `is_post_only=false`
- maker exit: `side=sell`, `position_side=long`, `order_type=limit`, `time_in_force=gtc`, `is_post_only=true`, `is_reduce_only=false`, `is_quote_quantity=false`
- taker exit remains allowed: `side=sell`, `position_side=long`, `order_type=market`, `time_in_force=ioc`, `is_post_only=false`

GTD remains blocked because NT supports `Gtd`, but bolt-v3 has no approved TOML-owned expiry policy.

## Archetype Validation Excerpt

`src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:805-881`

```rust
fn check_entry_order_combination(context: &str, entry: &OrderParams) -> Vec<String> {
    let taker_limit_fok = (
        OrderSide::Buy,
        PositionSide::Long,
        OrderType::Limit,
        TimeInForce::Fok,
        false,
        false,
        false,
    );
    let maker_limit_gtc = (
        OrderSide::Buy,
        PositionSide::Long,
        OrderType::Limit,
        TimeInForce::Gtc,
        true,
        false,
        false,
    );
    let actual = (
        entry.side,
        entry.position_side,
        entry.order_type,
        entry.time_in_force,
        entry.is_post_only,
        entry.is_reduce_only,
        entry.is_quote_quantity,
    );
    if actual != taker_limit_fok && actual != maker_limit_gtc {
        vec![format!(
            "{context}: parameters.entry_order combination is not allowed for `binary_oracle_edge_taker`; \
             only side=buy, position_side=long, order_type=limit with either time_in_force=fok, is_post_only=false or time_in_force=gtc, is_post_only=true is allowed; \
             is_reduce_only=false and is_quote_quantity=false are required"
        )]
    } else {
        Vec::new()
    }
}

fn check_exit_order_combination(context: &str, exit: &OrderParams) -> Vec<String> {
    let taker_market_ioc = (
        OrderSide::Sell,
        PositionSide::Long,
        OrderType::Market,
        TimeInForce::Ioc,
        false,
        false,
        false,
    );
    let maker_limit_gtc = (
        OrderSide::Sell,
        PositionSide::Long,
        OrderType::Limit,
        TimeInForce::Gtc,
        true,
        false,
        false,
    );
    let actual = (
        exit.side,
        exit.position_side,
        exit.order_type,
        exit.time_in_force,
        exit.is_post_only,
        exit.is_reduce_only,
        exit.is_quote_quantity,
    );
    if actual != taker_market_ioc && actual != maker_limit_gtc {
        vec![format!(
            "{context}: parameters.exit_order combination is not allowed for `binary_oracle_edge_taker`; \
             only side=sell, position_side=long with either order_type=market, time_in_force=ioc, is_post_only=false or order_type=limit, time_in_force=gtc, is_post_only=true is allowed; \
             is_reduce_only=false and is_quote_quantity=false are required"
        )]
    } else {
        Vec::new()
    }
}
```

## Strategy Order Construction Excerpt

`src/strategies/binary_oracle_edge_taker.rs:4711-4778`

```rust
fn parse_configured_time_in_force(field: &str, value: &str) -> Result<TimeInForce> {
    match value {
        TIME_IN_FORCE_GTC_VALUE => Ok(TimeInForce::Gtc),
        TIME_IN_FORCE_FOK_VALUE => Ok(TimeInForce::Fok),
        TIME_IN_FORCE_IOC_VALUE => Ok(TimeInForce::Ioc),
        _ => anyhow::bail!("{field} must be `gtc`, `fok`, or `ioc`, got `{value}`"),
    }
}

#[expect(clippy::too_many_arguments)]
fn build_configured_order(
    core: &mut StrategyCore,
    prefix: &'static str,
    order_type: &str,
    time_in_force: &str,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
    instrument_id: InstrumentId,
    order_side: OrderSide,
    quantity: Quantity,
    price: Price,
    client_order_id: ClientOrderId,
) -> Result<nautilus_model::orders::OrderAny> {
    let order_type = parse_configured_order_type(&format!("{prefix}_order_type"), order_type)?;
    let time_in_force =
        parse_configured_time_in_force(&format!("{prefix}_time_in_force"), time_in_force)?;
    match order_type {
        ConfiguredOrderType::Limit => Ok(core.order_factory().limit(
            instrument_id,
            order_side,
            quantity,
            price,
            Some(time_in_force),
            None,
            Some(is_post_only),
            Some(is_reduce_only),
            Some(is_quote_quantity),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(client_order_id),
        )),
        ConfiguredOrderType::Market => {
            anyhow::ensure!(
                !is_post_only,
                "{prefix}_is_post_only must be false for market orders"
            );
            Ok(core.order_factory().market(
```

`src/strategies/binary_oracle_edge_taker.rs:3460-3505`

```rust
    fn build_configured_entry_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        build_configured_order(
            &mut self.core,
            ORDER_CONFIGURATION_PREFIX_ENTRY,
            &self.config.entry_order.order_type,
            &self.config.entry_order.time_in_force,
            self.config.entry_order.is_post_only,
            self.config.entry_order.is_reduce_only,
            self.config.entry_order.is_quote_quantity,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )
    }

    fn build_configured_exit_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        build_configured_order(
            &mut self.core,
            ORDER_CONFIGURATION_PREFIX_EXIT,
            &self.config.exit_order.order_type,
            &self.config.exit_order.time_in_force,
            self.config.exit_order.is_post_only,
            self.config.exit_order.is_reduce_only,
            self.config.exit_order.is_quote_quantity,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
```

## Direct Order Object Test Excerpt

`src/strategies/binary_oracle_edge_taker.rs:8516-8570`

```rust
    fn assert_limit_gtc_post_only_order(
        order: OrderAny,
        expected_side: OrderSide,
        expected_price: Price,
    ) {
        let OrderAny::Limit(order) = order else {
            panic!("maker order should be built as an NT limit order");
        };
        assert_eq!(order.order_side(), expected_side);
        assert_eq!(order.order_type(), OrderType::Limit);
        assert_eq!(order.time_in_force(), TimeInForce::Gtc);
        assert_eq!(order.price(), Some(expected_price));
        assert!(order.is_post_only());
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(order.expire_time(), None);
    }

    #[test]
    fn post_only_maker_order_objects_preserve_nt_limit_gtc_fields() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        strategy.config.entry_order.time_in_force = "gtc".to_string();
        strategy.config.entry_order.is_post_only = true;
        strategy.config.exit_order.order_type = "limit".to_string();
        strategy.config.exit_order.time_in_force = "gtc".to_string();
        strategy.config.exit_order.is_post_only = true;

        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-DOWN.POLYMARKET");
        let quantity = Quantity::new(1.0, 2);
        let entry_price = Price::new(0.40, 2);
        let entry_order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                entry_price,
                ClientOrderId::from("O-19700101-000000-001-001-1"),
            )
            .expect("maker entry order should build");
        assert_limit_gtc_post_only_order(entry_order, OrderSide::Buy, entry_price);

        let exit_price = Price::new(0.45, 2);
        let exit_order = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                quantity,
                exit_price,
                ClientOrderId::from("O-19700101-000000-001-002-1"),
            )
            .expect("maker exit order should build");
        assert_limit_gtc_post_only_order(exit_order, OrderSide::Sell, exit_price);
    }
```

## Runtime Mapping Test Excerpts

`tests/bolt_v3_strategy_registration.rs:296-391`

```rust
fn binary_oracle_runtime_mapping_preserves_post_only_gtc_entry_order() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "bitcoin_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("post-only GTC entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("limit")
    );
    assert_eq!(
        entry.get("time_in_force").and_then(toml::Value::as_str),
        Some("gtc")
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        entry.get("is_reduce_only").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry
            .get("is_quote_quantity")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
}

#[test]
fn binary_oracle_runtime_mapping_preserves_post_only_gtc_exit_order() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "bitcoin_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("post-only GTC exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("limit")
    );
    assert_eq!(
        exit.get("time_in_force").and_then(toml::Value::as_str),
        Some("gtc")
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        exit.get("is_reduce_only").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        exit.get("is_quote_quantity")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
}
```

## Passive Price Test Excerpts

`src/strategies/binary_oracle_edge_taker.rs:8460-8514`

```rust
    #[test]
    fn post_only_entry_submission_price_uses_passive_book_price() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.entry_order.time_in_force = "gtc".to_string();
        strategy.config.entry_order.is_post_only = true;
        strategy.active.books.down.best_bid = Some(0.40);
        strategy.active.books.down.best_ask = Some(0.41);

        assert_eq!(
            strategy.submission_entry_price(OutcomeSide::Down),
            Some(0.40)
        );
        assert_eq!(
            strategy.executable_entry_cost(OutcomeSide::Down),
            Some(0.40)
        );
    }

    #[test]
    fn post_only_exit_submission_price_uses_passive_book_price() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.active.phase = SelectionPhase::Freeze;
        strategy.config.exit_order.order_type = "limit".to_string();
        strategy.config.exit_order.time_in_force = "gtc".to_string();
        strategy.config.exit_order.is_post_only = true;
        strategy.active.books.up.best_bid = Some(0.44);
        strategy.active.books.up.best_ask = Some(0.45);
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-UP-001"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        let expected_passive_price = open_position.book.best_ask;
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(decision.price, expected_passive_price);
    }
```

## Config Validation Test Excerpts

`tests/config_parsing.rs:613-690`

```rust
fn bolt_v3_archetype_accepts_post_only_gtc_entry_order() {
    let maker_strategy = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace("time_in_force = \"fok\"", "time_in_force = \"gtc\"")
    .replacen("is_post_only = false", "is_post_only = true", 1);
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_strategy)
        .expect("post-only GTC entry order should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "post-only GTC entry order should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
    );
}

fn bolt_v3_archetype_accepts_post_only_gtc_exit_order() {
    let taker_strategy = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let maker_exit_strategy = taker_strategy
        .replace("order_type = \"market\"", "order_type = \"limit\"")
        .replace("time_in_force = \"ioc\"", "time_in_force = \"gtc\"");
    let (before_exit, exit_block) = maker_exit_strategy
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit order block");
    let maker_exit_strategy = format!(
        "{before_exit}[parameters.exit_order]{}",
        exit_block.replacen("is_post_only = false", "is_post_only = true", 1)
    );
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_exit_strategy)
        .expect("post-only GTC exit order should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "post-only GTC exit order should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
    );
}
```

`tests/config_parsing.rs:743-793`

```rust
fn bolt_v3_archetype_rejects_gtd_time_in_force_until_expiry_policy_exists() {
    let entry_gtd_strategy: BoltV3StrategyConfig =
        toml::from_str(&fixture.replace("time_in_force = \"fok\"", "time_in_force = \"gtd\""))
            .expect("gtd should parse via NT TimeInForce");
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|m| {
            m.contains("entry_order")
                && m.contains("time_in_force=fok")
                && m.contains("time_in_force=gtc")
        }),
        "expected entry_order GTD rejection until an expiry policy exists, got: {messages:#?}"
    );

    let exit_gtd_strategy: BoltV3StrategyConfig =
        toml::from_str(&fixture.replace("time_in_force = \"ioc\"", "time_in_force = \"gtd\""))
            .expect("gtd should parse via NT TimeInForce");
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|m| {
            m.contains("exit_order")
                && m.contains("time_in_force=ioc")
                && m.contains("time_in_force=gtc")
        }),
        "expected exit_order GTD rejection until an expiry policy exists, got: {messages:#?}"
    );
}
```

## NT Adapter Evidence

`tests/fixtures/nt_polymarket_query_post_order_params_7c2aafb.txt` is a committed fixture from pinned NT revision `7c2aafb`. `tests/config_parsing.rs` asserts its SHA-256 and checks that NT serializes the Polymarket post-only flag as `postOnly`.

## Docs Contract Evidence

`specs/022-nt-maker-order-scope/contracts/maker-order-config.md` and `docs/bolt-v3/2026-04-25-bolt-v3-schema.md` state:

- maker entry and maker exit are `limit` + `gtc` + `is_post_only=true`
- maker exit is passive and can remain unfilled
- operators needing immediate flattening must configure taker exit until a separate TOML-owned forced-exit override exists
- GTD stays disabled until a TOML-owned expiry policy is approved

## Verification Already Run Before This Audit Packet

- `cargo fmt -- --check`: pass
- `git diff --check`: pass
- `cargo test bolt_v3_archetype_rejects_gtd_time_in_force_until_expiry_policy_exists -- --nocapture`: pass
- `cargo test post_only_maker_order_objects_preserve_nt_limit_gtc_fields -- --nocapture`: pass
- `just source-fence`: pass after sandbox cache-lock denial and escalated rerun
- `cargo test`: pass, including 251 lib tests, all integration tests, and doc tests

External reviewers should report `APPROVE`, `REQUEST_CHANGES`, or `NEEDS_INFO`, with blocking findings first. Review focus: whether this implementation truly enables NT-compatible maker entry and maker exit without speculative GTD policy, hardcoded runtime values, or hidden dual paths.
