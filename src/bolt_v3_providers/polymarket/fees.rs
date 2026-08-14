use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::{
    enums::LiquiditySide,
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};
use nautilus_polymarket::execution::parse::{
    compute_commission, instrument_fee_exponent, instrument_taker_fee,
};
use nautilus_polymarket::{common::consts::POLYMARKET, http::clob::PolymarketClobHttpClient};
use rust_decimal::Decimal;

use crate::bolt_v3_providers::FeeProvider;

const ENTRY_FEE_BPS_SCALE: i64 = 10_000;
const NT_COMMISSION_ROUNDING_DECIMAL_PLACES: u32 = 5;
const NT_COMMISSION_ROUNDING_RATE_MULTIPLIER: i64 = 2;

trait FeeRateFetcher: Send + Sync {
    fn fetch_fee_bps<'a>(&'a self, token_id: &'a str) -> BoxFuture<'a, Result<Decimal>>;
}

#[derive(Clone)]
struct ClobFeeRateFetcher {
    client: PolymarketClobHttpClient,
}

impl FeeRateFetcher for ClobFeeRateFetcher {
    fn fetch_fee_bps<'a>(&'a self, token_id: &'a str) -> BoxFuture<'a, Result<Decimal>> {
        async move {
            self.client
                .get_fee_rate(token_id)
                .await
                .map(|response| response.base_fee)
                .map_err(anyhow::Error::from)
        }
        .boxed()
    }
}

#[derive(Clone, Debug)]
struct FeeCacheEntry {
    fee_bps: Decimal,
    fetched_at: Instant,
}

#[derive(Clone)]
pub struct PolymarketClobFeeProvider {
    fetcher: Arc<dyn FeeRateFetcher>,
    cache: Arc<Mutex<HashMap<InstrumentId, FeeCacheEntry>>>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    ttl: Duration,
}

fn clob_token_id_from_instrument_id(instrument_id: InstrumentId) -> Result<String> {
    anyhow::ensure!(
        instrument_id.venue.as_str() == POLYMARKET,
        "Polymarket fee lookup requires venue `{POLYMARKET}`, got `{}`",
        instrument_id.venue
    );
    let raw_symbol = instrument_id.symbol.as_str();
    let (_, token_id) = raw_symbol.rsplit_once('-').ok_or_else(|| {
        anyhow::anyhow!("Polymarket fee lookup requires symbol shape `<condition_id>-<token_id>`")
    })?;
    anyhow::ensure!(
        !token_id.is_empty(),
        "Polymarket fee lookup requires a non-empty token_id in instrument_id"
    );
    Ok(token_id.to_string())
}

impl std::fmt::Debug for PolymarketClobFeeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketClobFeeProvider")
            .finish_non_exhaustive()
    }
}

impl PolymarketClobFeeProvider {
    pub fn new(client: PolymarketClobHttpClient, ttl: Duration) -> Self {
        Self {
            fetcher: Arc::new(ClobFeeRateFetcher { client }),
            cache: Arc::new(Mutex::new(HashMap::new())),
            now: Arc::new(Instant::now),
            ttl,
        }
    }

    #[cfg(test)]
    fn new_for_tests(
        fetcher: Arc<dyn FeeRateFetcher>,
        now: Arc<dyn Fn() -> Instant + Send + Sync>,
        ttl: Duration,
    ) -> Self {
        Self {
            fetcher,
            cache: Arc::new(Mutex::new(HashMap::new())),
            now,
            ttl,
        }
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<InstrumentId, FeeCacheEntry>> {
        self.cache
            .lock()
            .expect("Polymarket fee cache mutex poisoned")
    }

    fn entry_within_ttl(&self, entry: &FeeCacheEntry, now: Instant) -> bool {
        now.checked_duration_since(entry.fetched_at)
            .is_some_and(|elapsed| elapsed < self.ttl)
    }

    fn retain_entries_within_ttl(
        &self,
        cache: &mut HashMap<InstrumentId, FeeCacheEntry>,
        now: Instant,
    ) {
        cache.retain(|_, entry| self.entry_within_ttl(entry, now));
    }

    fn warm_inner(&self, instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async move {
            let token_id = clob_token_id_from_instrument_id(instrument_id)?;
            let now = (self.now)();
            {
                let mut cache = self.cache();
                self.retain_entries_within_ttl(&mut cache, now);
                if cache.contains_key(&instrument_id) {
                    return Ok(());
                }
            }

            match self.fetcher.fetch_fee_bps(&token_id).await {
                Ok(fee_bps) => {
                    let fetched_at = (self.now)();
                    self.cache().insert(
                        instrument_id,
                        FeeCacheEntry {
                            fee_bps,
                            fetched_at,
                        },
                    );
                    Ok(())
                }
                Err(error) => Err(error).context(format!(
                    "failed to warm fee rate for instrument {instrument_id}"
                )),
            }
        }
        .boxed()
    }
}

impl FeeProvider for PolymarketClobFeeProvider {
    fn fee_bps(&self, instrument_id: InstrumentId) -> Option<Decimal> {
        let now = (self.now)();
        let mut cache = self.cache();
        let entry = cache.get(&instrument_id)?;
        if self.entry_within_ttl(entry, now) {
            Some(entry.fee_bps)
        } else {
            cache.remove(&instrument_id);
            None
        }
    }

