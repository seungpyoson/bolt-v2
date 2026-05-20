# Strategy Source Excerpts For Maker-Order Audit

Exact implementation head: `52ab7719f6b0770cf3ca45b7c82f5cd8fe843f8d`

Purpose: give external reviewers the exact source ranges needed to verify end-to-end maker-order wiring without sending the full `src/strategies/binary_oracle_edge_taker.rs` file.

## `src/strategies/binary_oracle_edge_taker.rs:421-438`

```rust
    fn executable_price_for_order_side(&self, order_side: OrderSide) -> Option<f64> {
        match order_side {
            OrderSide::Buy => self.best_ask,
            OrderSide::Sell => self.best_bid,
            _ => None,
        }
        .filter(|value| is_positive_finite(*value))
    }

    fn passive_price_for_order_side(&self, order_side: OrderSide) -> Option<f64> {
        match order_side {
            OrderSide::Buy => self.best_bid,
            OrderSide::Sell => self.best_ask,
            _ => None,
        }
        .filter(|value| is_positive_finite(*value))
    }
```

## `src/strategies/binary_oracle_edge_taker.rs:1096-1151`

```rust
fn order_price_for_side(
    book: &OutcomeBookState,
    order_side: OrderSide,
    is_post_only: bool,
) -> Option<f64> {
    if is_post_only {
        book.passive_price_for_order_side(order_side)
    } else {
        book.executable_price_for_order_side(order_side)
    }
}

fn infer_strategy_position_side_from_entry_fill(
    entry_order_side: OrderSide,
    configured_entry_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<PositionSide> {
    (entry_order_side == configured_entry_order_side).then_some(configured_position_side)
}

fn managed_position_effective_entry_cost(
    position: &OpenPositionState,
    configured_entry_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<f64> {
    (position.entry_order_side == configured_entry_order_side
        && position.side == configured_position_side)
        .then_some(position.avg_px_open)
        .filter(|effective_cost| is_positive_finite(*effective_cost))
}

fn managed_position_exit_order(
    position: &OpenPositionState,
    configured_order_side: OrderSide,
    configured_position_side: PositionSide,
    is_post_only: bool,
) -> Option<(OrderSide, f64)> {
    (position.side == configured_position_side)
        .then_some((
            configured_order_side,
            order_price_for_side(&position.book, configured_order_side, is_post_only)?,
        ))
        .filter(|(_, price)| is_positive_finite(*price))
}

fn managed_position_exit_value(
    position: &OpenPositionState,
    configured_order_side: OrderSide,
    configured_position_side: PositionSide,
    is_post_only: bool,
) -> Option<f64> {
    let value = (position.side == configured_position_side)
        .then(|| order_price_for_side(&position.book, configured_order_side, is_post_only))
        .flatten()?;
    Some(value).filter(|value| is_positive_finite(*value))
}
```

## `src/strategies/binary_oracle_edge_taker.rs:2668-2680`

```rust
    fn executable_entry_cost(&self, side: OutcomeSide) -> Option<f64> {
        let order_side = self.configured_entry_order_side().ok()?;
        let book = self.active_book_for_outcome(side);
        if self.config.entry_order.is_post_only {
            book.passive_price_for_order_side(order_side)
        } else {
            book.executable_price_for_order_side(order_side)
        }
    }

    fn submission_entry_price(&self, side: OutcomeSide) -> Option<f64> {
        self.executable_entry_cost(side)
    }
```

## `src/strategies/binary_oracle_edge_taker.rs:3022-3042`

```rust
    fn current_exit_order_for_open_position(&self) -> Option<(OrderSide, f64)> {
        let open_position = &self.managed_position()?.position;
        let contract = self.configured_position_contract().ok()?;
        managed_position_exit_order(
            open_position,
            contract.exit_order_side,
            contract.exit_position_side,
            self.config.exit_order.is_post_only,
        )
    }

    fn current_exit_value_for_open_position(&self) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let contract = self.configured_position_contract().ok()?;
        managed_position_exit_value(
            open_position,
            contract.exit_order_side,
            contract.exit_position_side,
            self.config.exit_order.is_post_only,
        )
    }
```

## `src/strategies/binary_oracle_edge_taker.rs:3447-3510`

```rust
    fn submit_order_with_decision_evidence(
        &mut self,
        intent: BoltV3OrderIntentEvidence,
        order: nautilus_model::orders::OrderAny,
        client_id: ClientId,
    ) -> Result<()> {
        self.context
            .decision_evidence()
            .record_order_intent(&intent)?;
        let request = submit_admission_request_from_intent(&intent)?;
        let _permit = self.context.submit_admission().admit(&request)?;
        self.submit_order(order, None, Some(client_id), None)
    }

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
        )
    }
```

## `src/strategies/binary_oracle_edge_taker.rs:4728-4784`

```rust
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
                instrument_id,
                order_side,
                quantity,
                Some(time_in_force),
                Some(is_reduce_only),
                Some(is_quote_quantity),
                None,
                None,
                None,
                Some(client_order_id),
            ))
        }
    }
}
```

## `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:110-118`

```rust
pub struct OrderParams {
    pub side: OrderSide,
    pub position_side: PositionSide,
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}
```

## `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:653-682`

```rust
fn insert_order_config(table: &mut Map<String, Value>, key: &'static str, order: &OrderParams) {
    let mut order_table = Map::new();
    insert_string(&mut order_table, "side", enum_variant_lowercase(order.side));
    insert_string(
        &mut order_table,
        "position_side",
        enum_variant_lowercase(order.position_side),
    );
    insert_string(
        &mut order_table,
        "order_type",
        enum_variant_lowercase(order.order_type),
    );
    insert_string(
        &mut order_table,
        "time_in_force",
        enum_variant_lowercase(order.time_in_force),
    );
    insert_bool(&mut order_table, "is_post_only", order.is_post_only);
    insert_bool(&mut order_table, "is_reduce_only", order.is_reduce_only);
    insert_bool(
        &mut order_table,
        "is_quote_quantity",
        order.is_quote_quantity,
    );
    table.insert(key.to_string(), Value::Table(order_table));
}
```

## `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:805-886`

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
