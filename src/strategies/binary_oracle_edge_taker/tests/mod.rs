#![cfg(test)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_common::{
    cache::Cache,
    clock::TestClock,
    messages::execution::TradingCommand,
    msgbus::{self, MessagingSwitchboard, stubs::get_typed_into_message_saving_handler},
};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    enums::AssetClass,
    identifiers::{Symbol, TraderId},
    instruments::BinaryOption,
    orders::{Order, OrderAny},
    position::Position,
    types::{Currency, Price, Quantity},
};
use nautilus_portfolio::portfolio::Portfolio;
use rust_decimal::Decimal;

use super::*;
// Selection types used only by the test fixtures (the production parent module
// imports the rest via `use self::selection::{…}`). Imported here at test
// scope so the production build does not flag them as unused.
use super::selection::{CandidateOutcome, SelectionDecision};
use crate::strategies::{production_strategy_registry, registry::StrategyBuilder};

mod shared_fixture;

use shared_fixture::*;

mod adverse_path_harness;
mod book_sizing;
mod config;
mod core_glue;
mod exposure;
mod orders_admission;
mod pricing;
mod reference_price;
mod selection;
mod source_evidence;
mod trade_flow;
