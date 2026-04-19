use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context;
use arc_swap::{ArcSwap, ArcSwapOption};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_polymarket::http::query::GetGammaEventsParams;
use nautilus_polymarket::{
    common::credential::Secrets as PolymarketSecrets,
    common::enums::SignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{EventParamsFilter, EventSlugFilter, InstrumentFilter},
    http::{
        clob::PolymarketClobHttpClient, gamma::PolymarketGammaRawHttpClient, models::GammaEvent,
    },
};
use serde::Deserialize;
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use toml::Value;

use crate::config::{RulesetConfig, RulesetVenueKind};
use crate::secrets::ResolvedPolymarketSecrets;

pub mod fees;

pub use fees::{FeeProvider, PolymarketClobFeeProvider};

/// Single schema boundary for `data_clients[].config` in both legacy event-slug
/// mode and ruleset mode. Ruleset mode forbids the presence of `event_slugs`
/// separately in `build_data_client`, but typo rejection belongs on the full
/// operator-facing schema rather than on a subset parser.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolymarketDataClientInput {
    #[serde(default)]
    pub subscribe_new_markets: bool,
    #[serde(default = "default_update_instruments_interval_mins")]
    pub update_instruments_interval_mins: u64,
    #[serde(default = "default_gamma_refresh_interval_secs")]
    pub gamma_refresh_interval_secs: u64,
    /// Accepted at the schema boundary for ruleset/runtime wiring. This field is
    /// not consumed by `build_data_client(...)` itself.
    #[serde(default)]
    pub gamma_event_fetch_max_concurrent: Option<usize>,
    #[serde(default = "default_ws_max_subscriptions")]
    pub ws_max_subscriptions: usize,
    #[serde(default)]
    pub event_slugs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct PolymarketRulesetSelector {
    pub tag_slug: String,
    #[serde(default)]
    pub event_slug_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PolymarketPrefixDiscovery {
    pub selector: PolymarketRulesetSelector,
    pub min_time_to_expiry_secs: u64,
    pub max_time_to_expiry_secs: u64,
}

#[derive(Clone, Debug)]
struct SelectorStateSnapshot {
    refreshed_at: DateTime<Utc>,
    event_slugs: Vec<String>,
}

#[derive(Clone, Debug)]
struct CurrentEventSlugsCache {
    as_of: DateTime<Utc>,
    generation: u64,
    event_slugs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectorDiscoveryRead {
    Missing,
    AgedOut,
    EmptyFresh,
    Live(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct PolymarketSelectorState {
    event_slugs_by_discovery:
        Arc<ArcSwap<BTreeMap<PolymarketPrefixDiscovery, SelectorStateSnapshot>>>,
    current_event_slugs_cache: Arc<ArcSwapOption<CurrentEventSlugsCache>>,
    snapshot_generation: Arc<AtomicU64>,
}

impl PolymarketSelectorState {
    /// Construct a selector state preseeded with a map of event slugs for each
    /// prefix discovery. Public entry point for test harnesses that need to
    /// simulate the post-startup state without running a live Gamma fetch.
    pub fn for_testing(
        event_slugs_by_ruleset: Vec<(&RulesetConfig, Vec<String>)>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut seeded = Vec::with_capacity(event_slugs_by_ruleset.len());
        for (ruleset, event_slugs) in event_slugs_by_ruleset {
            let discovery = polymarket_prefix_discovery_for_ruleset(ruleset)?.ok_or_else(|| {
                format!(
                    "ruleset {} must be a polymarket prefix ruleset to seed selector state",
                    ruleset.id
                )
            })?;
            seeded.push((discovery, event_slugs));
        }
        Ok(Self::new(seeded))
    }

    pub(crate) fn new<I>(event_slugs_by_discovery: I) -> Self
    where
        I: IntoIterator<Item = (PolymarketPrefixDiscovery, Vec<String>)>,
    {
        Self::new_at(event_slugs_by_discovery, selector_reference_now())
    }

    fn new_at<I>(event_slugs_by_discovery: I, refreshed_at: DateTime<Utc>) -> Self
    where
        I: IntoIterator<Item = (PolymarketPrefixDiscovery, Vec<String>)>,
    {
        let initial_map: BTreeMap<PolymarketPrefixDiscovery, SelectorStateSnapshot> =
            event_slugs_by_discovery
                .into_iter()
                .map(|(discovery, event_slugs)| {
                    (
                        discovery,
                        SelectorStateSnapshot {
                            refreshed_at,
                            event_slugs: event_slugs
                                .into_iter()
                                .collect::<BTreeSet<_>>()
                                .into_iter()
                                .collect(),
                        },
                    )
                })
                .collect();
        Self {
            event_slugs_by_discovery: Arc::new(ArcSwap::from_pointee(initial_map)),
            current_event_slugs_cache: Arc::new(ArcSwapOption::empty()),
            snapshot_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_at_for_testing<I>(
        event_slugs_by_discovery: I,
        refreshed_at: DateTime<Utc>,
    ) -> Self
    where
        I: IntoIterator<Item = (PolymarketPrefixDiscovery, Vec<String>)>,
    {
        Self::new_at(event_slugs_by_discovery, refreshed_at)
    }

    fn current_event_slugs(&self) -> Vec<String> {
        self.current_event_slugs_at(selector_reference_now())
    }

    fn current_event_slugs_at(&self, now: DateTime<Utc>) -> Vec<String> {
        let now = normalize_selector_now(now);
        let generation = self.snapshot_generation.load(Ordering::Acquire);
        if let Some(cache) = self.current_event_slugs_cache.load_full()
            && cache.as_of == now
            && cache.generation == generation
        {
            return cache.event_slugs.clone();
        }

        let event_slugs: Vec<String> = self
            .event_slugs_by_discovery
            .load()
            .iter()
            .filter(|(discovery, snapshot)| selector_snapshot_is_live(discovery, snapshot, now))
            .flat_map(|(_, snapshot)| snapshot.event_slugs.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        if self.snapshot_generation.load(Ordering::Acquire) == generation {
            self.current_event_slugs_cache
                .store(Some(Arc::new(CurrentEventSlugsCache {
                    as_of: now,
                    generation,
                    event_slugs: event_slugs.clone(),
                })));
        }

        event_slugs
    }

    /// Insert or overwrite the event-slug list for each discovery in the input.
    ///
    /// Per-discovery upsert semantics: every `(discovery, event_slugs)` entry in the
    /// iterator inserts into `event_slugs_by_discovery`, overwriting any prior value for
    /// that discovery. Discoveries NOT present in the input iterator are left untouched.
    ///
    /// Empty `event_slugs` for a given discovery overwrites with an empty list and emits
    /// a warn log naming the discovery (see commit `a79bbc5` and #180 for the empty-response
    /// semantics trade-off).
    ///
    /// Single-writer contract: the only live caller is the selector refresh task, so the
    /// load → clone → mutate → store sequence does not race against itself. Concurrent
    /// readers see either the old snapshot fully or the new snapshot fully — never a
    /// partially updated map.
    fn upsert_event_slugs_by_discovery<I>(&self, event_slugs_by_discovery: I)
    where
        I: IntoIterator<Item = (PolymarketPrefixDiscovery, Vec<String>)>,
    {
        self.upsert_event_slugs_by_discovery_at(event_slugs_by_discovery, selector_reference_now());
    }

    fn upsert_event_slugs_by_discovery_at<I>(
        &self,
        event_slugs_by_discovery: I,
        refreshed_at: DateTime<Utc>,
    ) where
        I: IntoIterator<Item = (PolymarketPrefixDiscovery, Vec<String>)>,
    {
        let mut next = (**self.event_slugs_by_discovery.load()).clone();

        for (discovery, event_slugs) in event_slugs_by_discovery {
            let unique_event_slugs: Vec<String> = event_slugs
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if unique_event_slugs.is_empty() {
                log::warn!(
                    "selector refresh returned no event slugs for tag_slug={} prefix={:?}; clearing previous selector state",
                    discovery.selector.tag_slug,
                    discovery.selector.event_slug_prefix.as_deref()
                );
            }
            next.insert(
                discovery,
                SelectorStateSnapshot {
                    refreshed_at,
                    event_slugs: unique_event_slugs,
                },
            );
        }

        self.event_slugs_by_discovery.store(Arc::new(next));
        self.current_event_slugs_cache.store(None);
        self.snapshot_generation.fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(crate) fn event_slugs_for_discovery(
        &self,
        discovery: &PolymarketPrefixDiscovery,
    ) -> Vec<String> {
        self.event_slugs_for_discovery_at(discovery, selector_reference_now())
    }

    pub(crate) fn event_slugs_read_for_discovery(
        &self,
        discovery: &PolymarketPrefixDiscovery,
    ) -> SelectorDiscoveryRead {
        self.event_slugs_read_for_discovery_at(discovery, selector_reference_now())
    }

    fn event_slugs_read_for_discovery_at(
        &self,
        discovery: &PolymarketPrefixDiscovery,
        now: DateTime<Utc>,
    ) -> SelectorDiscoveryRead {
        let snapshots = self.event_slugs_by_discovery.load();
        let Some(snapshot) = snapshots.get(discovery) else {
            return SelectorDiscoveryRead::Missing;
        };
        if !selector_snapshot_is_live(discovery, snapshot, now) {
            return SelectorDiscoveryRead::AgedOut;
        }
        if snapshot.event_slugs.is_empty() {
            return SelectorDiscoveryRead::EmptyFresh;
        }
        SelectorDiscoveryRead::Live(snapshot.event_slugs.clone())
    }

    #[cfg(test)]
    fn event_slugs_for_discovery_at(
        &self,
        discovery: &PolymarketPrefixDiscovery,
        now: DateTime<Utc>,
    ) -> Vec<String> {
        match self.event_slugs_read_for_discovery_at(discovery, now) {
            SelectorDiscoveryRead::Live(event_slugs) => event_slugs,
            SelectorDiscoveryRead::Missing
            | SelectorDiscoveryRead::AgedOut
            | SelectorDiscoveryRead::EmptyFresh => Vec::new(),
        }
    }
}

fn selector_reference_now() -> DateTime<Utc> {
    normalize_selector_now(Utc::now())
}

fn normalize_selector_now(now: DateTime<Utc>) -> DateTime<Utc> {
    now.with_nanosecond(0)
        .expect("zero nanoseconds should always be valid")
}

fn selector_snapshot_is_live(
    discovery: &PolymarketPrefixDiscovery,
    snapshot: &SelectorStateSnapshot,
    now: DateTime<Utc>,
) -> bool {
    let Ok(max_offset_secs) = i64::try_from(discovery.max_time_to_expiry_secs) else {
        return false;
    };
    let Some(max_offset) = ChronoDuration::try_seconds(max_offset_secs) else {
        return false;
    };
    let Some(expires_at) = snapshot.refreshed_at.checked_add_signed(max_offset) else {
        return false;
    };
    now <= expires_at
}

#[derive(Debug)]
pub struct PolymarketSelectorRefreshGuard {
    cancellation: CancellationToken,
    join_handle: JoinHandle<()>,
}

#[derive(Clone, Debug)]
pub struct PolymarketRulesetSetup {
    selectors: Vec<PolymarketRulesetSelector>,
    prefix_discoveries: Vec<PolymarketPrefixDiscovery>,
    selector_state: Option<PolymarketSelectorState>,
}

fn default_update_instruments_interval_mins() -> u64 {
    60
}

fn default_gamma_refresh_interval_secs() -> u64 {
    60
}

fn default_ws_max_subscriptions() -> usize {
    200
}

#[derive(Debug, Deserialize)]
pub struct PolymarketExecClientInput {
    pub account_id: String,
    pub signature_type: u8,
    pub funder: String,
}

fn map_signature_type(value: u8) -> Result<SignatureType, Box<dyn std::error::Error>> {
    match value {
        0 => Ok(SignatureType::Eoa),
        1 => Ok(SignatureType::PolyProxy),
        2 => Ok(SignatureType::PolyGnosisSafe),
        other => Err(format!("Unknown Polymarket signature_type: {other}").into()),
    }
}

pub(crate) fn build_data_client(
    raw: &Value,
    selectors: &[PolymarketRulesetSelector],
    selector_state: Option<PolymarketSelectorState>,
) -> Result<
    (
        Box<PolymarketDataClientFactory>,
        Box<PolymarketDataClientConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    if !selectors.is_empty() && raw.get("event_slugs").is_some() {
        return Err(
            "data_clients[].config.event_slugs must be omitted when rulesets are enabled".into(),
        );
    }

    let PolymarketDataClientInput {
        subscribe_new_markets,
        update_instruments_interval_mins,
        gamma_refresh_interval_secs: _,
        gamma_event_fetch_max_concurrent: _,
        ws_max_subscriptions,
        event_slugs,
    } = raw.clone().try_into()?;

    reject_mixed_polymarket_rulesets_global_scope(selectors)?;

    let has_prefix_selectors = selectors
        .iter()
        .any(|selector| selector.event_slug_prefix.is_some());

    let filters: Vec<Arc<dyn InstrumentFilter>> = if selectors.is_empty() {
        if event_slugs.is_empty() {
            return Err(
                "data_clients[].config.event_slugs must contain at least one slug when rulesets are disabled"
                    .into(),
            );
        }

        vec![Arc::new(EventSlugFilter::from_slugs(event_slugs))]
    } else {
        if has_prefix_selectors {
            let selector_state = selector_state.ok_or_else(|| {
                std::io::Error::other(
                    "polymarket ruleset validation: prefix selectors require selector_state; build the data client through PolymarketRulesetSetup::from_rulesets",
                )
            })?;
            vec![Arc::new(EventSlugFilter::new(move || {
                selector_state.current_event_slugs()
            }))]
        } else {
            selectors
                .iter()
                .map(|selector| {
                    Arc::new(EventParamsFilter::new(GetGammaEventsParams {
                        tag_slug: Some(selector.tag_slug.clone()),
                        ..Default::default()
                    })) as Arc<dyn InstrumentFilter>
                })
                .collect()
        }
    };

    let config = PolymarketDataClientConfig {
        subscribe_new_markets: subscribe_new_markets && !has_prefix_selectors,
        update_instruments_interval_mins,
        ws_max_subscriptions,
        filters,
        new_market_filter: None,
        ..Default::default()
    };

    Ok((Box::new(PolymarketDataClientFactory), Box::new(config)))
}

pub fn polymarket_ruleset_tag_slugs(
    rulesets: &[RulesetConfig],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let selectors = polymarket_ruleset_selectors(rulesets)?;
    let mut tag_slugs = Vec::new();
    for selector in selectors {
        if !tag_slugs.contains(&selector.tag_slug) {
            tag_slugs.push(selector.tag_slug);
        }
    }

    Ok(tag_slugs)
}

fn polymarket_ruleset_selector(
    ruleset: &RulesetConfig,
) -> Result<PolymarketRulesetSelector, Box<dyn std::error::Error>> {
    let selector: PolymarketRulesetSelector =
        ruleset.selector.clone().try_into().map_err(|error| {
            format!(
                "failed to parse polymarket selector for ruleset {}: {error}",
                ruleset.id
            )
        })?;

    if selector.tag_slug.contains(char::is_whitespace) {
        return Err(format!(
            "polymarket selector tag_slug for ruleset {} must not contain whitespace, got {:?}",
            ruleset.id, selector.tag_slug
        )
        .into());
    }

    Ok(selector)
}

pub fn polymarket_ruleset_selectors(
    rulesets: &[RulesetConfig],
) -> Result<Vec<PolymarketRulesetSelector>, Box<dyn std::error::Error>> {
    let mut selectors = Vec::new();
    for ruleset in rulesets {
        if ruleset.venue != RulesetVenueKind::Polymarket {
            continue;
        }

        selectors.push(polymarket_ruleset_selector(ruleset)?);
    }

    Ok(selectors)
}

/// Enforce that ALL Polymarket rulesets uniformly use either tag-only selectors
/// (event_slug_prefix = None) or prefix-based selectors (event_slug_prefix = Some(_)).
///
/// All Polymarket rulesets feed a single shared PolymarketRulesetSetup that is reused
/// across every Polymarket data_client in the runtime. The same flattened selector
/// list is applied to all polymarket data_clients, so this invariant operates at the
/// ruleset (global) level, not per data_client.
///
/// Mixing is rejected fail-closed before any selector state is built: the NT adapter
/// combines `InstrumentFilter`s additively on fetch, so a tag-only selector sharing a
/// data_client with a prefix selector would fetch the entire tag regardless of the
/// sibling prefix selector's narrowing intent, producing end-to-end over-admission.
pub(crate) fn reject_mixed_polymarket_rulesets_global_scope(
    selectors: &[PolymarketRulesetSelector],
) -> Result<(), Box<dyn std::error::Error>> {
    let tag_only: Vec<&PolymarketRulesetSelector> = selectors
        .iter()
        .filter(|selector| selector.event_slug_prefix.is_none())
        .collect();
    let prefix_based: Vec<&PolymarketRulesetSelector> = selectors
        .iter()
        .filter(|selector| selector.event_slug_prefix.is_some())
        .collect();

    if !tag_only.is_empty() && !prefix_based.is_empty() {
        let tag_only_tags: Vec<&str> = tag_only.iter().map(|s| s.tag_slug.as_str()).collect();
        let prefix_pairs: Vec<String> = prefix_based
            .iter()
            .map(|s| {
                format!(
                    "{}:{}",
                    s.tag_slug,
                    s.event_slug_prefix.as_deref().unwrap_or("")
                )
            })
            .collect();
        return Err(format!(
            "all Polymarket rulesets must be uniformly tag-only or uniformly prefix-based; found {} tag-only ruleset selector(s) [{}] mixed with {} prefix-based ruleset selector(s) [{}]",
            tag_only.len(),
            tag_only_tags.join(", "),
            prefix_based.len(),
            prefix_pairs.join(", "),
        )
        .into());
    }

    Ok(())
}

pub(crate) fn polymarket_ruleset_prefix_discoveries(
    rulesets: &[RulesetConfig],
) -> Result<Vec<PolymarketPrefixDiscovery>, Box<dyn std::error::Error>> {
    let mut discoveries = Vec::new();
    for ruleset in rulesets {
        if ruleset.venue != RulesetVenueKind::Polymarket {
            continue;
        }

        let selector = polymarket_ruleset_selector(ruleset)?;
        if selector.event_slug_prefix.is_none() {
            continue;
        }

        discoveries.push(PolymarketPrefixDiscovery {
            selector,
            min_time_to_expiry_secs: ruleset.min_time_to_expiry_secs,
            max_time_to_expiry_secs: ruleset.max_time_to_expiry_secs,
        });
    }

    Ok(discoveries)
}

pub(crate) fn polymarket_prefix_discovery_for_ruleset(
    ruleset: &RulesetConfig,
) -> Result<Option<PolymarketPrefixDiscovery>, Box<dyn std::error::Error>> {
    if ruleset.venue != RulesetVenueKind::Polymarket {
        return Ok(None);
    }

    let selector = polymarket_ruleset_selector(ruleset)?;
    if selector.event_slug_prefix.is_none() {
        return Ok(None);
    }

    Ok(Some(PolymarketPrefixDiscovery {
        selector,
        min_time_to_expiry_secs: ruleset.min_time_to_expiry_secs,
        max_time_to_expiry_secs: ruleset.max_time_to_expiry_secs,
    }))
}

pub fn gamma_refresh_interval_secs(raw: &Value) -> Result<u64, Box<dyn std::error::Error>> {
    let input: PolymarketDataClientInput = raw.clone().try_into()?;
    Ok(input.gamma_refresh_interval_secs)
}

pub(crate) fn build_selector_state(
    prefix_discoveries: &[PolymarketPrefixDiscovery],
    timeout_secs: u64,
) -> Result<Option<PolymarketSelectorState>, Box<dyn std::error::Error>> {
    if prefix_discoveries.is_empty() {
        return Ok(None);
    }

    let event_slugs_by_discovery =
        resolve_event_slugs_for_prefix_discoveries(prefix_discoveries, timeout_secs)?;
    Ok(Some(PolymarketSelectorState::new(event_slugs_by_discovery)))
}

pub(crate) fn spawn_selector_refresh_task(
    selector_state: PolymarketSelectorState,
    prefix_discoveries: Vec<PolymarketPrefixDiscovery>,
    interval_secs: u64,
    timeout_secs: u64,
) -> Result<PolymarketSelectorRefreshGuard, Box<dyn std::error::Error>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client for selector refresh")?;
    Ok(spawn_selector_refresh_task_with_client(
        selector_state,
        prefix_discoveries,
        interval_secs,
        raw_client,
    ))
}

fn spawn_selector_refresh_task_with_client(
    selector_state: PolymarketSelectorState,
    prefix_discoveries: Vec<PolymarketPrefixDiscovery>,
    interval_secs: u64,
    raw_client: PolymarketGammaRawHttpClient,
) -> PolymarketSelectorRefreshGuard {
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let join_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = task_cancellation.cancelled() => return,
                _ = ticker.tick() => {}
            }

            let fetch_result = tokio::select! {
                _ = task_cancellation.cancelled() => return,
                result = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_best_effort(
                    &prefix_discoveries,
                    &raw_client,
                ) => result,
            };
            match fetch_result {
                Ok(event_slugs) => selector_state.upsert_event_slugs_by_discovery(event_slugs),
                Err(error) => {
                    log::warn!(
                        "polymarket ruleset validation: failed refreshing polymarket selector event slugs: {error}"
                    );
                }
            }
        }
    });

    PolymarketSelectorRefreshGuard {
        cancellation,
        join_handle,
    }
}

impl PolymarketSelectorRefreshGuard {
    pub async fn shutdown(self) {
        self.cancellation.cancel();
        let _ = self.join_handle.await;
    }
}

impl PolymarketRulesetSetup {
    pub fn from_rulesets(
        rulesets: &[RulesetConfig],
        timeout_secs: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let selectors = polymarket_ruleset_selectors(rulesets)?;
        reject_mixed_polymarket_rulesets_global_scope(&selectors)?;
        let prefix_discoveries = polymarket_ruleset_prefix_discoveries(rulesets)?;
        let selector_state = build_selector_state(&prefix_discoveries, timeout_secs)?;
        Ok(Self {
            selectors,
            prefix_discoveries,
            selector_state,
        })
    }

    pub fn build_data_client(
        &self,
        raw: &Value,
    ) -> Result<
        (
            Box<PolymarketDataClientFactory>,
            Box<PolymarketDataClientConfig>,
        ),
        Box<dyn std::error::Error>,
    > {
        ensure_refresh_interval_fits_prefix_discoveries(
            &self.prefix_discoveries,
            gamma_refresh_interval_secs(raw)?,
        )?;
        build_data_client(raw, &self.selectors, self.selector_state.clone())
    }

    pub fn selector_state(&self) -> Option<PolymarketSelectorState> {
        self.selector_state.clone()
    }

    pub fn resolved_prefix_event_slugs(&self) -> Vec<String> {
        self.selector_state
            .as_ref()
            .map(PolymarketSelectorState::current_event_slugs)
            .unwrap_or_default()
    }

    pub fn spawn_selector_refresh_task_if_configured(
        &self,
        raw: &Value,
        timeout_secs: u64,
    ) -> Result<Option<PolymarketSelectorRefreshGuard>, Box<dyn std::error::Error>> {
        let Some(selector_state) = self.selector_state.clone() else {
            return Ok(None);
        };
        let refresh_interval_secs = gamma_refresh_interval_secs(raw)?;
        ensure_refresh_interval_fits_prefix_discoveries(
            &self.prefix_discoveries,
            refresh_interval_secs,
        )?;

        spawn_selector_refresh_task(
            selector_state,
            self.prefix_discoveries.clone(),
            refresh_interval_secs,
            timeout_secs,
        )
        .map(Some)
    }
}

fn ensure_refresh_interval_fits_prefix_discoveries(
    prefix_discoveries: &[PolymarketPrefixDiscovery],
    refresh_interval_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(min_max_expiry_secs) = prefix_discoveries
        .iter()
        .map(|discovery| discovery.max_time_to_expiry_secs)
        .min()
    else {
        return Ok(());
    };

    if refresh_interval_secs == 0 {
        return Err(
            "data_clients[].config.gamma_refresh_interval_secs must be > 0 for prefix-selector refresh"
                .into(),
        );
    }

    if refresh_interval_secs >= min_max_expiry_secs {
        return Err(format!(
            "data_clients[].config.gamma_refresh_interval_secs ({refresh_interval_secs}) must be < the smallest prefix ruleset max_time_to_expiry_secs ({min_max_expiry_secs}) so selector snapshots do not age out between refreshes"
        )
        .into());
    }

    Ok(())
}

fn gamma_selector_datetime_after_seconds(
    now: DateTime<Utc>,
    offset_secs: u64,
) -> anyhow::Result<String> {
    let now = normalize_selector_now(now);
    let offset_secs = i64::try_from(offset_secs)
        .map_err(|_| anyhow::anyhow!("selector expiry bound {offset_secs} exceeds i64::MAX"))?;
    let offset = ChronoDuration::try_seconds(offset_secs).ok_or_else(|| {
        anyhow::anyhow!("selector expiry bound {offset_secs} exceeds chrono duration range")
    })?;
    Ok(now
        .checked_add_signed(offset)
        .ok_or_else(|| {
            anyhow::anyhow!("selector expiry bound {offset_secs} results in datetime overflow")
        })?
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string())
}

pub(crate) fn gamma_event_params_for_prefix_discovery(
    discovery: &PolymarketPrefixDiscovery,
    now: DateTime<Utc>,
) -> anyhow::Result<GetGammaEventsParams> {
    Ok(GetGammaEventsParams {
        tag_slug: Some(discovery.selector.tag_slug.clone()),
        end_date_min: Some(gamma_selector_datetime_after_seconds(
            now,
            discovery.min_time_to_expiry_secs,
        )?),
        end_date_max: Some(gamma_selector_datetime_after_seconds(
            now,
            discovery.max_time_to_expiry_secs,
        )?),
        ..Default::default()
    })
}

pub(crate) fn resolve_event_slugs_for_prefix_discoveries(
    discoveries: &[PolymarketPrefixDiscovery],
    timeout_secs: u64,
) -> Result<BTreeMap<PolymarketPrefixDiscovery, Vec<String>>, Box<dyn std::error::Error>> {
    let raw_client = PolymarketGammaRawHttpClient::new(None, timeout_secs)
        .context("failed to build gamma raw client")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(
        resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            discoveries,
            &raw_client,
        ),
    )
}

struct CanonicalQueryGroup {
    canonical_params: GetGammaEventsParams,
    canonical_min_secs: u64,
    canonical_max_secs: u64,
    member_discoveries: Vec<PolymarketPrefixDiscovery>,
}

fn group_discoveries_by_canonical_query(
    discoveries: &[PolymarketPrefixDiscovery],
    now: DateTime<Utc>,
) -> Result<Vec<CanonicalQueryGroup>, Box<dyn std::error::Error>> {
    let mut by_tag: BTreeMap<String, Vec<PolymarketPrefixDiscovery>> = BTreeMap::new();
    for discovery in discoveries {
        by_tag
            .entry(discovery.selector.tag_slug.clone())
            .or_default()
            .push(discovery.clone());
    }

    let mut groups = Vec::new();
    for (_, mut tag_discoveries) in by_tag {
        tag_discoveries.sort_by(|a, b| {
            a.min_time_to_expiry_secs
                .cmp(&b.min_time_to_expiry_secs)
                .then(a.max_time_to_expiry_secs.cmp(&b.max_time_to_expiry_secs))
                .then(a.selector.cmp(&b.selector))
        });

        let mut iter = tag_discoveries.into_iter();
        let Some(first) = iter.next() else {
            continue;
        };
        let mut current_min = first.min_time_to_expiry_secs;
        let mut current_max = first.max_time_to_expiry_secs;
        let mut current_members = vec![first];

        for discovery in iter {
            if discovery.min_time_to_expiry_secs <= current_max {
                current_max = current_max.max(discovery.max_time_to_expiry_secs);
                current_members.push(discovery);
            } else {
                let canonical_discovery = PolymarketPrefixDiscovery {
                    selector: PolymarketRulesetSelector {
                        tag_slug: current_members[0].selector.tag_slug.clone(),
                        event_slug_prefix: None,
                    },
                    min_time_to_expiry_secs: current_min,
                    max_time_to_expiry_secs: current_max,
                };
                groups.push(CanonicalQueryGroup {
                    canonical_params: gamma_event_params_for_prefix_discovery(
                        &canonical_discovery,
                        now,
                    )?,
                    canonical_min_secs: current_min,
                    canonical_max_secs: current_max,
                    member_discoveries: std::mem::take(&mut current_members),
                });

                current_min = discovery.min_time_to_expiry_secs;
                current_max = discovery.max_time_to_expiry_secs;
                current_members.push(discovery);
            }
        }

        let canonical_discovery = PolymarketPrefixDiscovery {
            selector: PolymarketRulesetSelector {
                tag_slug: current_members[0].selector.tag_slug.clone(),
                event_slug_prefix: None,
            },
            min_time_to_expiry_secs: current_min,
            max_time_to_expiry_secs: current_max,
        };
        groups.push(CanonicalQueryGroup {
            canonical_params: gamma_event_params_for_prefix_discovery(&canonical_discovery, now)?,
            canonical_min_secs: current_min,
            canonical_max_secs: current_max,
            member_discoveries: current_members,
        });
    }

    Ok(groups)
}

fn matches_time_bounds(
    event: &GammaEvent,
    discovery: &PolymarketPrefixDiscovery,
    now: DateTime<Utc>,
) -> bool {
    let now = normalize_selector_now(now);
    let Some(end_date) = event.end_date.as_deref() else {
        return false;
    };
    let Ok(end_at) = DateTime::parse_from_rfc3339(end_date) else {
        return false;
    };
    let end_at = end_at.with_timezone(&Utc);
    let Ok(min_offset_secs) = i64::try_from(discovery.min_time_to_expiry_secs) else {
        return false;
    };
    let Ok(max_offset_secs) = i64::try_from(discovery.max_time_to_expiry_secs) else {
        return false;
    };
    let Some(min_offset) = ChronoDuration::try_seconds(min_offset_secs) else {
        return false;
    };
    let Some(max_offset) = ChronoDuration::try_seconds(max_offset_secs) else {
        return false;
    };
    let Some(min_end) = now.checked_add_signed(min_offset) else {
        return false;
    };
    let Some(max_end) = now.checked_add_signed(max_offset) else {
        return false;
    };
    end_at >= min_end && end_at <= max_end
}

fn discovery_matches_event(
    event: &GammaEvent,
    discovery: &PolymarketPrefixDiscovery,
    canonical_bounds: (u64, u64),
    now: DateTime<Utc>,
) -> bool {
    let Some(event_slug) = event.slug.as_deref() else {
        return false;
    };
    let prefix_match = discovery
        .selector
        .event_slug_prefix
        .as_deref()
        .is_some_and(|prefix| event_slug.starts_with(prefix));
    if !prefix_match {
        return false;
    }
    let discovery_matches_canonical = discovery.min_time_to_expiry_secs == canonical_bounds.0
        && discovery.max_time_to_expiry_secs == canonical_bounds.1;
    discovery_matches_canonical || matches_time_bounds(event, discovery, now)
}

fn partition_events_by_discovery(
    out: &mut BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>>,
    canonical_bounds: (u64, u64),
    discoveries: &[PolymarketPrefixDiscovery],
    events: Vec<GammaEvent>,
    now: DateTime<Utc>,
) {
    for event in events {
        for discovery in discoveries {
            if !discovery_matches_event(&event, discovery, canonical_bounds, now) {
                continue;
            }
            out.get_mut(discovery)
                .expect("discovery bucket should already exist")
                .push(event.clone());
        }
    }
}

async fn resolve_matching_events_by_discovery_with_gamma_client(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>>, Box<dyn std::error::Error>> {
    let now = selector_reference_now();
    let unique_discoveries: Vec<PolymarketPrefixDiscovery> = discoveries
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let groups = group_discoveries_by_canonical_query(&unique_discoveries, now)?;

    let mut out: BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>> = unique_discoveries
        .iter()
        .cloned()
        .map(|d| (d, Vec::new()))
        .collect();

    for group in groups {
        let CanonicalQueryGroup {
            canonical_params,
            canonical_min_secs,
            canonical_max_secs,
            member_discoveries,
        } = group;
        let events = fetch_gamma_events_paginated(client, canonical_params).await?;
        partition_events_by_discovery(
            &mut out,
            (canonical_min_secs, canonical_max_secs),
            &member_discoveries,
            events,
            now,
        );
    }

    Ok(out)
}

/// Keep the named strict entrypoint explicit: startup seeding is fail-closed,
/// even though it currently shares the same core resolver as the strict helper.
async fn resolve_matching_events_by_discovery_with_gamma_client_strict(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>>, Box<dyn std::error::Error>> {
    resolve_matching_events_by_discovery_with_gamma_client(discoveries, client).await
}

/// Best-effort variant: per-group Gamma errors are logged as warnings and the
/// affected group is omitted from the returned map.
///
/// Used by the selector refresh task so that one tag/time-window outage cannot
/// stale every other group's selector state at once. Because
/// `PolymarketSelectorState::upsert_event_slugs_by_discovery` leaves absent keys
/// untouched, omitted groups keep their prior snapshot until their next refresh
/// or until that snapshot ages out past the discovery max-expiry window.
async fn resolve_matching_events_by_discovery_with_gamma_client_best_effort(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>>, Box<dyn std::error::Error>> {
    let now = selector_reference_now();
    let unique_discoveries: Vec<PolymarketPrefixDiscovery> = discoveries
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let groups = group_discoveries_by_canonical_query(&unique_discoveries, now)?;

    let mut matched_events_by_discovery: BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>> =
        BTreeMap::new();

    for group in groups {
        let CanonicalQueryGroup {
            canonical_params,
            canonical_min_secs,
            canonical_max_secs,
            member_discoveries,
        } = group;
        let tag_slug = canonical_params.tag_slug.clone().unwrap_or_default();
        match fetch_gamma_events_paginated(client, canonical_params).await {
            Ok(events) => {
                for discovery in &member_discoveries {
                    matched_events_by_discovery
                        .entry(discovery.clone())
                        .or_default();
                }
                partition_events_by_discovery(
                    &mut matched_events_by_discovery,
                    (canonical_min_secs, canonical_max_secs),
                    &member_discoveries,
                    events,
                    now,
                );
            }
            Err(error) => {
                log::warn!(
                    "polymarket ruleset validation: group tag_slug={tag_slug} gamma fetch failed: {error}; preserving prior selector state for that group until refresh succeeds or the snapshot ages out"
                );
            }
        }
    }

    Ok(matched_events_by_discovery)
}

pub(crate) async fn resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<BTreeMap<PolymarketPrefixDiscovery, Vec<String>>, Box<dyn std::error::Error>> {
    let matched_events_by_discovery =
        resolve_matching_events_by_discovery_with_gamma_client_strict(discoveries, client).await?;
    Ok(events_by_discovery_to_event_slugs(
        matched_events_by_discovery,
    ))
}

pub(crate) async fn resolve_event_slugs_for_prefix_discoveries_with_gamma_client_best_effort(
    discoveries: &[PolymarketPrefixDiscovery],
    client: &PolymarketGammaRawHttpClient,
) -> Result<BTreeMap<PolymarketPrefixDiscovery, Vec<String>>, Box<dyn std::error::Error>> {
    let matched_events_by_discovery =
        resolve_matching_events_by_discovery_with_gamma_client_best_effort(discoveries, client)
            .await?;
    Ok(events_by_discovery_to_event_slugs(
        matched_events_by_discovery,
    ))
}

fn events_by_discovery_to_event_slugs(
    matched_events_by_discovery: BTreeMap<PolymarketPrefixDiscovery, Vec<GammaEvent>>,
) -> BTreeMap<PolymarketPrefixDiscovery, Vec<String>> {
    matched_events_by_discovery
        .into_iter()
        .map(|(discovery, events)| {
            let event_slugs = events
                .into_iter()
                .filter_map(|event| event.slug)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            (discovery, event_slugs)
        })
        .collect()
}

pub(crate) async fn fetch_gamma_events_paginated(
    client: &PolymarketGammaRawHttpClient,
    base_params: GetGammaEventsParams,
) -> Result<Vec<GammaEvent>, Box<dyn std::error::Error>> {
    const PAGE_LIMIT: u32 = 100;

    let page_size = base_params.limit.unwrap_or(PAGE_LIMIT);
    let mut all_events = Vec::new();
    let mut offset = base_params.offset.unwrap_or(0);

    loop {
        let page = client
            .get_gamma_events(GetGammaEventsParams {
                limit: Some(page_size),
                offset: Some(offset),
                ..base_params.clone()
            })
            .await?;
        let page_len = page.len() as u32;
        all_events.extend(page);

        if page_len < page_size {
            break;
        }

        offset += page_size;
    }

    Ok(all_events)
}

pub fn build_exec_client(
    raw: &Value,
    trader_id: TraderId,
    secrets: ResolvedPolymarketSecrets,
) -> Result<
    (
        Box<PolymarketExecutionClientFactory>,
        Box<PolymarketExecClientConfig>,
    ),
    Box<dyn std::error::Error>,
> {
    let input: PolymarketExecClientInput = raw.clone().try_into()?;

    let config = PolymarketExecClientConfig {
        trader_id,
        account_id: AccountId::from(input.account_id.as_str()),
        private_key: Some(secrets.private_key),
        api_key: Some(secrets.api_key),
        api_secret: Some(secrets.api_secret),
        passphrase: Some(secrets.passphrase),
        funder: Some(input.funder),
        signature_type: map_signature_type(input.signature_type)?,
        ..Default::default()
    };

    Ok((Box::new(PolymarketExecutionClientFactory), Box::new(config)))
}

pub fn build_fee_provider(
    raw: &Value,
    secrets: &ResolvedPolymarketSecrets,
    timeout_secs: u64,
) -> Result<Arc<dyn FeeProvider>, Box<dyn std::error::Error>> {
    let input: PolymarketExecClientInput = raw.clone().try_into()?;
    let secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.clone()),
        Some(secrets.api_secret.clone()),
        Some(secrets.passphrase.clone()),
        Some(input.funder),
    )
    .map_err(|error| format!("failed to resolve Polymarket fee credentials: {error}"))?;
    let client =
        PolymarketClobHttpClient::new(secrets.credential, secrets.address, None, timeout_secs)
            .map_err(|error| format!("failed to create Polymarket fee HTTP client: {error}"))?;

    Ok(Arc::new(PolymarketClobFeeProvider::new(client)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::HashMap,
        net::SocketAddr,
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn parse_request_target(request: &str) -> (&str, HashMap<String, String>) {
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("request line should include path");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let params = query
            .split('&')
            .filter(|part| !part.is_empty())
            .filter_map(|part| {
                let (key, value) = part.split_once('=')?;
                Some((key.to_string(), value.to_string()))
            })
            .collect();
        (path, params)
    }

    fn decode_query_param(value: &str) -> String {
        value.replace("%3A", ":").replace("%3a", ":")
    }

    fn prefix_discovery(
        tag_slug: &str,
        event_slug_prefix: &str,
        min_time_to_expiry_secs: u64,
        max_time_to_expiry_secs: u64,
    ) -> PolymarketPrefixDiscovery {
        PolymarketPrefixDiscovery {
            selector: PolymarketRulesetSelector {
                tag_slug: tag_slug.to_string(),
                event_slug_prefix: Some(event_slug_prefix.to_string()),
            },
            min_time_to_expiry_secs,
            max_time_to_expiry_secs,
        }
    }

    async fn spawn_test_server() -> (SocketAddr, Arc<Mutex<Vec<HashMap<String, String>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let server_requests = Arc::clone(&server_requests);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (path, params) = parse_request_target(&request);
                    server_requests.lock().unwrap().push(params.clone());

                    let body = if path == "/events"
                        && params.get("tag_slug").map(String::as_str) == Some("bitcoin")
                    {
                        let canonical_min = DateTime::parse_from_rfc3339(&decode_query_param(
                            params
                                .get("end_date_min")
                                .expect("prefix discovery should send end_date_min"),
                        ))
                        .unwrap()
                        .with_timezone(&Utc);
                        let inside_range = canonical_min + ChronoDuration::seconds(120);
                        json!([
                            {
                                "id":"1",
                                "slug":"bitcoin-5m-alpha",
                                "endDate": inside_range.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            },
                            {
                                "id":"2",
                                "slug":"bitcoin-15m-beta",
                                "endDate": inside_range.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            },
                            {
                                "id":"3",
                                "slug":"ethereum-5m-gamma",
                                "endDate": inside_range.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            }
                        ])
                        .to_string()
                    } else {
                        "[]".to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, requests)
    }

    async fn spawn_recording_empty_server() -> (SocketAddr, Arc<Mutex<Vec<HashMap<String, String>>>>)
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let server_requests = Arc::clone(&server_requests);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (_, params) = parse_request_target(&request);
                    server_requests.lock().unwrap().push(params);

                    let body = "[]";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, requests)
    }

    async fn spawn_overlap_partition_test_server()
    -> (SocketAddr, Arc<Mutex<Vec<HashMap<String, String>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let server_requests = Arc::clone(&server_requests);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (path, params) = parse_request_target(&request);
                    server_requests.lock().unwrap().push(params.clone());

                    let body = if path == "/events"
                        && params.get("tag_slug").map(String::as_str) == Some("bitcoin")
                    {
                        let canonical_min = DateTime::parse_from_rfc3339(&decode_query_param(
                            params
                                .get("end_date_min")
                                .expect("canonical overlap fetch should send end_date_min"),
                        ))
                        .unwrap()
                        .with_timezone(&Utc);
                        let canonical_max = DateTime::parse_from_rfc3339(&decode_query_param(
                            params
                                .get("end_date_max")
                                .expect("canonical overlap fetch should send end_date_max"),
                        ))
                        .unwrap()
                        .with_timezone(&Utc);
                        let inside_both = canonical_min + ChronoDuration::seconds(120);
                        let outside_5m_only = canonical_max;
                        let outside_15m_only = canonical_min;

                        json!([
                            {
                                "id":"1",
                                "slug":"bitcoin-5m-alpha",
                                "endDate": inside_both.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            },
                            {
                                "id":"2",
                                "slug":"bitcoin-5m-late",
                                "endDate": outside_5m_only.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            },
                            {
                                "id":"3",
                                "slug":"bitcoin-15m-beta",
                                "endDate": inside_both.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            },
                            {
                                "id":"4",
                                "slug":"bitcoin-15m-early",
                                "endDate": outside_15m_only.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            }
                        ])
                        .to_string()
                    } else {
                        "[]".to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, requests)
    }

    async fn spawn_missing_end_date_test_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (path, params) = parse_request_target(&request);

                    let body = if path == "/events"
                        && params.get("tag_slug").map(String::as_str) == Some("bitcoin")
                    {
                        json!([
                            {"id":"1","slug":"bitcoin-5m-alpha","markets":[]}
                        ])
                        .to_string()
                    } else {
                        "[]".to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        addr
    }

    async fn spawn_sequenced_refresh_server(
        failing_request_count: usize,
    ) -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));
        let server_request_count = Arc::clone(&request_count);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let server_request_count = Arc::clone(&server_request_count);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (_, params) = parse_request_target(&request);
                    let request_number = server_request_count.fetch_add(1, Ordering::Relaxed) + 1;

                    let (status_line, body) = if request_number <= failing_request_count {
                        ("HTTP/1.1 500 Internal Server Error", "{}".to_string())
                    } else {
                        let canonical_min = DateTime::parse_from_rfc3339(&decode_query_param(
                            params
                                .get("end_date_min")
                                .expect("refresh request should send end_date_min"),
                        ))
                        .unwrap()
                        .with_timezone(&Utc);
                        let refreshed_end = canonical_min + ChronoDuration::seconds(1);
                        (
                            "HTTP/1.1 200 OK",
                            json!([{
                                "id":"recovered-event",
                                "slug":"bitcoin-5m-recovered",
                                "endDate": refreshed_end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                "markets":[]
                            }])
                            .to_string(),
                        )
                    };

                    let response = format!(
                        "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        (addr, request_count)
    }

    async fn wait_for_request_count(
        request_count: &std::sync::atomic::AtomicUsize,
        expected: usize,
    ) {
        use std::sync::atomic::Ordering;

        for _ in 0..200 {
            if request_count.load(Ordering::Relaxed) >= expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "expected at least {expected} requests, observed {}",
            request_count.load(Ordering::Relaxed)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_event_slugs_for_prefix_discoveries_with_gamma_client_filters_prefixes() {
        let (addr, requests) = spawn_test_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discoveries = vec![prefix_discovery("bitcoin", "bitcoin-5m", 30, 300)];

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            &discoveries,
            &client,
        )
        .await
        .unwrap();

        assert_eq!(
            event_slugs,
            BTreeMap::from([(discoveries[0].clone(), vec!["bitcoin-5m-alpha".to_string()])])
        );

        let recorded_requests = requests.lock().unwrap();
        assert_eq!(recorded_requests.len(), 1);
        let params = &recorded_requests[0];
        assert_eq!(params.get("tag_slug").map(String::as_str), Some("bitcoin"));
        assert_eq!(params.get("active"), None);
        assert_eq!(params.get("closed"), None);
        assert_eq!(params.get("archived"), None);

        let end_date_min = DateTime::parse_from_rfc3339(&decode_query_param(
            params
                .get("end_date_min")
                .expect("prefix discovery should send end_date_min"),
        ))
        .unwrap();
        let end_date_max = DateTime::parse_from_rfc3339(&decode_query_param(
            params
                .get("end_date_max")
                .expect("prefix discovery should send end_date_max"),
        ))
        .unwrap();
        assert_eq!((end_date_max - end_date_min).num_seconds(), 270);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_event_slugs_dedupes_identical_prefix_discovery_fetches() {
        let (addr, requests) = spawn_test_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discoveries = vec![
            prefix_discovery("bitcoin", "bitcoin-5m", 30, 300),
            prefix_discovery("bitcoin", "bitcoin-15m", 30, 300),
        ];

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            &discoveries,
            &client,
        )
        .await
        .unwrap();

        assert_eq!(
            event_slugs,
            BTreeMap::from([
                (discoveries[0].clone(), vec!["bitcoin-5m-alpha".to_string()]),
                (discoveries[1].clone(), vec!["bitcoin-15m-beta".to_string()]),
            ])
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_event_slugs_coalesces_overlapping_non_identical_discoveries() {
        let (addr, requests) = spawn_overlap_partition_test_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discovery_5m = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let discovery_15m = prefix_discovery("bitcoin", "bitcoin-15m", 60, 600);
        let discoveries = vec![discovery_5m.clone(), discovery_15m.clone()];

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            &discoveries,
            &client,
        )
        .await
        .unwrap();

        assert_eq!(
            event_slugs,
            BTreeMap::from([
                (discovery_5m.clone(), vec!["bitcoin-5m-alpha".to_string()]),
                (discovery_15m.clone(), vec!["bitcoin-15m-beta".to_string()]),
            ])
        );

        let recorded_requests = requests.lock().unwrap();
        assert_eq!(
            recorded_requests.len(),
            1,
            "overlapping windows should coalesce into one canonical Gamma fetch"
        );
        let params = &recorded_requests[0];
        assert_eq!(params.get("tag_slug").map(String::as_str), Some("bitcoin"));

        let end_date_min = DateTime::parse_from_rfc3339(&decode_query_param(
            params
                .get("end_date_min")
                .expect("coalesced overlap fetch should send end_date_min"),
        ))
        .unwrap();
        let end_date_max = DateTime::parse_from_rfc3339(&decode_query_param(
            params
                .get("end_date_max")
                .expect("coalesced overlap fetch should send end_date_max"),
        ))
        .unwrap();
        assert_eq!(
            (end_date_max - end_date_min).num_seconds(),
            570,
            "merged overlap query should widen from [30,300] and [60,600] to [30,600]"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn best_effort_overlap_path_matches_strict_partitioning() {
        let (addr, requests) = spawn_overlap_partition_test_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discovery_5m = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let discovery_15m = prefix_discovery("bitcoin", "bitcoin-15m", 60, 600);
        let discoveries = vec![discovery_5m.clone(), discovery_15m.clone()];

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_best_effort(
            &discoveries,
            &client,
        )
        .await
        .unwrap();

        assert_eq!(
            event_slugs,
            BTreeMap::from([
                (discovery_5m.clone(), vec!["bitcoin-5m-alpha".to_string()]),
                (discovery_15m.clone(), vec!["bitcoin-15m-beta".to_string()]),
            ])
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_task_end_to_end_preserves_then_ages_out_then_recovers() {
        use std::sync::atomic::Ordering;

        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 1, 1);
        let selector_state = PolymarketSelectorState::new(vec![(
            discovery.clone(),
            vec!["bitcoin-5m-startup".to_string()],
        )]);
        let (addr, request_count) = spawn_sequenced_refresh_server(2).await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let guard = spawn_selector_refresh_task_with_client(
            selector_state.clone(),
            vec![discovery.clone()],
            2,
            client,
        );

        wait_for_request_count(&request_count, 1).await;
        assert_eq!(
            selector_state.current_event_slugs(),
            vec!["bitcoin-5m-startup".to_string()]
        );
        assert_eq!(
            selector_state.event_slugs_read_for_discovery(&discovery),
            SelectorDiscoveryRead::Live(vec!["bitcoin-5m-startup".to_string()])
        );

        tokio::time::sleep(Duration::from_millis(2100)).await;
        wait_for_request_count(&request_count, 2).await;
        assert!(
            selector_state.current_event_slugs().is_empty(),
            "failed-group snapshot should remain aged out while later refreshes are still failing"
        );
        assert_eq!(
            selector_state.event_slugs_read_for_discovery(&discovery),
            SelectorDiscoveryRead::AgedOut
        );

        tokio::time::sleep(Duration::from_millis(2100)).await;
        wait_for_request_count(&request_count, 3).await;
        assert_eq!(
            selector_state.current_event_slugs(),
            vec!["bitcoin-5m-recovered".to_string()]
        );
        assert_eq!(
            selector_state.event_slugs_read_for_discovery(&discovery),
            SelectorDiscoveryRead::Live(vec!["bitcoin-5m-recovered".to_string()])
        );
        assert_eq!(request_count.load(Ordering::Relaxed), 3);

        guard.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn canonical_window_members_accept_missing_end_date_on_prefix_match() {
        let addr = spawn_missing_end_date_test_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            std::slice::from_ref(&discovery),
            &client,
        )
        .await
        .unwrap();

        assert_eq!(
            event_slugs,
            BTreeMap::from([(discovery.clone(), vec!["bitcoin-5m-alpha".to_string()])]),
            "canonical-window members should keep the pre-existing prefix-match behavior when Gamma omits endDate"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_event_slugs_keeps_non_overlapping_same_tag_discoveries_separate() {
        let (addr, requests) = spawn_recording_empty_server().await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discovery_5m = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let discovery_15m = prefix_discovery("bitcoin", "bitcoin-15m", 600, 900);

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            &[discovery_5m.clone(), discovery_15m.clone()],
            &client,
        )
        .await
        .unwrap();

        assert_eq!(
            event_slugs,
            BTreeMap::from([
                (discovery_5m.clone(), Vec::<String>::new()),
                (discovery_15m.clone(), Vec::<String>::new()),
            ])
        );

        let recorded_requests = requests.lock().unwrap();
        assert_eq!(
            recorded_requests.len(),
            2,
            "non-overlapping windows should remain separate canonical fetches"
        );
    }

    /// Test server that returns 500 for a configured set of tag_slugs and 200
    /// with the provided body for all other tag_slugs.
    async fn spawn_test_server_with_failing_tags(
        success_bodies: HashMap<String, serde_json::Value>,
        failing_tags: Vec<String>,
    ) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let success_bodies = Arc::new(success_bodies);
        let failing_tags = Arc::new(failing_tags);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let success_bodies = Arc::clone(&success_bodies);
                let failing_tags = Arc::clone(&failing_tags);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let (_, params) = parse_request_target(&request);

                    let tag_slug = params.get("tag_slug").cloned().unwrap_or_default();
                    let (status_line, body) = if failing_tags.contains(&tag_slug) {
                        ("HTTP/1.1 500 Internal Server Error", "{}".to_string())
                    } else {
                        let canonical_min = params.get("end_date_min").map(|value| {
                            DateTime::parse_from_rfc3339(&decode_query_param(value))
                                .unwrap()
                                .with_timezone(&Utc)
                        });
                        let value = success_bodies
                            .get(&tag_slug)
                            .cloned()
                            .unwrap_or_else(|| json!([]));
                        let body = match (canonical_min, value) {
                            (Some(canonical_min), serde_json::Value::Array(events)) => {
                                let inside_range = canonical_min + ChronoDuration::seconds(120);
                                serde_json::Value::Array(
                                    events
                                        .into_iter()
                                        .map(|event| match event {
                                            serde_json::Value::Object(mut object) => {
                                                object.entry("endDate".to_string()).or_insert_with(
                                                    || {
                                                        serde_json::Value::String(
                                                            inside_range.to_rfc3339_opts(
                                                                chrono::SecondsFormat::Secs,
                                                                true,
                                                            ),
                                                        )
                                                    },
                                                );
                                                serde_json::Value::Object(object)
                                            }
                                            other => other,
                                        })
                                        .collect(),
                                )
                                .to_string()
                            }
                            (_, value) => value.to_string(),
                        };
                        ("HTTP/1.1 200 OK", body)
                    };

                    let response = format!(
                        "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        addr
    }

    #[tokio::test(flavor = "current_thread")]
    async fn strict_variant_aborts_when_any_group_fails() {
        let bodies = HashMap::from([(
            "ethereum".to_string(),
            json!([{"id":"e1","slug":"ethereum-5m-alpha","markets":[]}]),
        )]);
        let addr = spawn_test_server_with_failing_tags(bodies, vec!["bitcoin".to_string()]).await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discoveries = vec![
            prefix_discovery("bitcoin", "bitcoin-5m", 30, 300),
            prefix_discovery("ethereum", "ethereum-5m", 30, 300),
        ];

        let err = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_strict(
            &discoveries,
            &client,
        )
        .await
        .expect_err("strict variant must abort when any group's gamma fetch fails");
        // Proves fail-closed behavior required by startup path.
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn best_effort_variant_returns_healthy_groups_and_omits_failed_groups() {
        let bodies = HashMap::from([(
            "ethereum".to_string(),
            json!([{"id":"e1","slug":"ethereum-5m-alpha","markets":[]}]),
        )]);
        let addr = spawn_test_server_with_failing_tags(bodies, vec!["bitcoin".to_string()]).await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let btc_discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let eth_discovery = prefix_discovery("ethereum", "ethereum-5m", 30, 300);
        let discoveries = vec![btc_discovery.clone(), eth_discovery.clone()];

        let event_slugs = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_best_effort(
            &discoveries,
            &client,
        )
        .await
        .expect("best-effort variant must not propagate per-group gamma errors");

        // ethereum succeeded → present.
        assert_eq!(
            event_slugs.get(&eth_discovery).map(|v| v.as_slice()),
            Some(["ethereum-5m-alpha".to_string()].as_slice()),
            "healthy group must be populated"
        );
        // bitcoin failed → omitted (upsert will leave prior state untouched).
        assert!(
            !event_slugs.contains_key(&btc_discovery),
            "failed group must be omitted from best-effort result so upsert preserves prior state"
        );
    }

    /// Test server that accepts the TCP connection but never writes a response.
    /// Simulates a Gamma backend hang so we can verify shutdown preempts the fetch.
    async fn spawn_hanging_test_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    // Hold the connection open indefinitely. Drop only when the
                    // outer test task ends.
                    let _keep = stream;
                    std::future::pending::<()>().await;
                });
            }
        });
        addr
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_task_shutdown_preempts_in_flight_gamma_fetch() {
        // Regression guard for Fix 5 (PR #183): the refresh task must wrap its
        // fetch in a tokio::select! against the cancellation token. Previously,
        // a hanging Gamma backend could stall shutdown for up to timeout_secs.
        let addr = spawn_hanging_test_server().await;
        // Generous HTTP timeout so that WITHOUT cancellation preemption, the fetch
        // would block far past our shutdown budget.
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 30).unwrap();
        let state =
            PolymarketSelectorState::new(Vec::<(PolymarketPrefixDiscovery, Vec<String>)>::new());
        let discoveries = vec![prefix_discovery("bitcoin", "bitcoin-5m", 30, 300)];
        let guard = spawn_selector_refresh_task_with_client(state, discoveries, 60, client);

        // Give the task a moment to fire its first tick and enter the fetch.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let start = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(2), guard.shutdown())
            .await
            .expect("shutdown must return well under the HTTP client timeout");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "shutdown must preempt in-flight fetch; took {elapsed:?} (budget 500ms)"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_task_best_effort_preserves_prior_state_for_failed_group() {
        // End-to-end: selector_state seeded with both groups; best-effort refresh
        // returns success only for ethereum and skips bitcoin; upsert must leave
        // bitcoin's prior slugs intact.
        let bodies = HashMap::from([(
            "ethereum".to_string(),
            json!([{"id":"e1","slug":"ethereum-5m-alpha","markets":[]}]),
        )]);
        let addr = spawn_test_server_with_failing_tags(bodies, vec!["bitcoin".to_string()]).await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let btc_discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let eth_discovery = prefix_discovery("ethereum", "ethereum-5m", 30, 300);
        let state = PolymarketSelectorState::new(vec![
            (btc_discovery.clone(), vec!["bitcoin-5m-prior".to_string()]),
            (eth_discovery.clone(), vec!["ethereum-5m-prior".to_string()]),
        ]);

        let refresh = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_best_effort(
            &[btc_discovery.clone(), eth_discovery.clone()],
            &client,
        )
        .await
        .unwrap();
        state.upsert_event_slugs_by_discovery(refresh);

        assert_eq!(
            state.event_slugs_for_discovery(&btc_discovery),
            vec!["bitcoin-5m-prior".to_string()],
            "failed group's prior state must be preserved across best-effort refresh"
        );
        assert_eq!(
            state.event_slugs_for_discovery(&eth_discovery),
            vec!["ethereum-5m-alpha".to_string()],
            "healthy group must be overwritten with fresh slugs"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_task_best_effort_clears_prior_state_when_successful_group_has_no_matches() {
        let bodies = HashMap::from([(
            "bitcoin".to_string(),
            json!([{"id":"b1","slug":"bitcoin-15m-alpha","markets":[]}]),
        )]);
        let addr = spawn_test_server_with_failing_tags(bodies, Vec::new()).await;
        let client = PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap();
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let state = PolymarketSelectorState::new(vec![(
            discovery.clone(),
            vec!["bitcoin-5m-prior".to_string()],
        )]);

        let refresh = resolve_event_slugs_for_prefix_discoveries_with_gamma_client_best_effort(
            std::slice::from_ref(&discovery),
            &client,
        )
        .await
        .unwrap();
        state.upsert_event_slugs_by_discovery(refresh);

        assert!(
            state.event_slugs_for_discovery(&discovery).is_empty(),
            "successful best-effort refresh with zero matches must clear stale selector state"
        );
    }

    #[test]
    fn matches_time_bounds_returns_false_when_expiry_offsets_overflow_i64() {
        let now = DateTime::parse_from_rfc3339("2026-04-19T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", u64::MAX, u64::MAX);
        let event = GammaEvent {
            id: "e1".to_string(),
            slug: Some("bitcoin-5m-alpha".to_string()),
            title: None,
            description: None,
            start_date: None,
            end_date: Some("2026-04-18T23:59:59Z".to_string()),
            active: None,
            closed: None,
            archived: None,
            markets: Vec::new(),
            liquidity: None,
            volume: None,
            open_interest: None,
            volume_24hr: None,
            category: None,
            neg_risk: None,
            neg_risk_market_id: None,
            featured: None,
        };

        assert!(
            !matches_time_bounds(&event, &discovery, now),
            "overflowing expiry offsets must fail closed"
        );
    }

    #[test]
    fn matches_time_bounds_accepts_exact_boundary_with_subsecond_now() {
        let now = DateTime::parse_from_rfc3339("2026-04-19T00:00:00.500Z")
            .unwrap()
            .with_timezone(&Utc);
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let event = GammaEvent {
            id: "e2".to_string(),
            slug: Some("bitcoin-5m-boundary".to_string()),
            title: None,
            description: None,
            start_date: None,
            end_date: Some("2026-04-19T00:00:30Z".to_string()),
            active: None,
            closed: None,
            archived: None,
            markets: Vec::new(),
            liquidity: None,
            volume: None,
            open_interest: None,
            volume_24hr: None,
            category: None,
            neg_risk: None,
            neg_risk_market_id: None,
            featured: None,
        };

        assert!(
            matches_time_bounds(&event, &discovery, now),
            "second-aligned boundary events should not be dropped because local now had fractional seconds"
        );
    }

    #[test]
    fn prefix_discovery_builds_narrowed_event_params() {
        let discovery = prefix_discovery("ethereum", "eth-updown-5m", 30, 300);
        let now = DateTime::parse_from_rfc3339("2026-04-16T10:38:38Z")
            .unwrap()
            .with_timezone(&Utc);

        let params = gamma_event_params_for_prefix_discovery(&discovery, now).unwrap();

        assert_eq!(params.tag_slug.as_deref(), Some("ethereum"));
        assert_eq!(params.active, None);
        assert_eq!(params.closed, None);
        assert_eq!(params.archived, None);
        assert_eq!(params.end_date_min.as_deref(), Some("2026-04-16T10:39:08Z"));
        assert_eq!(params.end_date_max.as_deref(), Some("2026-04-16T10:43:38Z"));
    }

    #[test]
    fn build_data_client_uses_event_slug_filter_when_selector_state_present() {
        let selectors = vec![PolymarketRulesetSelector {
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: Some("bitcoin-5m".to_string()),
        }];
        let selector_state = Some(PolymarketSelectorState::new(vec![(
            prefix_discovery("bitcoin", "bitcoin-5m", 30, 300),
            vec!["bitcoin-5m-alpha".to_string()],
        )]));
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let (_, config) = build_data_client(&raw, &selectors, selector_state).unwrap();
        let debug = format!("{config:?}");

        assert!(debug.contains("EventSlugFilter"), "{debug}");
        assert!(config.new_market_filter.is_none());
        assert!(!config.subscribe_new_markets);
    }

    #[test]
    fn build_data_client_uses_event_params_filter_for_tag_only_selectors() {
        let selectors = vec![PolymarketRulesetSelector {
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: None,
        }];
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let (_, config) = build_data_client(&raw, &selectors, None).unwrap();
        let debug = format!("{config:?}");

        assert!(debug.contains("EventParamsFilter"), "{debug}");
        assert!(!debug.contains("EventSlugFilter"), "{debug}");
    }

    #[test]
    fn build_data_client_accepts_gamma_event_fetch_max_concurrent_in_ruleset_mode() {
        let selectors = vec![PolymarketRulesetSelector {
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: None,
        }];
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            gamma_event_fetch_max_concurrent = 8
            ws_max_subscriptions = 200
        }
        .into();

        let (_, config) = build_data_client(&raw, &selectors, None)
            .expect("schema boundary should accept gamma_event_fetch_max_concurrent");
        let debug = format!("{config:?}");

        assert!(debug.contains("EventParamsFilter"), "{debug}");
        assert!(!debug.contains("EventSlugFilter"), "{debug}");
    }

    #[test]
    fn build_data_client_rejects_empty_event_slugs_without_rulesets() {
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
            event_slugs = []
        }
        .into();

        let err = build_data_client(&raw, &[], None)
            .expect_err("legacy mode without event slugs should be rejected");
        assert!(err.to_string().contains("must contain at least one slug"));
    }

    #[test]
    fn build_data_client_rejects_prefix_selectors_without_selector_state() {
        let selectors = vec![PolymarketRulesetSelector {
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: Some("bitcoin-5m".to_string()),
        }];
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let err = build_data_client(&raw, &selectors, None)
            .expect_err("prefix selectors without selector state should be rejected");
        assert!(
            err.to_string()
                .contains("prefix selectors require selector_state")
        );
    }

    #[test]
    fn build_data_client_rejects_mixed_selectors() {
        let selectors = vec![
            PolymarketRulesetSelector {
                tag_slug: "bitcoin".to_string(),
                event_slug_prefix: Some("bitcoin-5m".to_string()),
            },
            PolymarketRulesetSelector {
                tag_slug: "ethereum".to_string(),
                event_slug_prefix: None,
            },
        ];
        let selector_state = Some(PolymarketSelectorState::new(vec![(
            prefix_discovery("bitcoin", "bitcoin-5m", 30, 300),
            vec!["bitcoin-5m-alpha".to_string()],
        )]));
        let raw = toml::toml! {
            subscribe_new_markets = true
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 45
            ws_max_subscriptions = 200
        }
        .into();

        let err = build_data_client(&raw, &selectors, selector_state)
            .expect_err("mixed selectors should be rejected at the build_data_client boundary");
        assert!(
            err.to_string()
                .contains("uniformly tag-only or uniformly prefix-based")
        );
    }

    #[test]
    fn selector_state_returns_exact_discovery_owned_slugs() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let selector_state = PolymarketSelectorState::new(vec![(
            discovery.clone(),
            vec!["bitcoin-1m-alpha".to_string()],
        )]);

        assert_eq!(
            selector_state.event_slugs_for_discovery(&discovery),
            vec!["bitcoin-1m-alpha".to_string()]
        );
        assert!(
            selector_state
                .event_slugs_for_discovery(&prefix_discovery("bitcoin", "bitcoin-15m", 30, 300))
                .is_empty()
        );
    }

    #[test]
    fn selector_state_clears_discovery_when_refresh_returns_empty_slugs() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let selector_state = PolymarketSelectorState::new(vec![(
            discovery.clone(),
            vec!["bitcoin-5m-alpha".to_string()],
        )]);

        assert_eq!(
            selector_state.event_slugs_for_discovery(&discovery),
            vec!["bitcoin-5m-alpha".to_string()]
        );

        selector_state.upsert_event_slugs_by_discovery(vec![(discovery.clone(), Vec::new())]);

        assert!(
            selector_state
                .event_slugs_for_discovery(&discovery)
                .is_empty(),
            "empty Gamma refresh response must clear previous selector state, not preserve it"
        );
    }

    #[test]
    fn selector_state_ages_out_snapshots_after_max_expiry_window() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let seeded_at = DateTime::parse_from_rfc3339("2026-04-19T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let selector_state = PolymarketSelectorState::new_at(
            vec![(discovery.clone(), vec!["bitcoin-5m-alpha".to_string()])],
            seeded_at,
        );

        assert_eq!(
            selector_state
                .event_slugs_for_discovery_at(&discovery, seeded_at + ChronoDuration::seconds(299)),
            vec!["bitcoin-5m-alpha".to_string()]
        );
        assert!(
            selector_state
                .event_slugs_for_discovery_at(&discovery, seeded_at + ChronoDuration::seconds(301))
                .is_empty(),
            "stale selector snapshots should age out after the discovery max expiry window"
        );
        assert!(
            selector_state
                .current_event_slugs_at(seeded_at + ChronoDuration::seconds(301))
                .is_empty(),
            "aged-out snapshots must disappear from the WS filter view too"
        );
    }

    #[test]
    fn stale_generation_cache_entry_is_ignored_after_upsert() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let seeded_at = DateTime::parse_from_rfc3339("2026-04-19T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let selector_state = PolymarketSelectorState::new_at(
            vec![(discovery.clone(), vec!["bitcoin-5m-alpha".to_string()])],
            seeded_at,
        );

        selector_state
            .current_event_slugs_cache
            .store(Some(Arc::new(CurrentEventSlugsCache {
                as_of: seeded_at,
                generation: 0,
                event_slugs: vec!["stale-cache-value".to_string()],
            })));
        selector_state
            .snapshot_generation
            .store(1, Ordering::Release);

        assert_eq!(
            selector_state.current_event_slugs_at(seeded_at),
            vec!["bitcoin-5m-alpha".to_string()],
            "cache entries with an old generation must be ignored and recomputed"
        );
    }

    #[test]
    fn omitted_failed_group_snapshot_still_ages_out_after_max_expiry_window() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let seeded_at = DateTime::parse_from_rfc3339("2026-04-19T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let selector_state = PolymarketSelectorState::new_at(
            vec![(discovery.clone(), vec!["bitcoin-5m-alpha".to_string()])],
            seeded_at,
        );

        selector_state.upsert_event_slugs_by_discovery_at(
            Vec::<(PolymarketPrefixDiscovery, Vec<String>)>::new(),
            seeded_at + ChronoDuration::seconds(60),
        );

        assert_eq!(
            selector_state
                .event_slugs_for_discovery_at(&discovery, seeded_at + ChronoDuration::seconds(300)),
            vec!["bitcoin-5m-alpha".to_string()]
        );
        assert!(
            selector_state
                .event_slugs_for_discovery_at(&discovery, seeded_at + ChronoDuration::seconds(301))
                .is_empty(),
            "omitted failed-group snapshots must age out instead of staying live forever"
        );
    }

    #[test]
    fn refresh_interval_must_be_below_smallest_prefix_expiry_window() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let setup = PolymarketRulesetSetup {
            selectors: vec![discovery.selector.clone()],
            prefix_discoveries: vec![discovery.clone()],
            selector_state: Some(PolymarketSelectorState::new(vec![(
                discovery.clone(),
                vec!["bitcoin-5m-alpha".to_string()],
            )])),
        };
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 300
            ws_max_subscriptions = 200
        }
        .into();

        let err = setup
            .build_data_client(&raw)
            .expect_err("refresh interval equal to the discovery max-expiry window should be rejected before runtime wiring");
        assert!(
            err.to_string()
                .contains("gamma_refresh_interval_secs (300) must be <")
        );
    }

    #[test]
    fn refresh_interval_must_be_positive_for_prefix_refresh() {
        let discovery = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let setup = PolymarketRulesetSetup {
            selectors: vec![discovery.selector.clone()],
            prefix_discoveries: vec![discovery.clone()],
            selector_state: Some(PolymarketSelectorState::new(vec![(
                discovery.clone(),
                vec!["bitcoin-5m-alpha".to_string()],
            )])),
        };
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_secs = 0
            ws_max_subscriptions = 200
        }
        .into();

        let err = setup
            .build_data_client(&raw)
            .expect_err("zero refresh interval should be rejected explicitly");
        assert!(
            err.to_string()
                .contains("gamma_refresh_interval_secs must be > 0")
        );
    }

    fn ruleset_with_selector(id: &str, selector_toml: Value) -> RulesetConfig {
        RulesetConfig {
            id: id.to_string(),
            venue: RulesetVenueKind::Polymarket,
            selector: selector_toml,
            resolution_basis: "binance_btcusdt_1m".to_string(),
            min_time_to_expiry_secs: 60,
            max_time_to_expiry_secs: 900,
            min_liquidity_num: 1000.0,
            require_accepting_orders: true,
            freeze_before_end_secs: 90,
            selector_poll_interval_ms: 1_000,
            candidate_load_timeout_secs: 30,
        }
    }

    fn tag_only_ruleset(id: &str, tag_slug: &str) -> RulesetConfig {
        ruleset_with_selector(
            id,
            toml::toml! {
                tag_slug = tag_slug
            }
            .into(),
        )
    }

    fn prefix_ruleset(id: &str, tag_slug: &str, event_slug_prefix: &str) -> RulesetConfig {
        ruleset_with_selector(
            id,
            toml::toml! {
                tag_slug = tag_slug
                event_slug_prefix = event_slug_prefix
            }
            .into(),
        )
    }

    #[test]
    fn rejects_mixed_tag_only_and_prefix_polymarket_rulesets_globally() {
        let rulesets = vec![
            tag_only_ruleset("ETH-TAG", "ethereum"),
            prefix_ruleset("BTC-5M", "bitcoin", "bitcoin-5m"),
        ];

        let err = PolymarketRulesetSetup::from_rulesets(&rulesets, 5)
            .expect_err("mixed tag-only and prefix selectors must be rejected");
        let message = err.to_string();

        assert!(
            message.contains("tag-only ruleset selector"),
            "error should name tag-only ruleset selectors: {message}"
        );
        assert!(
            message.contains("prefix-based ruleset selector"),
            "error should name prefix-based ruleset selectors: {message}"
        );
        assert!(
            message.contains("ethereum"),
            "error should name the tag-only tag_slug: {message}"
        );
        assert!(
            message.contains("bitcoin-5m"),
            "error should name the prefix: {message}"
        );
        assert!(
            message.contains("uniformly tag-only or uniformly prefix-based"),
            "error should state the uniformity requirement: {message}"
        );
    }

    #[test]
    fn accepts_all_tag_only_polymarket_rulesets_globally() {
        let rulesets = vec![
            tag_only_ruleset("ETH-TAG", "ethereum"),
            tag_only_ruleset("BTC-TAG", "bitcoin"),
        ];

        let setup = PolymarketRulesetSetup::from_rulesets(&rulesets, 5)
            .expect("all-tag-only selectors must be accepted");

        assert!(setup.selector_state().is_none());
        assert!(setup.resolved_prefix_event_slugs().is_empty());
    }

    #[test]
    fn accepts_all_prefix_polymarket_rulesets_invariant_passes() {
        // Pure unit-level check that the invariant accepts an all-prefix ruleset set.
        // This does not exercise the full from_rulesets path (which would require
        // live Gamma HTTP for prefix discovery); instead it directly verifies the
        // validation step that gates everything downstream.
        let rulesets = vec![
            prefix_ruleset("BTC-5M", "bitcoin", "bitcoin-5m"),
            prefix_ruleset("BTC-15M", "bitcoin", "bitcoin-15m"),
        ];

        let selectors = polymarket_ruleset_selectors(&rulesets)
            .expect("selectors should parse for all-prefix rulesets");
        reject_mixed_polymarket_rulesets_global_scope(&selectors)
            .expect("all-prefix selectors must pass the mixed-scope check");

        let prefix_discoveries = polymarket_ruleset_prefix_discoveries(&rulesets)
            .expect("prefix discoveries should parse for all-prefix rulesets");
        assert_eq!(prefix_discoveries.len(), 2);
        assert!(
            prefix_discoveries
                .iter()
                .all(|d| d.selector.event_slug_prefix.is_some())
        );
    }

    #[test]
    fn from_rulesets_rejects_mixed_selectors_before_attempting_gamma_http() {
        // Proves the invariant is enforced BEFORE any Gamma HTTP call is attempted.
        // If the invariant were checked after build_selector_state (which issues HTTP
        // with the configured timeout), rejection would take at least timeout_secs*1000
        // ms. We pass timeout_secs=1 and assert the error returns in well under that
        // bound, guaranteeing the ordering.
        let rulesets = vec![
            tag_only_ruleset("ETH-TAG", "ethereum"),
            prefix_ruleset("BTC-5M", "bitcoin", "bitcoin-5m"),
        ];
        let start = std::time::Instant::now();
        let err = PolymarketRulesetSetup::from_rulesets(&rulesets, 1)
            .expect_err("mixed selectors should be rejected");
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 200,
            "rejection must be pre-HTTP; took {}ms (invariant may have been checked after HTTP)",
            elapsed.as_millis()
        );
        assert!(
            err.to_string().contains("uniformly"),
            "error should mention uniformity requirement: {err}"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn selector_state_readers_never_see_partial_map_under_concurrent_writes() {
        // Readers must only ever observe a fully-committed snapshot (old map entirely
        // or new map entirely). ArcSwap guarantees this; this test is a regression
        // guard against a future refactor that reintroduces a lock+mutate pattern
        // that could expose torn reads.
        let discovery_a = prefix_discovery("bitcoin", "bitcoin-5m", 30, 300);
        let discovery_b = prefix_discovery("bitcoin", "bitcoin-15m", 30, 300);
        let initial = vec![
            (discovery_a.clone(), vec!["bitcoin-5m-alpha".to_string()]),
            (discovery_b.clone(), vec!["bitcoin-15m-alpha".to_string()]),
        ];
        let state = PolymarketSelectorState::new(initial);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mut reader_handles = Vec::new();
        for _ in 0..8 {
            let state = state.clone();
            let stop = Arc::clone(&stop);
            let discovery_a = discovery_a.clone();
            let discovery_b = discovery_b.clone();
            reader_handles.push(tokio::spawn(async move {
                let mut observations: u64 = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let slugs_a = state.event_slugs_for_discovery(&discovery_a);
                    let slugs_b = state.event_slugs_for_discovery(&discovery_b);
                    // Both per-discovery lists must always be non-empty because the
                    // writer only swaps between complete paired snapshots — never
                    // clears one side alone.
                    assert!(
                        !slugs_a.is_empty(),
                        "discovery_a must always observe a non-empty slug list; got {slugs_a:?}"
                    );
                    assert!(
                        !slugs_b.is_empty(),
                        "discovery_b must always observe a non-empty slug list; got {slugs_b:?}"
                    );
                    // Each slug must be one of the permitted snapshot values for its
                    // discovery — never a mixed/interleaved value.
                    for slug in &slugs_a {
                        assert!(
                            slug == "bitcoin-5m-alpha" || slug == "bitcoin-5m-beta",
                            "discovery_a saw unexpected slug: {slug}"
                        );
                    }
                    for slug in &slugs_b {
                        assert!(
                            slug == "bitcoin-15m-alpha" || slug == "bitcoin-15m-beta",
                            "discovery_b saw unexpected slug: {slug}"
                        );
                    }
                    observations += 1;
                    tokio::task::yield_now().await;
                }
                observations
            }));
        }

        let writer_handle = {
            let state = state.clone();
            let stop = Arc::clone(&stop);
            let discovery_a = discovery_a.clone();
            let discovery_b = discovery_b.clone();
            tokio::spawn(async move {
                let mut toggle = false;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let (a, b) = if toggle {
                        (
                            vec!["bitcoin-5m-beta".to_string()],
                            vec!["bitcoin-15m-beta".to_string()],
                        )
                    } else {
                        (
                            vec!["bitcoin-5m-alpha".to_string()],
                            vec!["bitcoin-15m-alpha".to_string()],
                        )
                    };
                    state.upsert_event_slugs_by_discovery(vec![
                        (discovery_a.clone(), a),
                        (discovery_b.clone(), b),
                    ]);
                    toggle = !toggle;
                    tokio::task::yield_now().await;
                }
            })
        };

        tokio::time::sleep(Duration::from_millis(500)).await;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writer_handle.await.unwrap();
        let mut total_observations = 0u64;
        for handle in reader_handles {
            total_observations += handle.await.unwrap();
        }
        assert!(
            total_observations > 0,
            "readers should have made at least one observation; got 0"
        );
    }

    #[test]
    fn rejects_unknown_field_in_polymarket_data_client_config() {
        // Regression guard: a typo like `gamma_refresh_interval_sec` (missing trailing `s`)
        // must be rejected rather than silently falling back to the default value.
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_sec = 60
            ws_max_subscriptions = 200
        }
        .into();

        let result = build_data_client(&raw, &[], None);
        let err = result.expect_err("unknown field should be rejected");
        let message = err.to_string();
        assert!(
            message.contains("gamma_refresh_interval_sec"),
            "error should name the unknown field: {message}"
        );
    }

    #[test]
    fn ruleset_mode_rejects_unknown_field_in_polymarket_data_client_config() {
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_sec = 60
            ws_max_subscriptions = 200
        }
        .into();

        let setup =
            PolymarketRulesetSetup::from_rulesets(&[tag_only_ruleset("BTC-TAG", "bitcoin")], 5)
                .expect("tag-only ruleset setup should stay offline");
        let err = setup
            .build_data_client(&raw)
            .expect_err("unknown field should be rejected in ruleset mode");
        let message = err.to_string();
        assert!(message.contains("gamma_refresh_interval_sec"), "{message}");
    }

    #[test]
    fn gamma_refresh_interval_reader_rejects_unknown_field_in_ruleset_mode() {
        let raw = toml::toml! {
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            gamma_refresh_interval_sec = 60
            ws_max_subscriptions = 200
        }
        .into();

        let err = gamma_refresh_interval_secs(&raw)
            .expect_err("unknown field should be rejected by the refresh-interval reader");
        let message = err.to_string();
        assert!(message.contains("gamma_refresh_interval_sec"), "{message}");
    }

    #[test]
    fn legacy_event_slugs_config_builds_client_without_rulesets() {
        let raw = toml::toml! {
            event_slugs = ["eth-updown-5m-2026-04-18"]
            subscribe_new_markets = false
        }
        .into();

        let setup = PolymarketRulesetSetup::from_rulesets(&[], 5)
            .expect("legacy setup with zero rulesets must construct");
        let result = setup.build_data_client(&raw);
        let (_, config) = result.expect("legacy event_slugs config must build a data client");

        assert!(!config.subscribe_new_markets);
        assert_eq!(config.filters.len(), 1);
        let debug = format!("{:?}", config.filters);
        assert!(debug.contains("EventSlugFilter"), "{debug}");
    }
}
