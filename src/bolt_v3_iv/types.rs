use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
};

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
    DerivedInputDiagnostics,
    SourceHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvBasis {
    Mark,
    Bid,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IvConvention {
    Named(String),
}

impl Serialize for IvConvention {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Named(name) => serializer.serialize_str(name),
        }
    }
}

impl<'de> Deserialize<'de> for IvConvention {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(IvConventionVisitor)
    }
}

struct IvConventionVisitor;

impl<'de> Visitor<'de> for IvConventionVisitor {
    type Value = IvConvention;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an IV convention string or a named convention record")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(IvConvention::Named(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(IvConvention::Named(value))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut named = None;

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "named" => {
                    if named.replace(map.next_value::<String>()?).is_some() {
                        return Err(de::Error::duplicate_field("named"));
                    }
                }
                _ => return Err(de::Error::unknown_field(&key, &["named"])),
            }
        }

        named
            .map(IvConvention::Named)
            .ok_or_else(|| de::Error::missing_field("named"))
    }
}
