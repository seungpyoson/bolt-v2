use std::collections::BTreeSet;

use crate::config::ReferenceVenueKind;

const SUPPORTED_FAMILIES: &[&str] = &[
    "binance",
    "bybit",
    "deribit",
    "hyperliquid",
    "kraken",
    "okx",
    "polymarket",
    "chainlink",
];

const SYMBOL_STOPWORDS: &[&str] = &[
    "com",
    "data",
    "en",
    "feed",
    "feeds",
    "for",
    "from",
    "http",
    "https",
    "information",
    "is",
    "market",
    "markets",
    "price",
    "prices",
    "resolution",
    "source",
    "spot",
    "stream",
    "streams",
    "the",
    "this",
    "trade",
    "used",
    "will",
    "www",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionBasis {
    family: String,
    symbol: String,
    cadence: Option<String>,
}

impl ResolutionBasis {
    pub fn canonical(&self) -> String {
        match &self.cadence {
            Some(cadence) => format!("{}_{}_{}", self.family, self.symbol, cadence),
            None => format!("{}_{}", self.family, self.symbol),
        }
    }

    pub fn family(&self) -> &str {
        &self.family
    }
}

pub fn parse_resolution_basis(input: &str) -> Option<ResolutionBasis> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parts: Vec<&str> = trimmed.split('_').collect();
    if parts.len() < 2 {
        return None;
    }

    let family = normalize_basis_part(parts[0])?;
    let symbol = normalize_basis_part(parts[1])?;
    let cadence = if parts.len() > 2 {
        let mut normalized_parts = Vec::with_capacity(parts.len() - 2);
        for part in &parts[2..] {
            normalized_parts.push(normalize_basis_part(part)?);
        }
        Some(normalized_parts.join("_"))
    } else {
        None
    };

    Some(ResolutionBasis {
        family,
        symbol,
        cadence,
    })
}

pub fn parse_declared_resolution_basis(
    resolution_source: Option<&str>,
    description: Option<&str>,
) -> Option<String> {
    [resolution_source, description]
        .into_iter()
        .flatten()
        .find_map(parse_declared_resolution_basis_from_input)
        .map(|basis| basis.canonical())
}

pub fn implied_reference_venue_kind(resolution_basis: &str) -> Option<ReferenceVenueKind> {
    let family = parse_resolution_basis(resolution_basis)?.family;
    match family.as_str() {
        "binance" => Some(ReferenceVenueKind::Binance),
        "bybit" => Some(ReferenceVenueKind::Bybit),
        "deribit" => Some(ReferenceVenueKind::Deribit),
        "hyperliquid" => Some(ReferenceVenueKind::Hyperliquid),
        "kraken" => Some(ReferenceVenueKind::Kraken),
        "okx" => Some(ReferenceVenueKind::Okx),
        "polymarket" => Some(ReferenceVenueKind::Polymarket),
        "chainlink" => Some(ReferenceVenueKind::Chainlink),
        _ => None,
    }
}

fn parse_declared_resolution_basis_from_input(input: &str) -> Option<ResolutionBasis> {
    let family = detect_family(input)?;
    let symbol = extract_symbol_pair(input)?;
    Some(ResolutionBasis {
        family: family.to_string(),
        symbol,
        cadence: default_cadence_for_family(family).map(str::to_string),
    })
}

fn detect_family(input: &str) -> Option<&'static str> {
    let normalized = normalize_input(Some(input));
    SUPPORTED_FAMILIES
        .iter()
        .copied()
        .find(|family| normalized.contains(family))
}

fn extract_symbol_pair(input: &str) -> Option<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut candidates = BTreeSet::new();

    for (index, ch) in chars.iter().enumerate() {
        if !matches!(ch, '/' | '_' | '-') {
            continue;
        }

        let Some(left) = left_symbol_token(&chars, index) else {
            continue;
        };
        let Some(right) = right_symbol_token(&chars, index) else {
            continue;
        };

        if !is_symbol_component(&left) || !is_symbol_component(&right) {
            continue;
        }

        candidates.insert(format!(
            "{}{}",
            left.to_ascii_lowercase(),
            right.to_ascii_lowercase()
        ));
    }

    if candidates.len() == 1 {
        candidates.into_iter().next()
    } else {
        None
    }
}

fn left_symbol_token(chars: &[char], separator_index: usize) -> Option<String> {
    if separator_index == 0 {
        return None;
    }

    let mut end = separator_index;
    while end > 0 && chars[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 || !chars[end - 1].is_ascii_alphanumeric() {
        return None;
    }

    let mut start = end;
    while start > 0 && chars[start - 1].is_ascii_alphanumeric() {
        start -= 1;
    }

    Some(chars[start..end].iter().collect())
}

fn right_symbol_token(chars: &[char], separator_index: usize) -> Option<String> {
    let mut start = separator_index + 1;
    while start < chars.len() && chars[start].is_ascii_whitespace() {
        start += 1;
    }
    if start >= chars.len() || !chars[start].is_ascii_alphanumeric() {
        return None;
    }

    let mut end = start;
    while end < chars.len() && chars[end].is_ascii_alphanumeric() {
        end += 1;
    }

    Some(chars[start..end].iter().collect())
}

fn is_symbol_component(token: &str) -> bool {
    let normalized = token.to_ascii_lowercase();
    let len = normalized.len();

    (2..=6).contains(&len)
        && normalized.chars().all(|ch| ch.is_ascii_alphanumeric())
        && !SYMBOL_STOPWORDS.contains(&normalized.as_str())
}

fn default_cadence_for_family(family: &str) -> Option<&'static str> {
    match family {
        "chainlink" => None,
        _ => Some("1m"),
    }
}

fn normalize_basis_part(part: &str) -> Option<String> {
    if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }

    Some(part.to_ascii_lowercase())
}

fn normalize_input(input: Option<&str>) -> String {
    input
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}
