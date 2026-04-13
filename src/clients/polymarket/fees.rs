use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_polymarket::http::clob::PolymarketClobHttpClient;
use rust_decimal::Decimal;

const FEE_TTL: Duration = Duration::from_secs(300);

pub trait FeeProvider: Send + Sync {
    fn fee_bps(&self, token_id: &str) -> Option<Decimal>;
    fn warm(&self, token_id: &str) -> BoxFuture<'_, Result<()>>;
}

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
    cache: Arc<Mutex<HashMap<String, FeeCacheEntry>>>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl std::fmt::Debug for PolymarketClobFeeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketClobFeeProvider")
            .finish_non_exhaustive()
    }
}

impl PolymarketClobFeeProvider {
    pub fn new(client: PolymarketClobHttpClient) -> Self {
        Self {
            fetcher: Arc::new(ClobFeeRateFetcher { client }),
            cache: Arc::new(Mutex::new(HashMap::new())),
            now: Arc::new(Instant::now),
        }
    }

    fn warm_inner(&self, token_id: String) -> BoxFuture<'_, Result<()>> {
        async move {
            let now = (self.now)();
            {
                let cache = self.cache.lock().expect("fee cache mutex poisoned");
                if let Some(entry) = cache.get(&token_id)
                    && now.duration_since(entry.fetched_at) < FEE_TTL
                {
                    return Ok(());
                }
            }

            match self.fetcher.fetch_fee_bps(&token_id).await {
                Ok(fee_bps) => {
                    self.cache.lock().expect("fee cache mutex poisoned").insert(
                        token_id,
                        FeeCacheEntry {
                            fee_bps,
                            fetched_at: now,
                        },
                    );
                    Ok(())
                }
                Err(error) => {
                    let mut cache = self.cache.lock().expect("fee cache mutex poisoned");
                    if let Some(entry) = cache.get_mut(&token_id) {
                        log::warn!(
                            "fee refresh failed for token {token_id}, using stale cached value: {error}"
                        );
                        entry.fetched_at = now;
                        Ok(())
                    } else {
                        Err(error).context(format!("failed to warm fee rate for token {token_id}"))
                    }
                }
            }
        }
        .boxed()
    }

    #[cfg(test)]
    fn new_for_tests(
        fetcher: Arc<dyn FeeRateFetcher>,
        now: Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            fetcher,
            cache: Arc::new(Mutex::new(HashMap::new())),
            now,
        }
    }
}

impl FeeProvider for PolymarketClobFeeProvider {
    fn fee_bps(&self, token_id: &str) -> Option<Decimal> {
        self.cache
            .lock()
            .expect("fee cache mutex poisoned")
            .get(token_id)
            .map(|entry| entry.fee_bps)
    }

    fn warm(&self, token_id: &str) -> BoxFuture<'_, Result<()>> {
        self.warm_inner(token_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn decimal(input: &str) -> Decimal {
        input.parse().expect("decimal literal should parse")
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

    #[tokio::test]
    async fn fee_provider_cold_miss_fetches_and_caches() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("1.75"))]);
        let provider =
            PolymarketClobFeeProvider::new_for_tests(Arc::new(fetcher.clone()), clock.source());

        assert_eq!(provider.fee_bps("token-a"), None);
        provider.warm("token-a").await.expect("warm should succeed");

        assert_eq!(provider.fee_bps("token-a"), Some(decimal("1.75")));
        assert_eq!(fetcher.call_count(), 1);
    }

    #[tokio::test]
    async fn fee_provider_cache_hit_within_ttl_skips_refresh() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Success(decimal("2.50"))]);
        let provider =
            PolymarketClobFeeProvider::new_for_tests(Arc::new(fetcher.clone()), clock.source());

        provider
            .warm("token-b")
            .await
            .expect("first warm should succeed");
        provider
            .warm("token-b")
            .await
            .expect("second warm should reuse cache");

        assert_eq!(provider.fee_bps("token-b"), Some(decimal("2.50")));
        assert_eq!(fetcher.call_count(), 1);
    }

    #[tokio::test]
    async fn fee_provider_stale_fallback_on_refresh_failure() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![
            MockFetchResult::Success(decimal("3.10")),
            MockFetchResult::Failure("refresh down"),
        ]);
        let provider =
            PolymarketClobFeeProvider::new_for_tests(Arc::new(fetcher.clone()), clock.source());

        provider
            .warm("token-c")
            .await
            .expect("initial warm should succeed");
        clock.advance(FEE_TTL + Duration::from_secs(1));

        provider
            .warm("token-c")
            .await
            .expect("stale fallback should keep cached fee");

        assert_eq!(provider.fee_bps("token-c"), Some(decimal("3.10")));
        assert_eq!(fetcher.call_count(), 2);

        provider
            .warm("token-c")
            .await
            .expect("stale fallback should refresh ttl");
        assert_eq!(fetcher.call_count(), 2);
    }

    #[tokio::test]
    async fn fee_provider_cold_miss_failure_stays_empty() {
        let clock = TestClock::new();
        let fetcher = MockFeeRateFetcher::new(vec![MockFetchResult::Failure("network down")]);
        let provider =
            PolymarketClobFeeProvider::new_for_tests(Arc::new(fetcher.clone()), clock.source());

        let error = provider
            .warm("token-d")
            .await
            .expect_err("cold miss failure should error");

        assert!(error.to_string().contains("token-d"));
        assert_eq!(provider.fee_bps("token-d"), None);
        assert_eq!(fetcher.call_count(), 1);
    }
}
