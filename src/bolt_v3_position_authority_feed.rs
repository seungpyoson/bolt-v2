use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{Arc, Mutex, Weak},
};

use anyhow::{Context, Result};
use nautilus_common::{
    cache::Cache,
    msgbus::{self, MStr, Pattern, ShareableMessageHandler, switchboard::MessagingSwitchboard},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    enums::{OmsType, PositionSide, PositionSideSpecified},
    identifiers::{AccountId, ClientId, InstrumentId, PositionId, TradeId, Venue},
    reports::PositionStatusReport,
};
use rust_decimal::Decimal;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct BoltV3PositionAuthorityKey {
    execution_client_id: ClientId,
    account_id: AccountId,
    instrument_id: InstrumentId,
    venue_position_id: Option<PositionId>,
}

fn normalize_position_authority_key(
    execution_client_id: ClientId,
    account_id: AccountId,
    instrument_id: InstrumentId,
    venue_position_id: Option<PositionId>,
) -> BoltV3PositionAuthorityKey {
    BoltV3PositionAuthorityKey {
        execution_client_id,
        account_id,
        instrument_id,
        venue_position_id,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3PositionAuthorityObservationOutcome {
    Applied,
    IgnoredOutsideRegisteredAccounts,
    NoActiveLease(BoltV3PositionAuthorityKey),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoltV3PositionAuthoritySnapshot {
    pub(crate) report_id: UUID4,
    pub(crate) signed_quantity: Decimal,
    pub(crate) position_side: PositionSideSpecified,
    pub(crate) ts_last: UnixNanos,
    pub(crate) ts_init: UnixNanos,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3PositionAuthorityConflict {
    SameReportIdentityChanged,
    EqualTimestampChanged,
    GenerationOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BoltV3PositionAuthorityLeaseState {
    Awaiting,
    Coherent(BoltV3PositionAuthoritySnapshot),
    Conflicted(BoltV3PositionAuthorityConflict),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoltV3PositionAuthorityStaleHealth {
    pub(crate) observed_ts_last: UnixNanos,
    pub(crate) current_ts_last: UnixNanos,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoltV3PositionAuthorityLeaseObservation {
    pub(crate) state: BoltV3PositionAuthorityLeaseState,
    pub(crate) stale_health: Option<BoltV3PositionAuthorityStaleHealth>,
}

#[derive(Clone)]
pub struct BoltV3PositionAuthorityFeed {
    inner: Arc<Mutex<PositionAuthorityFeedState>>,
    cache: Rc<RefCell<Cache>>,
}

#[derive(Clone)]
pub struct BoltV3PositionAuthorityCapability {
    feed: BoltV3PositionAuthorityFeed,
    execution_client_id: ClientId,
    account_id: AccountId,
    oms_type: OmsType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoltV3CanonicalPositionAuthority {
    signed_quantity: Decimal,
    side: PositionSideSpecified,
    trade_ids: BTreeSet<TradeId>,
    target_scope: BoltV3CanonicalPositionTargetScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoltV3CanonicalPositionTargetScope {
    Exact,
    AmbiguousNettingAggregate,
}

impl BoltV3CanonicalPositionAuthority {
    pub(crate) fn is_exact_target(&self) -> bool {
        self.target_scope == BoltV3CanonicalPositionTargetScope::Exact
    }

    pub(crate) fn signed_quantity(&self) -> Decimal {
        self.signed_quantity
    }

    pub(crate) fn side(&self) -> PositionSideSpecified {
        self.side
    }

    pub(crate) fn trade_ids(&self) -> &BTreeSet<TradeId> {
        &self.trade_ids
    }

    #[cfg(test)]
    pub(crate) fn exact_for_test(
        signed_quantity: Decimal,
        side: PositionSideSpecified,
        trade_ids: BTreeSet<TradeId>,
    ) -> Self {
        Self {
            signed_quantity,
            side,
            trade_ids,
            target_scope: BoltV3CanonicalPositionTargetScope::Exact,
        }
    }

    #[cfg(test)]
    pub(crate) fn ambiguous_for_test(
        signed_quantity: Decimal,
        side: PositionSideSpecified,
        trade_ids: BTreeSet<TradeId>,
    ) -> Self {
        Self {
            signed_quantity,
            side,
            trade_ids,
            target_scope: BoltV3CanonicalPositionTargetScope::AmbiguousNettingAggregate,
        }
    }
}

pub(crate) struct BoltV3SealedPositionAuthority {
    canonical: BoltV3CanonicalPositionAuthority,
    lease: BoltV3PositionAuthorityLease,
}

impl BoltV3SealedPositionAuthority {
    pub(crate) fn canonical(&self) -> &BoltV3CanonicalPositionAuthority {
        &self.canonical
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BoltV3CanonicalPositionAuthority,
        BoltV3PositionAuthorityLease,
    ) {
        (self.canonical, self.lease)
    }
}

/// Owns the canonical position-authority feed and its raw NT report subscription.
///
/// Composition roots retain this value for exactly as long as strategies may
/// acquire or reconcile position-authority leases. The feed and subscription
/// cannot be constructed independently outside this module.
pub struct BoltV3PositionAuthorityRuntime {
    feed: BoltV3PositionAuthorityFeed,
    _subscription: BoltV3PositionAuthorityFeedSubscription,
}

impl BoltV3PositionAuthorityCapability {
    pub(crate) const fn new(
        feed: BoltV3PositionAuthorityFeed,
        execution_client_id: ClientId,
        account_id: AccountId,
        oms_type: OmsType,
    ) -> Self {
        Self {
            feed,
            execution_client_id,
            account_id,
            oms_type,
        }
    }

    fn venue_position_id(&self, position_id: PositionId) -> Result<Option<PositionId>> {
        match self.oms_type {
            OmsType::Hedging => Ok(Some(position_id)),
            OmsType::Netting => Ok(None),
            OmsType::Unspecified => {
                anyhow::bail!("position authority requires a specified OMS type")
            }
        }
    }

    pub(crate) fn acquire_for_position(
        &self,
        position_id: PositionId,
        instrument_id: InstrumentId,
    ) -> Result<BoltV3PositionAuthorityLease> {
        self.feed.acquire(
            self.execution_client_id,
            self.account_id,
            instrument_id,
            self.venue_position_id(position_id)?,
        )
    }

    pub(crate) fn canonical_position(
        &self,
        position_id: PositionId,
        instrument_id: InstrumentId,
    ) -> Result<Option<BoltV3CanonicalPositionAuthority>> {
        let cache = self.feed.cache.borrow();
        let Some(position) = cache.position(&position_id) else {
            return Ok(None);
        };
        anyhow::ensure!(
            position.account_id == self.account_id && position.instrument_id == instrument_id,
            "position authority cache identity mismatch: position_id={position_id} expected_account={} observed_account={} expected_instrument={instrument_id} observed_instrument={}",
            self.account_id,
            position.account_id,
            position.instrument_id,
        );
        let side = match position.side {
            PositionSide::Long => PositionSideSpecified::Long,
            PositionSide::Short => PositionSideSpecified::Short,
            PositionSide::Flat => PositionSideSpecified::Flat,
            PositionSide::NoPositionSide => {
                anyhow::bail!("canonical position authority has no specified side")
            }
        };
        let target_scope = match self.oms_type {
            OmsType::Hedging => BoltV3CanonicalPositionTargetScope::Exact,
            OmsType::Netting => {
                let matching_open_positions = cache.positions_open(
                    Some(&instrument_id.venue),
                    Some(&instrument_id),
                    None,
                    Some(&self.account_id),
                    None,
                );
                match (position.is_open(), matching_open_positions.as_slice()) {
                    (true, [only_open]) if only_open.id == position_id => {
                        BoltV3CanonicalPositionTargetScope::Exact
                    }
                    (false, []) => BoltV3CanonicalPositionTargetScope::Exact,
                    (true, _) | (false, _) => {
                        BoltV3CanonicalPositionTargetScope::AmbiguousNettingAggregate
                    }
                }
            }
            OmsType::Unspecified => {
                anyhow::bail!("position authority requires a specified OMS type")
            }
        };
        Ok(Some(BoltV3CanonicalPositionAuthority {
            signed_quantity: position.signed_decimal_qty(),
            side,
            trade_ids: position.trade_ids().into_iter().collect(),
            target_scope,
        }))
    }

    pub(crate) fn acquire_canonical_position(
        &self,
        position_id: PositionId,
        instrument_id: InstrumentId,
    ) -> Result<BoltV3SealedPositionAuthority> {
        let lease = self.acquire_for_position(position_id, instrument_id)?;
        let canonical = self
            .canonical_position(position_id, instrument_id)?
            .with_context(|| {
                format!(
                    "position authority cache is missing position_id={position_id} instrument_id={instrument_id}"
                )
            })?;
        Ok(BoltV3SealedPositionAuthority { canonical, lease })
    }

    #[cfg(test)]
    pub(crate) fn observe_for_test(
        &self,
        report: &PositionStatusReport,
    ) -> Result<BoltV3PositionAuthorityObservationOutcome> {
        self.feed.observe(report)
    }
}

impl BoltV3PositionAuthorityRuntime {
    pub(crate) fn try_new(
        bindings: impl IntoIterator<Item = (AccountId, ClientId, Venue)>,
        cache: Rc<RefCell<Cache>>,
    ) -> Result<Self> {
        let feed = BoltV3PositionAuthorityFeed::try_new_with_cache(bindings, cache)?;
        let subscription = feed.subscribe();
        Ok(Self {
            feed,
            _subscription: subscription,
        })
    }

    pub(crate) fn feed(&self) -> BoltV3PositionAuthorityFeed {
        self.feed.clone()
    }
}

#[derive(Default)]
struct PositionAuthorityFeedState {
    client_by_account: BTreeMap<AccountId, PositionAuthorityAccountBinding>,
    keys: BTreeMap<BoltV3PositionAuthorityKey, PositionAuthorityKeyState>,
}

struct PositionAuthorityKeyState {
    lease_count: u64,
    authority: BoltV3PositionAuthorityLeaseState,
    stale_health: Option<BoltV3PositionAuthorityStaleHealth>,
}

#[derive(Clone, Copy)]
struct PositionAuthorityAccountBinding {
    execution_client_id: ClientId,
    venue: Venue,
}

impl BoltV3PositionAuthorityFeed {
    pub(crate) fn try_new_with_cache(
        bindings: impl IntoIterator<Item = (AccountId, ClientId, Venue)>,
        cache: Rc<RefCell<Cache>>,
    ) -> Result<Self> {
        let mut client_by_account = BTreeMap::new();
        for (account_id, execution_client_id, venue) in bindings {
            let binding = PositionAuthorityAccountBinding {
                execution_client_id,
                venue,
            };
            if let Some(prior) = client_by_account.insert(account_id, binding)
                && (prior.execution_client_id != execution_client_id || prior.venue != venue)
            {
                anyhow::bail!(
                    "position authority account attribution is ambiguous: account_id={account_id} clients={},{} venues={},{}",
                    prior.execution_client_id,
                    execution_client_id,
                    prior.venue,
                    venue
                );
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(PositionAuthorityFeedState {
                client_by_account,
                keys: BTreeMap::new(),
            })),
            cache,
        })
    }

    pub(crate) fn acquire(
        &self,
        execution_client_id: ClientId,
        account_id: AccountId,
        instrument_id: InstrumentId,
        venue_position_id: Option<PositionId>,
    ) -> Result<BoltV3PositionAuthorityLease> {
        let key = normalize_position_authority_key(
            execution_client_id,
            account_id,
            instrument_id,
            venue_position_id,
        );
        let mut state = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("position authority feed lock is poisoned"))?;
        let attributed = state
            .client_by_account
            .get(&key.account_id)
            .with_context(|| {
                format!(
                    "position authority has no execution-client attribution for account_id={}",
                    key.account_id
                )
            })?;
        anyhow::ensure!(
            attributed.execution_client_id == key.execution_client_id
                && attributed.venue == key.instrument_id.venue,
            "position authority execution-client attribution mismatch: account_id={} expected={} observed={}",
            key.account_id,
            attributed.execution_client_id,
            key.execution_client_id
        );
        let entry = state
            .keys
            .entry(key.clone())
            .or_insert_with(|| PositionAuthorityKeyState {
                lease_count: 0,
                authority: BoltV3PositionAuthorityLeaseState::Awaiting,
                stale_health: None,
            });
        entry.lease_count = entry
            .lease_count
            .checked_add(1)
            .context("position authority lease count overflow")?;
        drop(state);
        Ok(BoltV3PositionAuthorityLease {
            feed: Arc::downgrade(&self.inner),
            key: Some(key),
        })
    }

    pub(crate) fn observe(
        &self,
        report: &PositionStatusReport,
    ) -> Result<BoltV3PositionAuthorityObservationOutcome> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("position authority feed lock is poisoned"))?;
        let Some(binding) = state.client_by_account.get(&report.account_id).copied() else {
            return Ok(BoltV3PositionAuthorityObservationOutcome::IgnoredOutsideRegisteredAccounts);
        };
        if report.instrument_id.venue != binding.venue {
            return Ok(BoltV3PositionAuthorityObservationOutcome::IgnoredOutsideRegisteredAccounts);
        }
        let key = normalize_position_authority_key(
            binding.execution_client_id,
            report.account_id,
            report.instrument_id,
            report.venue_position_id,
        );
        let Some(entry) = state.keys.get_mut(&key) else {
            return Ok(BoltV3PositionAuthorityObservationOutcome::NoActiveLease(
                key,
            ));
        };
        entry.observe(report);
        Ok(BoltV3PositionAuthorityObservationOutcome::Applied)
    }

    fn observe_with_operator_telemetry(&self, report: &PositionStatusReport) {
        match self.observe(report) {
            Ok(BoltV3PositionAuthorityObservationOutcome::NoActiveLease(key)) => {
                log::warn!(
                    "position authority report dropped without an active lease: execution_client_id={} account_id={} instrument_id={} venue_position_id={:?}",
                    key.execution_client_id,
                    key.account_id,
                    key.instrument_id,
                    key.venue_position_id,
                );
            }
            Ok(BoltV3PositionAuthorityObservationOutcome::Applied)
            | Ok(BoltV3PositionAuthorityObservationOutcome::IgnoredOutsideRegisteredAccounts) => {}
            Err(error) => {
                log::error!("position authority report observation failed: {error:#}");
            }
        }
    }

    fn subscribe(&self) -> BoltV3PositionAuthorityFeedSubscription {
        let pattern: MStr<Pattern> =
            MessagingSwitchboard::reconciliation_raw_position_status_report_topic()
                .as_str()
                .into();
        let feed = self.clone();
        let handler = ShareableMessageHandler::from_typed(move |report: &PositionStatusReport| {
            feed.observe_with_operator_telemetry(report);
        });
        msgbus::subscribe_any(pattern, handler.clone(), None);
        BoltV3PositionAuthorityFeedSubscription {
            pattern,
            handler: Some(handler),
        }
    }

    #[cfg(test)]
    fn active_key_count(&self) -> usize {
        self.inner
            .lock()
            .expect("test position authority feed lock should not be poisoned")
            .keys
            .len()
    }

    #[cfg(test)]
    fn set_generation_for_test(&self, key: &BoltV3PositionAuthorityKey, generation: u64) {
        let mut state = self
            .inner
            .lock()
            .expect("test position authority feed lock should not be poisoned");
        let entry = state
            .keys
            .get_mut(key)
            .expect("test lease key should exist");
        let BoltV3PositionAuthorityLeaseState::Coherent(snapshot) = &mut entry.authority else {
            panic!("test lease should hold a coherent snapshot");
        };
        snapshot.generation = generation;
    }
}

impl PositionAuthorityKeyState {
    fn observe(&mut self, report: &PositionStatusReport) {
        let observed = BoltV3PositionAuthoritySnapshot {
            report_id: report.report_id,
            signed_quantity: report.signed_decimal_qty,
            position_side: report.position_side,
            ts_last: report.ts_last,
            ts_init: report.ts_init,
            generation: 0,
        };
        let next = match &self.authority {
            BoltV3PositionAuthorityLeaseState::Awaiting => Some(
                BoltV3PositionAuthorityLeaseState::Coherent(BoltV3PositionAuthoritySnapshot {
                    generation: 1,
                    ..observed
                }),
            ),
            BoltV3PositionAuthorityLeaseState::Coherent(current) => {
                if observed.report_id == current.report_id {
                    if same_report_body(current, &observed) {
                        None
                    } else {
                        Some(BoltV3PositionAuthorityLeaseState::Conflicted(
                            BoltV3PositionAuthorityConflict::SameReportIdentityChanged,
                        ))
                    }
                } else if observed.ts_last < current.ts_last {
                    self.stale_health = Some(BoltV3PositionAuthorityStaleHealth {
                        observed_ts_last: observed.ts_last,
                        current_ts_last: current.ts_last,
                    });
                    None
                } else if observed.ts_last == current.ts_last
                    && (observed.signed_quantity != current.signed_quantity
                        || observed.position_side != current.position_side)
                {
                    Some(BoltV3PositionAuthorityLeaseState::Conflicted(
                        BoltV3PositionAuthorityConflict::EqualTimestampChanged,
                    ))
                } else if let Some(generation) = current.generation.checked_add(1) {
                    self.stale_health = None;
                    Some(BoltV3PositionAuthorityLeaseState::Coherent(
                        BoltV3PositionAuthoritySnapshot {
                            generation,
                            ..observed
                        },
                    ))
                } else {
                    Some(BoltV3PositionAuthorityLeaseState::Conflicted(
                        BoltV3PositionAuthorityConflict::GenerationOverflow,
                    ))
                }
            }
            BoltV3PositionAuthorityLeaseState::Conflicted(_) => None,
        };
        if let Some(next) = next {
            self.authority = next;
        }
    }
}

fn same_report_body(
    current: &BoltV3PositionAuthoritySnapshot,
    observed: &BoltV3PositionAuthoritySnapshot,
) -> bool {
    current.signed_quantity == observed.signed_quantity
        && current.position_side == observed.position_side
        && current.ts_last == observed.ts_last
        && current.ts_init == observed.ts_init
}

pub(crate) struct BoltV3PositionAuthorityLease {
    feed: Weak<Mutex<PositionAuthorityFeedState>>,
    key: Option<BoltV3PositionAuthorityKey>,
}

impl BoltV3PositionAuthorityLease {
    pub(crate) fn key(&self) -> &BoltV3PositionAuthorityKey {
        self.key
            .as_ref()
            .expect("active position authority lease must retain its key")
    }

    pub(crate) fn observation(&self) -> Result<BoltV3PositionAuthorityLeaseObservation> {
        let feed = self
            .feed
            .upgrade()
            .context("position authority feed no longer exists")?;
        let state = feed
            .lock()
            .map_err(|_| anyhow::anyhow!("position authority feed lock is poisoned"))?;
        let entry = state
            .keys
            .get(self.key())
            .context("position authority lease key is no longer active")?;
        Ok(BoltV3PositionAuthorityLeaseObservation {
            state: entry.authority.clone(),
            stale_health: entry.stale_health.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> Result<BoltV3PositionAuthorityLeaseState> {
        Ok(self.observation()?.state)
    }

    pub(crate) fn coherent_generation(&self) -> Result<Option<u64>> {
        Ok(match self.observation()?.state {
            BoltV3PositionAuthorityLeaseState::Coherent(snapshot) => Some(snapshot.generation),
            BoltV3PositionAuthorityLeaseState::Awaiting
            | BoltV3PositionAuthorityLeaseState::Conflicted(_) => None,
        })
    }

    #[cfg(test)]
    pub(crate) fn stale_health(&self) -> Result<Option<BoltV3PositionAuthorityStaleHealth>> {
        Ok(self.observation()?.stale_health)
    }
}

impl Drop for BoltV3PositionAuthorityLease {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        let Some(feed) = self.feed.upgrade() else {
            return;
        };
        let mut state = feed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = match state.keys.get_mut(&key) {
            Some(entry) if entry.lease_count > 1 => {
                entry.lease_count -= 1;
                false
            }
            Some(_) => true,
            None => false,
        };
        if remove {
            state.keys.remove(&key);
        }
    }
}

struct BoltV3PositionAuthorityFeedSubscription {
    pattern: MStr<Pattern>,
    handler: Option<ShareableMessageHandler>,
}

impl Drop for BoltV3PositionAuthorityFeedSubscription {
    fn drop(&mut self) {
        if let Some(handler) = self.handler.take() {
            msgbus::unsubscribe_any(self.pattern, &handler);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_model::types::Quantity;

    fn feed() -> BoltV3PositionAuthorityFeed {
        BoltV3PositionAuthorityFeed::try_new_with_cache(
            [(
                AccountId::from("ACCOUNT-001"),
                ClientId::from("execution-client"),
                Venue::from("POLYMARKET"),
            )],
            Rc::new(RefCell::new(Cache::default())),
        )
        .expect("test attribution should be unambiguous")
    }

    fn key(instrument: &str, venue_position_id: Option<&str>) -> BoltV3PositionAuthorityKey {
        normalize_position_authority_key(
            ClientId::from("execution-client"),
            AccountId::from("ACCOUNT-001"),
            InstrumentId::from(instrument),
            venue_position_id.map(PositionId::from),
        )
    }

    fn acquire(
        feed: &BoltV3PositionAuthorityFeed,
        instrument: &str,
        venue_position_id: Option<&str>,
    ) -> BoltV3PositionAuthorityLease {
        feed.acquire(
            ClientId::from("execution-client"),
            AccountId::from("ACCOUNT-001"),
            InstrumentId::from(instrument),
            venue_position_id.map(PositionId::from),
        )
        .expect("lease should acquire")
    }

    fn report(
        instrument: &str,
        quantity: &str,
        ts_last: u64,
        report_id: Option<UUID4>,
        venue_position_id: Option<&str>,
    ) -> PositionStatusReport {
        PositionStatusReport::new(
            AccountId::from("ACCOUNT-001"),
            InstrumentId::from(instrument),
            PositionSideSpecified::Long,
            Quantity::from(quantity),
            UnixNanos::from(ts_last),
            UnixNanos::from(ts_last),
            report_id,
            venue_position_id.map(PositionId::from),
            None,
        )
    }

    #[test]
    fn reports_without_a_lease_are_discarded_and_last_drop_deletes_authority() {
        let feed = feed();
        let unmatched_key = key("YES.POLYMARKET", None);
        assert_eq!(
            feed.observe(&report("YES.POLYMARKET", "10", 1, None, None))
                .expect("unleased report should produce typed health"),
            BoltV3PositionAuthorityObservationOutcome::NoActiveLease(unmatched_key.clone())
        );
        assert_eq!(feed.active_key_count(), 0);

        let lease = acquire(&feed, "YES.POLYMARKET", None);
        assert_eq!(
            feed.observe(&report("YES.POLYMARKET", "10", 5, None, None))
                .expect("leased report should be observed"),
            BoltV3PositionAuthorityObservationOutcome::Applied
        );
        assert_eq!(
            feed.observe(&report("YES.POLYMARKET", "9", 4, None, None))
                .expect("stale report should be health only"),
            BoltV3PositionAuthorityObservationOutcome::Applied
        );
        assert!(matches!(
            lease.state().unwrap(),
            BoltV3PositionAuthorityLeaseState::Coherent(_)
        ));
        assert!(lease.stale_health().unwrap().is_some());
        assert_eq!(
            feed.observe(&report("NO.POLYMARKET", "3", 6, None, None))
                .expect("a different unleased key should be typed and transient"),
            BoltV3PositionAuthorityObservationOutcome::NoActiveLease(key("NO.POLYMARKET", None))
        );
        assert_eq!(feed.active_key_count(), 1);
        drop(lease);
        assert_eq!(feed.active_key_count(), 0);
    }

    #[test]
    fn distinct_instruments_under_one_account_never_share_or_conflict() {
        let feed = feed();
        let yes = acquire(&feed, "YES.POLYMARKET", None);
        let no = acquire(&feed, "NO.POLYMARKET", None);

        feed.observe(&report("YES.POLYMARKET", "10", 7, None, None))
            .unwrap();
        feed.observe(&report("NO.POLYMARKET", "3", 7, None, None))
            .unwrap();

        let BoltV3PositionAuthorityLeaseState::Coherent(yes) = yes.state().unwrap() else {
            panic!("YES must remain coherent");
        };
        let BoltV3PositionAuthorityLeaseState::Coherent(no) = no.state().unwrap() else {
            panic!("NO must remain coherent");
        };
        assert_eq!(yes.signed_quantity, Decimal::new(10, 0));
        assert_eq!(no.signed_quantity, Decimal::new(3, 0));
    }

    #[test]
    fn generated_report_ids_advance_but_reused_changed_identity_conflicts() {
        let feed = feed();
        let lease = acquire(&feed, "YES.POLYMARKET", None);
        let first = report("YES.POLYMARKET", "10", 1, None, None);
        let second = report("YES.POLYMARKET", "9", 2, None, None);
        assert_ne!(first.report_id, second.report_id);
        feed.observe(&first).unwrap();
        feed.observe(&second).unwrap();
        let BoltV3PositionAuthorityLeaseState::Coherent(snapshot) = lease.state().unwrap() else {
            panic!("fresh generated IDs should advance coherently");
        };
        assert_eq!(snapshot.generation, 2);

        let conflicting = report("YES.POLYMARKET", "8", 3, Some(second.report_id), None);
        feed.observe(&conflicting).unwrap();
        assert_eq!(
            lease.state().unwrap(),
            BoltV3PositionAuthorityLeaseState::Conflicted(
                BoltV3PositionAuthorityConflict::SameReportIdentityChanged
            )
        );
    }

    #[test]
    fn stale_observation_is_health_only_and_generation_overflow_conflicts() {
        let feed = feed();
        let authority_key = key("YES.POLYMARKET", None);
        let lease = acquire(&feed, "YES.POLYMARKET", None);
        feed.observe(&report("YES.POLYMARKET", "10", 5, None, None))
            .unwrap();
        feed.observe(&report("YES.POLYMARKET", "9", 4, None, None))
            .unwrap();
        let BoltV3PositionAuthorityLeaseState::Coherent(snapshot) = lease.state().unwrap() else {
            panic!("stale report must not replace authority");
        };
        assert_eq!(snapshot.signed_quantity, Decimal::new(10, 0));
        assert_eq!(snapshot.generation, 1);
        assert_eq!(
            lease.stale_health().unwrap(),
            Some(BoltV3PositionAuthorityStaleHealth {
                observed_ts_last: UnixNanos::from(4_u64),
                current_ts_last: UnixNanos::from(5_u64),
            })
        );

        feed.set_generation_for_test(&authority_key, u64::MAX);
        feed.observe(&report("YES.POLYMARKET", "8", 6, None, None))
            .unwrap();
        assert_eq!(
            lease.state().unwrap(),
            BoltV3PositionAuthorityLeaseState::Conflicted(
                BoltV3PositionAuthorityConflict::GenerationOverflow
            )
        );
    }

    #[test]
    fn hedging_position_ids_are_exact_independent_keys() {
        let feed = feed();
        let first = acquire(&feed, "YES.POLYMARKET", Some("POSITION-A"));
        let second = acquire(&feed, "YES.POLYMARKET", Some("POSITION-B"));
        feed.observe(&report("YES.POLYMARKET", "4", 1, None, Some("POSITION-A")))
            .unwrap();

        assert!(matches!(
            first.state().unwrap(),
            BoltV3PositionAuthorityLeaseState::Coherent(_)
        ));
        assert_eq!(
            second.state().unwrap(),
            BoltV3PositionAuthorityLeaseState::Awaiting
        );
    }

    #[test]
    fn account_attribution_must_be_unambiguous() {
        let result = BoltV3PositionAuthorityFeed::try_new_with_cache(
            [
                (
                    AccountId::from("ACCOUNT-001"),
                    ClientId::from("execution-client-a"),
                    Venue::from("POLYMARKET"),
                ),
                (
                    AccountId::from("ACCOUNT-001"),
                    ClientId::from("execution-client-b"),
                    Venue::from("POLYMARKET"),
                ),
            ],
            Rc::new(RefCell::new(Cache::default())),
        );
        let Err(error) = result else {
            panic!("one account cannot select two execution clients");
        };
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn subscription_surfaces_unmatched_lease_as_operator_telemetry() {
        const CASE: &str = "position-authority-unmatched-lease-telemetry";
        if !crate::bolt_v3_test_log_capture::enter_isolated_log_capture(
            "bolt_v3_position_authority_feed::tests::subscription_surfaces_unmatched_lease_as_operator_telemetry",
            CASE,
        ) {
            return;
        }

        let feed = feed();
        let subscription = feed.subscribe();
        let topic = MessagingSwitchboard::reconciliation_raw_position_status_report_topic();
        let ((), records) = crate::bolt_v3_test_log_capture::with_captured_logs(|| {
            msgbus::publish_any(
                topic.as_str().into(),
                &report("UNMATCHED.POLYMARKET", "2", 1, None, None),
            );
        });

        assert!(records.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("position authority report dropped without an active lease")
                && message.contains("instrument_id=UNMATCHED.POLYMARKET")
        }));
        assert_eq!(feed.active_key_count(), 0);
        drop(subscription);
    }

    #[test]
    fn subscription_drop_and_restart_leave_one_active_report_handler() {
        let feed = feed();
        let lease = acquire(&feed, "RESTART.POLYMARKET", None);
        let topic = MessagingSwitchboard::reconciliation_raw_position_status_report_topic();

        let first_subscription = feed.subscribe();
        let first = report("RESTART.POLYMARKET", "10", 1, None, None);
        msgbus::publish_any(topic.as_str().into(), &first);
        let BoltV3PositionAuthorityLeaseState::Coherent(first_snapshot) = lease.state().unwrap()
        else {
            panic!("the initial subscription must observe its report");
        };
        assert_eq!(first_snapshot.generation, 1);

        drop(first_subscription);
        let while_stopped = report("RESTART.POLYMARKET", "9", 2, None, None);
        msgbus::publish_any(topic.as_str().into(), &while_stopped);
        let BoltV3PositionAuthorityLeaseState::Coherent(stopped_snapshot) = lease.state().unwrap()
        else {
            panic!("dropping the subscription must preserve prior authority");
        };
        assert_eq!(stopped_snapshot.generation, 1);
        assert_eq!(stopped_snapshot.signed_quantity, Decimal::new(10, 0));

        let restarted_subscription = feed.subscribe();
        let restarted = report("RESTART.POLYMARKET", "8", 3, None, None);
        msgbus::publish_any(topic.as_str().into(), &restarted);
        let BoltV3PositionAuthorityLeaseState::Coherent(restarted_snapshot) =
            lease.state().unwrap()
        else {
            panic!("the restarted subscription must observe its report");
        };
        assert_eq!(
            restarted_snapshot.generation, 2,
            "one restarted publish must be observed exactly once"
        );
        assert_eq!(restarted_snapshot.signed_quantity, Decimal::new(8, 0));
        drop(restarted_subscription);
    }
}