    fn entry_fee_bps(&self, instrument: &InstrumentAny, entry_price: Decimal) -> Option<Decimal> {
        if entry_price <= Decimal::ZERO || entry_price >= Decimal::ONE {
            return None;
        }
        let fee_exponent = instrument_fee_exponent(instrument);
        if !fee_exponent.is_finite() || fee_exponent <= 0.0 {
            return None;
        }
        let fee_rate = instrument_taker_fee(instrument);
        if fee_rate.is_sign_negative() {
            return None;
        }
        let commission = compute_commission(
            fee_rate,
            fee_exponent,
            Decimal::ONE,
            entry_price,
            LiquiditySide::Taker,
        );
        if commission.is_sign_negative() {
            return None;
        }
        commission
            .checked_div(entry_price)?
            .checked_mul(Decimal::from(ENTRY_FEE_BPS_SCALE))
    }

    fn max_entry_fee_bps(
        &self,
        instrument: &InstrumentAny,
        entry_price: Decimal,
    ) -> Option<Decimal> {
        let min_price = instrument.min_price()?.as_decimal();
        let max_price = instrument.max_price()?.as_decimal();
        if min_price <= Decimal::ZERO
            || max_price >= Decimal::ONE
            || min_price > max_price
            || entry_price < min_price
            || entry_price > max_price
        {
            return None;
        }
        let fee_exponent = instrument_fee_exponent(instrument);
        if !fee_exponent.is_finite() || fee_exponent <= 0.0 {
            return None;
        }
        let fee_rate = instrument_taker_fee(instrument);
        if fee_rate.is_sign_negative() {
            return None;
        }
        let raw_rate_bound = fee_rate.checked_mul(Decimal::from(ENTRY_FEE_BPS_SCALE))?;
        let mut sampled_bound = Decimal::ZERO;
        for price in [min_price, entry_price, max_price] {
            sampled_bound = sampled_bound.max(self.entry_fee_bps(instrument, price)?);
        }
        if fee_exponent > 1.0 {
            let critical_price =
                Decimal::try_from((fee_exponent - 1.0) / (2.0 * fee_exponent - 1.0)).ok()?;
            if critical_price >= min_price && critical_price <= max_price {
                sampled_bound = sampled_bound.max(self.entry_fee_bps(instrument, critical_price)?);
            }
        }
        // NT rounds commission to five decimal places before converting it to basis points.
        // One full quantum at the minimum price safely covers both a sampled round-down at
        // the continuous maximum and a round-up elsewhere on the tradable price range.
        let rounding_uplift_bps = Decimal::new(1, NT_COMMISSION_ROUNDING_DECIMAL_PLACES)
            .checked_div(min_price)?
            .checked_mul(Decimal::from(ENTRY_FEE_BPS_SCALE))?;
        let rounded_curve_bound = sampled_bound.checked_add(rounding_uplift_bps)?;
        // Any non-zero rounded commission had an unrounded value of at least half a
        // quantum, so rounding can amplify its effective rate by at most two. This
        // quantity-independent factor also covers arbitrarily small partial fills.
        raw_rate_bound
            .max(rounded_curve_bound)
            .checked_mul(Decimal::from(NT_COMMISSION_ROUNDING_RATE_MULTIPLIER))
    }

