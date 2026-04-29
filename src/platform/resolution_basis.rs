use std::{collections::BTreeSet, fmt};

use crate::config::ReferenceVenueKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolutionSourceKind {
    Binance,
    Bybit,
    Deribit,
    Hyperliquid,
    Kraken,
    Okx,
    Chainlink,
}

impl ResolutionSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
            Self::Deribit => "deribit",
            Self::Hyperliquid => "hyperliquid",
            Self::Kraken => "kraken",
            Self::Okx => "okx",
            Self::Chainlink => "chainlink",
        }
    }

    fn parse_canonical_prefix(input: &str) -> Option<(Self, &str)> {
        for (prefix, source) in [
            ("binance_", Self::Binance),
            ("bybit_", Self::Bybit),
            ("deribit_", Self::Deribit),
            ("hyperliquid_", Self::Hyperliquid),
            ("kraken_", Self::Kraken),
            ("okx_", Self::Okx),
            ("chainlink_", Self::Chainlink),
        ] {
            if let Some(rest) = input.strip_prefix(prefix) {
                return Some((source, rest));
            }
        }

        None
    }

    fn is_exchange_candle(self) -> bool {
        matches!(
            self,
            Self::Binance
                | Self::Bybit
                | Self::Deribit
                | Self::Hyperliquid
                | Self::Kraken
                | Self::Okx
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandleInterval {
    OneMinute,
    FiveMinutes,
    FifteenMinutes,
    OneHour,
}

impl CandleInterval {
    fn as_str(self) -> &'static str {
        match self {
            Self::OneMinute => "1m",
            Self::FiveMinutes => "5m",
            Self::FifteenMinutes => "15m",
            Self::OneHour => "1h",
        }
    }

    fn parse_canonical_suffix(input: &str) -> Result<Option<(&str, Self)>, String> {
        for (suffix, interval) in [
            ("_1m", Self::OneMinute),
            ("_5m", Self::FiveMinutes),
            ("_15m", Self::FifteenMinutes),
            ("_1h", Self::OneHour),
        ] {
            if let Some(pair) = input.strip_suffix(suffix) {
                if !is_canonical_pair(pair) {
                    return Err(format!(
                        "resolution_basis pair must be lowercase ASCII alphanumeric with no underscores, got \"{pair}\""
                    ));
                }
                return Ok(Some((pair, interval)));
            }
        }

        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolutionBasis {
    ExchangeCandle {
        source: ResolutionSourceKind,
        pair: String,
        interval: CandleInterval,
    },
    OraclePriceFeed {
        source: ResolutionSourceKind,
        pair: String,
    },
}

impl fmt::Display for ResolutionBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExchangeCandle {
                source,
                pair,
                interval,
            } => write!(f, "{}_{}_{}", source.as_str(), pair, interval.as_str()),
            Self::OraclePriceFeed { source, pair } => write!(f, "{}_{}", source.as_str(), pair),
        }
    }
}

pub fn parse_ruleset_resolution_basis(input: &str) -> Result<ResolutionBasis, String> {
    let (source, rest) = ResolutionSourceKind::parse_canonical_prefix(input).ok_or_else(|| {
        format!("resolution_basis must start with a canonical source prefix, got \"{input}\"")
    })?;

    if source.is_exchange_candle() {
        let (pair, interval) = CandleInterval::parse_canonical_suffix(rest)?.ok_or_else(|| {
            format!(
                "exchange-candle resolution_basis must end with a canonical interval suffix, got \"{input}\""
            )
        })?;
        return Ok(ResolutionBasis::ExchangeCandle {
            source,
            pair: pair.to_string(),
            interval,
        });
    }

    if !is_canonical_pair(rest) {
        return Err(format!(
            "resolution_basis pair must be lowercase ASCII alphanumeric with no underscores, got \"{rest}\""
        ));
    }

    Ok(ResolutionBasis::OraclePriceFeed {
        source,
        pair: rest.to_string(),
    })
}

pub fn parse_declared_resolution_basis(description: Option<&str>) -> Option<ResolutionBasis> {
    let description = description?;
    let source = extract_unique_source(description)?;
    let pair = extract_pair(description)?;

    if source.is_exchange_candle() {
        return Some(ResolutionBasis::ExchangeCandle {
            source,
            pair,
            interval: extract_interval(description)?,
        });
    }

    Some(ResolutionBasis::OraclePriceFeed { source, pair })
}

pub fn required_reference_venue_kind(basis: &ResolutionBasis) -> Option<ReferenceVenueKind> {
    let source = match basis {
        ResolutionBasis::ExchangeCandle { source, .. } => source,
        ResolutionBasis::OraclePriceFeed { source, .. } => source,
    };

    Some(match source {
        ResolutionSourceKind::Binance => ReferenceVenueKind::Binance,
        ResolutionSourceKind::Bybit => ReferenceVenueKind::Bybit,
        ResolutionSourceKind::Deribit => ReferenceVenueKind::Deribit,
        ResolutionSourceKind::Hyperliquid => ReferenceVenueKind::Hyperliquid,
        ResolutionSourceKind::Kraken => ReferenceVenueKind::Kraken,
        ResolutionSourceKind::Okx => ReferenceVenueKind::Okx,
        ResolutionSourceKind::Chainlink => ReferenceVenueKind::Chainlink,
    })
}

