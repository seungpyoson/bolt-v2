# Bolt-v3 Dynamic Registration & Interfaces

Status: Trait boundary specification for Bolt-v3 core

## 1. Opaque Configuration Mapping

The TOML configuration needs to pass completely generic structures down to the providers. We flatten out `config.rs`.

```rust
// In src/v3/config.rs

use serde::Deserialize;
use toml::Value;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct V3Config {
    pub venues: HashMap<String, VenueBlock>,
    // strategies...
}

#[derive(Debug, Deserialize)]
pub struct VenueBlock {
    pub kind: String,
    pub data: Option<Value>,
    pub execution: Option<Value>,
    pub secrets: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct StrategyBlock {
    pub strategy_archetype: String,
    pub venue: String,
    pub target: Value,
    pub reference_data: Option<HashMap<String, ReferenceDataBlock>>,
    pub parameters: Value,
}

#[derive(Debug, Deserialize)]
pub struct ReferenceDataBlock {
    pub venue: String,
    pub instrument_identifier: String,
}
```

## 2. Dynamic Traits

```rust
// In src/v3/registry/mod.rs

use anyhow::Result;
use toml::Value;

/// Resolves provider-specific secret maps into SSM dynamically.
pub trait SecretResolver: Send + Sync {
    /// Validates the secret map format and missing fields.
    fn check_config(&self, secrets_config: &Value) -> Result<()>;
    /// Actually fetches from SSM using dynamic lookups.
    fn resolve(&self, secrets_config: &Value) -> Result<Box<dyn std::any::Any>>;
}

/// Binds a specific provider kind into NautilusTrader clients.
pub trait ProviderAdapter: Send + Sync {
    fn build_data_client(&self, data_config: &Value) -> Result<Box<dyn std::any::Any>>; // Returns NT data client
    fn build_exec_client(&self, exec_config: &Value, resolved_secrets: &dyn std::any::Any) -> Result<Box<dyn std::any::Any>>; // Returns NT exec client
}

/// Instantiates a strategy using opaque parameters.
pub trait StrategyArchetype: Send + Sync {
    fn build_strategy(&self, target: &Value, parameters: &Value) -> Result<Box<dyn std::any::Any>>;
}

```

## 3. The Central Registry

Core execution simply loops over the `HashMap<String, VenueBlock>` and looks up the `kind` string in the registry.

```rust
pub struct BoltRegistry {
    adapters: HashMap<String, Box<dyn ProviderAdapter>>,
    secret_resolvers: HashMap<String, Box<dyn SecretResolver>>,
    archetypes: HashMap<String, Box<dyn StrategyArchetype>>,
}
```
If a `kind` does not exist in the map, validation fails with "Unknown venue kind." No `match enum` is ever needed.
