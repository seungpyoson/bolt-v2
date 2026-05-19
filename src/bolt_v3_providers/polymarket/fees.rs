use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::identifiers::InstrumentId;
use nautilus_polymarket::{common::consts::POLYMARKET, http::clob::PolymarketClobHttpClient};
use rust_decimal::Decimal;

use crate::strategies::registry::FeeProvider;

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
        self.cache.lock().unwrap_or_else(|poisoned| {
            log::warn!("Polymarket fee cache mutex poisoned; recovering cached fee state");
            poisoned.into_inner()
        })
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

    fn warm(&self, instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        self.warm_inner(instrument_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use crate::bolt_v3_config::BoltV3RootConfig;

    fn decimal(input: &str) -> Decimal {
        input.parse().expect("decimal literal should parse")
    }

    fn test_fee_cache_ttl() -> Duration {
        let root: BoltV3RootConfig =
            toml::from_str(include_str!("../../../tests/fixtures/bolt_v3/root.toml"))
                .expect("bolt-v3 root fixture should parse");
        let seconds = root.venues["polymarket_main"]
            .execution
            .clone()
            .expect("fixture should define polymarket execution")
            .try_into::<crate::bolt_v3_providers::polymarket::PolymarketExecutionConfig>()
            .expect("fixture polymarket execution should parse")
            .fee_cache_ttl_seconds;
        Duration::from_secs(seconds)
    }

    fn instrument_id_for_token(token_id: &str) -> InstrumentId {
        InstrumentId::from(format!("0xcondition-{token_id}.POLYMARKET").as_str())
    }

    #[test]
    fn clob_token_id_uses_nt_polymarket_symbol_suffix() {
        let instrument_id = InstrumentId::from("0xcondition-with-dash-12345.POLYMARKET");

        assert_eq!(
            clob_token_id_from_instrument_id(instrument_id)
                .expect("valid Polymarket instrument should expose CLOB token id"),
            "12345"
        );
    }

    #[test]
    fn clob_token_id_rejects_non_polymarket_venue() {
        let instrument_id = InstrumentId::from("0xcondition-12345.BINANCE");

        let error = clob_token_id_from_instrument_id(instrument_id)
            .expect_err("fee lookup should reject non-Polymarket instruments");

        assert!(error.to_string().contains("requires venue `POLYMARKET`"));
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