fn extract_unique_source(description: &str) -> Option<ResolutionSourceKind> {
    let normalized = normalize_description(description);
    let mut matches = BTreeSet::new();

    for source in [
        ResolutionSourceKind::Binance,
        ResolutionSourceKind::Bybit,
        ResolutionSourceKind::Deribit,
        ResolutionSourceKind::Hyperliquid,
        ResolutionSourceKind::Kraken,
        ResolutionSourceKind::Okx,
        ResolutionSourceKind::Chainlink,
    ] {
        if normalized.contains(source.as_str()) {
            matches.insert(source);
        }
    }

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn extract_interval(description: &str) -> Option<CandleInterval> {
    let normalized = normalize_description(description);
    let mut matches = BTreeSet::new();

    for (pattern, interval) in [
        ("1minutecandle", CandleInterval::OneMinute),
        ("oneminutecandles", CandleInterval::OneMinute),
        ("with1mandcandlesselected", CandleInterval::OneMinute),
        ("1mcandle", CandleInterval::OneMinute),
        ("5minutecandle", CandleInterval::FiveMinutes),
        ("fiveminutecandles", CandleInterval::FiveMinutes),
        ("with5mandcandlesselected", CandleInterval::FiveMinutes),
        ("5mcandle", CandleInterval::FiveMinutes),
        ("15minutecandle", CandleInterval::FifteenMinutes),
        ("with15mandcandlesselected", CandleInterval::FifteenMinutes),
        ("15mcandle", CandleInterval::FifteenMinutes),
        ("1hourcandle", CandleInterval::OneHour),
        ("hourlycandles", CandleInterval::OneHour),
        ("relevant1hcandle", CandleInterval::OneHour),
        ("1hcandle", CandleInterval::OneHour),
    ] {
        if normalized.contains(pattern) {
            matches.insert(interval);
        }
    }

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn extract_pair(description: &str) -> Option<String> {
    let mut matches = BTreeSet::new();

    for separator in ['/', '_', '-'] {
        matches.extend(extract_separated_pairs(description, separator));
    }

    let tokens = tokenize(description);
    for (index, token) in tokens.iter().enumerate() {
        let normalized_prev = index
            .checked_sub(1)
            .and_then(|prev| tokens.get(prev))
            .map(|value| normalize_description(value));
        let normalized_next = tokens
            .get(index + 1)
            .map(|value| normalize_description(value));

        let Some(candidate) = canonicalize_compact_pair_token(token) else {
            continue;
        };

        let prev_allows = matches!(normalized_prev.as_deref(), Some("for"));
        let next_allows = matches!(
            normalized_next.as_deref(),
            Some("pair")
                | Some("price")
                | Some("prices")
                | Some("candle")
                | Some("candles")
                | Some("data")
                | Some("stream")
        );

        if prev_allows || next_allows {
            matches.insert(candidate);
        }
    }

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn canonicalize_compact_pair_token(token: &str) -> Option<String> {
    let trimmed = token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '/' | '_' | '-'));
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    if trimmed
        .bytes()
        .filter(|byte| byte.is_ascii_uppercase())
        .count()
        < 2
    {
        return None;
    }

    let compact = trimmed.to_ascii_lowercase();

    if compact.len() < 6
        || matches!(
            compact.as_str(),
            "binance"
                | "bybit"
                | "deribit"
                | "hyperliquid"
                | "kraken"
                | "okx"
                | "chainlink"
                | "polymarket"
        )
    {
        return None;
    }

    Some(compact)
}

fn extract_separated_pairs(description: &str, separator: char) -> BTreeSet<String> {
    let mut matches = BTreeSet::new();

    for token in tokenize(description) {
        let trimmed = token
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '/' | '_' | '-'));
        if trimmed.is_empty() || !trimmed.contains(separator) {
            continue;
        }
        if !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == separator)
        {
            continue;
        }

        let mut parts = trimmed.split(separator).filter(|part| !part.is_empty());
        let Some(left) = parts.next() else {
            continue;
        };
        let Some(right) = parts.next() else {
            continue;
        };
        if parts.next().is_some() {
            continue;
        }
        if left.is_empty() || right.is_empty() {
            continue;
        }
        if !contains_ascii_letter(left) || !contains_ascii_letter(right) {
            continue;
        }

        let pair = format!("{left}{right}").to_ascii_lowercase();
        if let Some(pair) = canonicalize_pair_token(&pair) {
            matches.insert(pair);
        }
    }

    matches
}

fn canonicalize_pair_token(token: &str) -> Option<String> {
    if !is_canonical_pair(token) {
        return None;
    }
    Some(token.to_string())
}

fn contains_ascii_letter(value: &str) -> bool {
    value.bytes().any(|byte| byte.is_ascii_alphabetic())
}

fn normalize_description(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn tokenize(input: &str) -> Vec<&str> {
    input.split_whitespace().collect()
}

fn is_canonical_pair(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}
