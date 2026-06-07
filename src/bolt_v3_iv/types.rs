use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvSourceKind {
    OptionGreeks,
    OptionChain,
    AggregateGreeks,
    CustomImpliedVolatility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvProductKind {
    IvPoint,
    IvGreeksPoint,
    Smile,
    Surface,
    AggregateGreeks,
    CustomIvEvidence,
    ProjectedScalarIv,
    DerivedIv,
    SourceHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvBasis {
    Mark,
    Bid,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvConvention {
    Named(String),
}
