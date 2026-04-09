pub fn parse_declared_resolution_basis(
    resolution_source: Option<&str>,
    description: Option<&str>,
) -> Option<String> {
    let source = resolution_source.unwrap_or_default();
    if source.contains("chain.link/streams/btc-usd") {
        return Some("chainlink_btcusd".to_string());
    }
    if source.contains("binance.com/en/trade/BTC_USDT") {
        return Some("binance_btcusdt_1m".to_string());
    }

    let description = description.unwrap_or_default();
    if description.contains("resolution source for this market is Binance") {
        return Some("binance_btcusdt_1m".to_string());
    }
    if description.contains("resolution source for this market is information from Chainlink") {
        return Some("chainlink_btcusd".to_string());
    }

    None
}