    fn warm(&self, instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        self.warm_inner(instrument_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        panic::{AssertUnwindSafe, catch_unwind},
    };

    use crate::bolt_v3_config::BoltV3RootConfig;
    use nautilus_core::{Params, UnixNanos};
    use nautilus_model::{
        enums::AssetClass,
        identifiers::Symbol,
        instruments::{BinaryOption, InstrumentAny},
        types::{Currency, Price, Quantity},
    };
    use rust_decimal::prelude::ToPrimitive;

    fn decimal(input: &str) -> Decimal {
        input.parse().expect("decimal literal should parse")
    }

    fn test_fee_cache_ttl() -> Duration {
        let root: BoltV3RootConfig =
            toml::from_str(include_str!("../../../tests/fixtures/bolt_v3/root.toml"))
                .expect("bolt-v3 root fixture should parse");
        let seconds = root.clients["polymarket_main"]
            .execution
            .clone()
            .expect("fixture should define polymarket execution")
            .try_into::<crate::bolt_v3_providers::polymarket::PolymarketExecutionConfig>()
            .expect("fixture polymarket execution should parse")
            .fee_cache_ttl_secs;
        Duration::from_secs(seconds)
    }

    fn instrument_id_for_token(token_id: &str) -> InstrumentId {
        InstrumentId::from(format!("0xcondition-{token_id}.POLYMARKET").as_str())
    }

    fn binary_option_with_taker_fee(
        instrument_id: InstrumentId,
        taker_fee: Decimal,
    ) -> InstrumentAny {
        binary_option_with_taker_fee_and_exponent(instrument_id, taker_fee, None)
    }

    fn binary_option_with_taker_fee_and_exponent(
        instrument_id: InstrumentId,
        taker_fee: Decimal,
        fee_exponent: Option<f64>,
    ) -> InstrumentAny {
        let ts = UnixNanos::from(1_000_000_000);
        let mut info = Params::new();
        if let Some(fee_exponent) = fee_exponent {
            info.insert(
                "fee_schedule".to_string(),
                serde_json::json!({ "exponent": fee_exponent }),
            );
        }
        InstrumentAny::BinaryOption(BinaryOption::new(
            instrument_id,
            Symbol::from("0xcondition-token_a"),
            AssetClass::Alternative,
            Currency::USDC(),
            ts,
            UnixNanos::from(2_000_000_000),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Price::from("0.999")),
            Some(Price::from("0.001")),
            None,
            None,
            Some(Decimal::ZERO),
            Some(taker_fee),
            None,
            Some(info),
            ts,
            ts,
        ))
    }

    #[test]
    fn clob_token_id_uses_nt_polymarket_symbol_suffix() {
        let instrument_id = InstrumentId::from("0xconfigured-market-with-dash-12345.POLYMARKET");

        assert_eq!(
            clob_token_id_from_instrument_id(instrument_id)
                .expect("valid Polymarket instrument should expose CLOB token id"),
            "12345"
        );
    }

    #[test]
    fn clob_token_id_rejects_non_polymarket_venue() {
        let instrument_id = InstrumentId::from("0xconfigured-market-12345.SOURCE");

        let error = clob_token_id_from_instrument_id(instrument_id)
            .expect_err("fee lookup should reject non-Polymarket instruments");

        assert!(error.to_string().contains("requires venue `POLYMARKET`"));
    }

