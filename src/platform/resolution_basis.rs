use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSourceKind {
    Binance,
    Bybit,
    Kraken,
    Chainlink,
}

impl ResolutionSourceKind {
    fn parse_canonical(value: &str) -> Option<Self> {
        match value {
            "binance" => Some(Self::Binance),
            "bybit" => Some(Self::Bybit),
            "kraken" => Some(Self::Kraken),
            "chainlink" => Some(Self::Chainlink),
            _ => None,
        }
    }

    fn from_description(description: &str) -> Option<Self> {
        [Self::Binance, Self::Bybit, Self::Kraken, Self::Chainlink]
            .into_iter()
            .find(|source| description.contains(source.as_str()))
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
            Self::Kraken => "kraken",
            Self::Chainlink => "chainlink",
        }
    }

    fn uses_exchange_candles(self) -> bool {
        !matches!(self, Self::Chainlink)
    }
}

impl Display for ResolutionSourceKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandleInterval {
    OneMinute,
    FiveMinute,
    OneHour,
}

impl CandleInterval {
    fn parse_canonical(value: &str) -> Option<Self> {
        match value {
            "1m" => Some(Self::OneMinute),
            "5m" => Some(Self::FiveMinute),
            "1h" => Some(Self::OneHour),
            _ => None,
        }
    }

    fn from_description(description: &str) -> Option<Self> {
        if contains_any(description, &["one-minute", "1-minute", "1 minute", "1m"]) {
            return Some(Self::OneMinute);
        }
        if contains_any(description, &["five-minute", "5-minute", "5 minute", "5m"]) {
            return Some(Self::FiveMinute);
        }
        if contains_any(description, &["hourly", "1-hour", "1 hour", "1h"]) {
            return Some(Self::OneHour);
        }
        None
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OneMinute => "1m",
            Self::FiveMinute => "5m",
            Self::OneHour => "1h",
        }
    }
}

impl Display for CandleInterval {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionBasis {
    ExchangeCandle {
        source: ResolutionSourceKind,
        pair: String,
        interval: CandleInterval,
    },
    PriceFeed {
        source: ResolutionSourceKind,
        pair: String,
    },
}

impl Display for ResolutionBasis {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExchangeCandle {
                source,
                pair,
                interval,
            } => write!(f, "{source}_{pair}_{interval}"),
            Self::PriceFeed { source, pair } => write!(f, "{source}_{pair}"),
        }
    }
}

pub fn parse_ruleset_resolution_basis(value: &str) -> Option<ResolutionBasis> {
    let mut parts = value.split('_');
    let source = ResolutionSourceKind::parse_canonical(parts.next()?)?;
    let pair = canonicalize_pair(parts.next()?)?;
    let interval = parts.next();

    if parts.next().is_some() {
        return None;
    }

    match (source.uses_exchange_candles(), interval) {
        (true, Some(interval)) => Some(ResolutionBasis::ExchangeCandle {
            source,
            pair,
            interval: CandleInterval::parse_canonical(interval)?,
        }),
        (false, None) => Some(ResolutionBasis::PriceFeed { source, pair }),
        _ => None,
    }
}

pub fn parse_declared_resolution_basis(description: Option<&str>) -> Option<ResolutionBasis> {
    let description = canonicalize_description(description?);
    let source = ResolutionSourceKind::from_description(&description)?;
    let pair = extract_explicit_pair(&description)?;

    if source.uses_exchange_candles() {
        return Some(ResolutionBasis::ExchangeCandle {
            source,
            pair,
            interval: CandleInterval::from_description(&description)?,
        });
    }

    Some(ResolutionBasis::PriceFeed { source, pair })
}

fn canonicalize_description(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn canonicalize_pair(value: &str) -> Option<String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    {
        return None;
    }
    Some(value.to_string())
}

fn extract_explicit_pair(description: &str) -> Option<String> {
    let chars: Vec<char> = description.chars().collect();
    for (index, ch) in chars.iter().enumerate() {
        if *ch != '/' {
            continue;
        }

        let left = collect_adjacent_alnum(&chars, index, Direction::Left);
        let right = collect_adjacent_alnum(&chars, index, Direction::Right);
        if left.is_empty() || right.is_empty() {
            continue;
        }

        let pair = format!("{left}{right}");
        if let Some(pair) = canonicalize_pair(&pair) {
            return Some(pair);
        }
    }

    None
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[derive(Clone, Copy)]
enum Direction {
    Left,
    Right,
}

fn collect_adjacent_alnum(chars: &[char], slash_index: usize, direction: Direction) -> String {
    let mut cursor = slash_index;

    loop {
        match direction {
            Direction::Left => {
                if cursor == 0 {
                    return String::new();
                }
                cursor -= 1;
            }
            Direction::Right => {
                cursor += 1;
                if cursor >= chars.len() {
                    return String::new();
                }
            }
        }

        if chars[cursor].is_ascii_whitespace() {
            continue;
        }
        break;
    }

    let mut collected = String::new();
    loop {
        let ch = chars[cursor];
        if !ch.is_ascii_alphanumeric() {
            break;
        }
        collected.push(ch);

        match direction {
            Direction::Left => {
                if cursor == 0 {
                    break;
                }
                cursor -= 1;
            }
            Direction::Right => {
                cursor += 1;
                if cursor >= chars.len() {
                    break;
                }
            }
        }
    }

    match direction {
        Direction::Left => collected.chars().rev().collect(),
        Direction::Right => collected,
    }
}
