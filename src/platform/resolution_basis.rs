pub fn parse_declared_resolution_basis(
    resolution_source: Option<&str>,
    description: Option<&str>,
) -> Option<String> {
    let source = normalize_input(resolution_source);
    if contains_all(&source, &["chainlink", "btcusd"]) {
        return Some("chainlink_btcusd".to_string());
    }
    if contains_all(&source, &["binance", "btcusdt"]) {
        return Some("binance_btcusdt_1m".to_string());
    }

    let description = normalize_input(description);
    if contains_all(&description, &["resolutionsource", "binance", "btcusdt"]) {
        return Some("binance_btcusdt_1m".to_string());
    }
    if contains_all(&description, &["resolutionsource", "chainlink", "btcusd"]) {
        return Some("chainlink_btcusd".to_string());
    }

    None
}

fn normalize_input(input: Option<&str>) -> String {
    input
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn contains_all(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().all(|needle| haystack.contains(needle))
}