    #[test]
    fn entry_fee_bps_uses_nt_commission_formula_not_cached_base_fee_rate() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("1000"))]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_a");
        let instrument = binary_option_with_taker_fee(instrument_id, decimal("0.07"));

        let fee_bps = provider
            .entry_fee_bps(&instrument, decimal("0.27"))
            .expect("entry fee bps should be derived from NT commission");

        let fee_bps = fee_bps
            .to_f64()
            .expect("entry fee bps should fit in f64 for assertion");
        assert!((fee_bps - 511.111111111111).abs() < 1e-9);
    }

    #[test]
    fn entry_fee_bps_uses_instrument_fee_schedule_exponent() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(Vec::new());
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_exponent");
        let instrument =
            binary_option_with_taker_fee_and_exponent(instrument_id, decimal("0.07"), Some(2.0));

        let fee_bps = provider
            .entry_fee_bps(&instrument, decimal("0.27"))
            .expect("entry fee bps should use the instrument fee exponent")
            .to_f64()
            .expect("entry fee bps should fit in f64 for assertion");

        assert!((fee_bps - 100.74074074074075).abs() < 1e-9);
    }

    #[test]
    fn max_entry_fee_bps_uses_raw_nt_fee_rate_as_cash_debit_floor() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("1000"))]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_a");
        let instrument = binary_option_with_taker_fee(instrument_id, decimal("0.07"));

        let bound = provider
            .max_entry_fee_bps(&instrument, decimal("0.27"))
            .expect("fee bound should be computable");

        assert!(bound >= decimal("700.00"), "bound={bound}");
    }

    #[test]
    fn max_entry_fee_bps_covers_fractional_exponent_over_tradable_range() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(Vec::new());
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_fractional_exponent");
        let instrument =
            binary_option_with_taker_fee_and_exponent(instrument_id, decimal("0.03"), Some(0.5));
        let entry_price = decimal("0.01");

        let actual = provider
            .entry_fee_bps(&instrument, entry_price)
            .expect("fractional-exponent entry fee should be computable");
        let bound = provider
            .max_entry_fee_bps(&instrument, entry_price)
            .expect("fractional-exponent entry fee bound should be computable");
        let structural_minimum_fee = provider
            .entry_fee_bps(&instrument, decimal("0.001"))
            .expect("fee at the structural minimum should be computable");

        assert!(bound >= actual, "bound={bound} actual={actual}");
        assert!(
            bound >= structural_minimum_fee,
            "bound={bound} structural_minimum_fee={structural_minimum_fee}"
        );
        assert!(bound > decimal("300"), "raw rate alone is not a bound");
    }

    #[test]
    fn max_entry_fee_bps_covers_commission_rounding_spikes() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(Vec::new());
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_rounding_spike");
        let instrument =
            binary_option_with_taker_fee_and_exponent(instrument_id, decimal("0.0001"), Some(0.5));
        let bound = provider
            .max_entry_fee_bps(&instrument, decimal("0.27"))
            .expect("fractional-exponent entry fee bound should be computable");

        for tick in 1..=999 {
            let price = Decimal::new(tick, 3);
            let actual = provider
                .entry_fee_bps(&instrument, price)
                .expect("fee on the configured price grid should be computable");
            assert!(
                bound >= actual,
                "price={price} bound={bound} actual={actual}"
            );
        }
    }

    #[test]
    fn max_entry_fee_bps_covers_small_partial_fill_rounding() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(Vec::new());
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_small_partial_fill");
        let instrument = binary_option_with_taker_fee(instrument_id, decimal("0.07"));
        let price = decimal("0.001");
        let size = decimal("0.072");
        let bound = provider
            .max_entry_fee_bps(&instrument, price)
            .expect("fee bound should be computable");
        let actual = compute_commission(
            instrument_taker_fee(&instrument),
            instrument_fee_exponent(&instrument),
            size,
            price,
            LiquiditySide::Taker,
        ) / (size * price)
            * Decimal::from(ENTRY_FEE_BPS_SCALE);

        assert!(bound >= actual, "bound={bound} actual={actual}");
    }

    #[test]
    fn fee_bounds_fail_closed_on_decimal_overflow() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(Vec::new());
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_decimal_overflow");
        let instrument = binary_option_with_taker_fee(instrument_id, Decimal::MAX);

        assert_eq!(provider.entry_fee_bps(&instrument, decimal("0.50")), None);
        assert_eq!(
            provider.max_entry_fee_bps(&instrument, decimal("0.50")),
            None
        );
    }

    #[derive(Clone)]
    struct TestClock {
        now: Arc<Mutex<Instant>>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                now: Arc::new(Mutex::new(Instant::now())),
            }
        }

        fn source(&self) -> Arc<dyn Fn() -> Instant + Send + Sync> {
            let now = Arc::clone(&self.now);
            Arc::new(move || *now.lock().expect("clock mutex poisoned"))
        }

        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().expect("clock mutex poisoned");
            *now += duration;
        }

        fn rewind(&self, duration: Duration) {
            let mut now = self.now.lock().expect("clock mutex poisoned");
            *now -= duration;
        }
    }

    #[derive(Clone)]
    enum MockFetchResult {
        Success(Decimal),
        Failure(&'static str),
    }

    #[derive(Clone)]
    struct MockFeeRateFetcher {
        results: Arc<Mutex<VecDeque<MockFetchResult>>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl MockFeeRateFetcher {
        fn new(results: Vec<MockFetchResult>) -> Self {
            Self {
                results: Arc::new(Mutex::new(results.into())),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().expect("calls mutex poisoned").len()
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls mutex poisoned").clone()
        }
    }

    impl FeeRateFetcher for MockFeeRateFetcher {
        fn fetch_fee_bps<'a>(&'a self, token_id: &'a str) -> BoxFuture<'a, Result<Decimal>> {
            let result = self
                .results
                .lock()
                .expect("results mutex poisoned")
                .pop_front()
                .expect("mock fetch result should exist");
            self.calls
                .lock()
                .expect("calls mutex poisoned")
                .push(token_id.to_string());

            async move {
                match result {
                    MockFetchResult::Success(value) => Ok(value),
                    MockFetchResult::Failure(message) => anyhow::bail!(message),
                }
            }
            .boxed()
        }
    }

    #[test]
    #[should_panic(expected = "Polymarket fee cache mutex poisoned")]
    fn fee_provider_fee_bps_panics_on_poisoned_cache() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(Vec::new());
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_poison");
        let fetched_at = *clock.now.lock().expect("clock mutex poisoned");
        provider.cache().insert(
            instrument_id,
            FeeCacheEntry {
                fee_bps: decimal("5.00"),
                fetched_at,
            },
        );
        let cache = provider.cache.clone();
        let poison_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = cache.lock().unwrap();
            panic!("poison fee cache");
        }));
        assert!(poison_result.is_err());
        assert!(cache.lock().is_err());

        provider.fee_bps(instrument_id);
    }

    #[derive(Clone)]
    struct AdvancingFeeRateFetcher {
        clock: TestClock,
        advance_by: Duration,
        fee_bps: Decimal,
    }

    impl FeeRateFetcher for AdvancingFeeRateFetcher {
        fn fetch_fee_bps<'a>(&'a self, _token_id: &'a str) -> BoxFuture<'a, Result<Decimal>> {
            self.clock.advance(self.advance_by);
            let fee_bps = self.fee_bps;

            async move { Ok(fee_bps) }.boxed()
        }
    }

    #[tokio::test]
    async fn fee_provider_cold_miss_fetches_and_caches() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("1.75"))]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_a");

        assert_eq!(provider.fee_bps(instrument_id), None);
        provider
            .warm(instrument_id)
            .await
            .expect("warm should succeed");

        assert_eq!(provider.fee_bps(instrument_id), Some(decimal("1.75")));
        assert_eq!(fetcher.calls(), vec!["token_a".to_string()]);
        assert_eq!(fetcher.call_count(), 1);
    }

    #[tokio::test]
    async fn fee_provider_cache_hit_within_ttl_skips_refresh() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("2.50"))]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_b");

        provider
            .warm(instrument_id)
            .await
            .expect("first warm should succeed");
        provider
            .warm(instrument_id)
            .await
            .expect("second warm should reuse cache");

        assert_eq!(provider.fee_bps(instrument_id), Some(decimal("2.50")));
        assert_eq!(fetcher.calls(), vec!["token_b".to_string()]);
        assert_eq!(fetcher.call_count(), 1);
    }

    #[tokio::test]
    async fn fee_provider_expired_cache_does_not_return_fee() {
        let clock = TestClock::new();
        let ttl = test_fee_cache_ttl();
        let fetcher = MockFeeRateFetcher::new(vec![
            MockFetchResult::Success(decimal("3.10")),
            MockFetchResult::Failure("refresh down"),
        ]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            ttl,
        );
        let instrument_id = instrument_id_for_token("token_c");

        provider
            .warm(instrument_id)
            .await
            .expect("initial warm should succeed");
        clock.advance(ttl + Duration::from_secs(1));

        assert_eq!(provider.fee_bps(instrument_id), None);
        let error = provider
            .warm(instrument_id)
            .await
            .expect_err("expired cache refresh failure should error");

        assert!(
            error
                .to_string()
                .contains(instrument_id.to_string().as_str())
        );
        assert_eq!(provider.fee_bps(instrument_id), None);
        assert_eq!(
            fetcher.calls(),
            vec!["token_c".to_string(), "token_c".to_string()]
        );
        assert_eq!(fetcher.call_count(), 2);
    }

    #[tokio::test]
    async fn fee_provider_refresh_after_ttl_replaces_cached_fee() {
        let clock = TestClock::new();
        let ttl = test_fee_cache_ttl();
        let fetcher = MockFeeRateFetcher::new(vec![
            MockFetchResult::Success(decimal("3.10")),
            MockFetchResult::Success(decimal("3.20")),
        ]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            ttl,
        );
        let instrument_id = instrument_id_for_token("token_e");

        provider
            .warm(instrument_id)
            .await
            .expect("initial warm should succeed");
        assert_eq!(provider.fee_bps(instrument_id), Some(decimal("3.10")));

        clock.advance(ttl + Duration::from_secs(1));
        assert_eq!(provider.fee_bps(instrument_id), None);

        provider
            .warm(instrument_id)
            .await
            .expect("refresh after ttl should succeed");

        assert_eq!(provider.fee_bps(instrument_id), Some(decimal("3.20")));
        assert_eq!(
            fetcher.calls(),
            vec!["token_e".to_string(), "token_e".to_string()]
        );
        assert_eq!(fetcher.call_count(), 2);
    }

    #[tokio::test]
    async fn fee_provider_ttl_starts_after_fetch_completes() {
        let clock = TestClock::new();
        let ttl = test_fee_cache_ttl();
        let fetcher = AdvancingFeeRateFetcher {
            clock: clock.clone(),
            advance_by: ttl / 2,
            fee_bps: decimal("4.10"),
        };
        let provider =
            PolymarketClobFeeProvider::new_for_tests(Arc::new(fetcher), clock.source(), ttl);
        let instrument_id = instrument_id_for_token("token_f");

        provider
            .warm(instrument_id)
            .await
            .expect("warm should succeed");
        clock.advance(ttl / 2);

        assert_eq!(provider.fee_bps(instrument_id), Some(decimal("4.10")));
    }

    #[tokio::test]
    async fn fee_provider_fee_bps_removes_expired_entry() {
        let clock = TestClock::new();
        let ttl = test_fee_cache_ttl();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("4.20"))]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            ttl,
        );
        let instrument_id = instrument_id_for_token("token_g");

        provider
            .warm(instrument_id)
            .await
            .expect("warm should succeed");
        clock.advance(ttl + Duration::from_secs(1));

        assert_eq!(provider.fee_bps(instrument_id), None);
        assert!(!provider.cache().contains_key(&instrument_id));
    }

    #[tokio::test]
    async fn fee_provider_warm_removes_expired_entries() {
        let clock = TestClock::new();
        let ttl = test_fee_cache_ttl();
        let fetcher = MockFeeRateFetcher::new(vec![
            MockFetchResult::Success(decimal("4.30")),
            MockFetchResult::Success(decimal("4.40")),
        ]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            ttl,
        );
        let instrument_h = instrument_id_for_token("token_h");
        let instrument_i = instrument_id_for_token("token_i");

        provider
            .warm(instrument_h)
            .await
            .expect("first warm should succeed");
        clock.advance(ttl + Duration::from_secs(1));
        provider
            .warm(instrument_i)
            .await
            .expect("second warm should succeed");

        assert!(!provider.cache().contains_key(&instrument_h));
        assert_eq!(provider.fee_bps(instrument_i), Some(decimal("4.40")));
        assert_eq!(
            fetcher.calls(),
            vec!["token_h".to_string(), "token_i".to_string()]
        );
    }

    #[tokio::test]
    async fn fee_provider_clock_before_fetched_at_returns_none() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("4.50"))]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_j");

        provider
            .warm(instrument_id)
            .await
            .expect("warm should succeed");
        clock.rewind(Duration::from_secs(1));

        assert_eq!(provider.fee_bps(instrument_id), None);
    }

    #[tokio::test]
    async fn fee_provider_cold_miss_failure_stays_empty() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Failure("network down")]);
        let provider = PolymarketClobFeeProvider::new_for_tests(
            Arc::new(fetcher.clone()),
            clock.source(),
            test_fee_cache_ttl(),
        );
        let instrument_id = instrument_id_for_token("token_d");

        let error = provider
            .warm(instrument_id)
            .await
            .expect_err("cold miss failure should error");

        assert!(
            error
                .to_string()
                .contains(instrument_id.to_string().as_str())
        );
        assert_eq!(provider.fee_bps(instrument_id), None);
        assert_eq!(fetcher.calls(), vec!["token_d".to_string()]);
        assert_eq!(fetcher.call_count(), 1);
    }
}
