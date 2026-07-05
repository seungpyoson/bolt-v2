use nautilus_model::identifiers::InstrumentId;

pub fn prediction_market_product_id_from_instrument_id(
    instrument_id: &InstrumentId,
) -> Option<String> {
    instrument_id
        .symbol
        .as_str()
        .rsplit_once('-')
        .and_then(|(_, product_id)| (!product_id.is_empty()).then(|| product_id.to_string()))
}
