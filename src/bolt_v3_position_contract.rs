use nautilus_model::enums::{OrderSide, PositionSide};

pub fn expected_position_side_for_entry_order(order_side: OrderSide) -> Option<PositionSide> {
    match order_side {
        OrderSide::Buy => Some(PositionSide::Long),
        OrderSide::Sell => Some(PositionSide::Short),
        _ => None,
    }
}

pub fn expected_exit_order_side_for_position(position_side: PositionSide) -> Option<OrderSide> {
    match position_side {
        PositionSide::Long => Some(OrderSide::Sell),
        PositionSide::Short => Some(OrderSide::Buy),
        _ => None,
    }
}

pub fn is_observed_open_side(side: PositionSide) -> bool {
    matches!(side, PositionSide::Long | PositionSide::Short)
}
