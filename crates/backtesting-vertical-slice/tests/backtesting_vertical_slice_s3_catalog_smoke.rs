use std::env;
use std::sync::Arc;

use ahash::AHashMap;
use anyhow::{Context, bail};
use chrono::Utc;
use futures_util::TryStreamExt;
use futures_util::future::join_all;
use hmac::{Hmac, Mac};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::{AggressorSide, CurrencyType},
    identifiers::{InstrumentId, Symbol, TradeId, Venue},
    instruments::{CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut};
use object_store::path::Path as ObjectPath;
use object_store::{Error as ObjectStoreError, ObjectStore, ObjectStoreExt, PutMode, PutOptions};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const SMOKE_CONFIG: &str = include_str!("fixtures/s3_catalog_smoke.toml");
const AWS_SIGNING_ALGORITHM: &str = "AWS4-HMAC-SHA256";
const AWS_SIGNING_REQUEST_TYPE: &str = "aws4_request";
const AWS_S3_SERVICE: &str = "s3";
const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SIGNED_HEADERS: &str = "host;x-amz-content-sha256;x-amz-date";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeConfig {
    opt_in: OptInConfig,
    ci: CiConfig,
    ci_minio: CiMinioConfig,
    store: StoreConfig,
    probe: ProbeConfig,
    catalog: CatalogConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OptInConfig {
    env_var: String,
    enabled_value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CiConfig {
    env_var: String,
    required_value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CiMinioConfig {
    image: String,
    container_name: String,
    container_data_dir: String,
    api_port: u16,
    console_port: u16,
    health_path: String,
    readiness_attempts: u16,
    readiness_sleep_seconds: u16,
    readiness_connect_timeout_seconds: u16,
    readiness_max_time_seconds: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreConfig {
    endpoint_url: String,
    region: String,
    bucket: String,
    access_key_id: String,
    secret_access_key: String,
    allow_http: bool,
    virtual_hosted_style_request: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeConfig {
    prefix: String,
    sentinel_key: String,
    race_key: String,
    transcript_key: String,
    writer_count: usize,
    payload_stem: String,
    requirement_ref: String,
    proof_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogConfig {
    prefix: String,
    symbol: String,
    venue: String,
    base_currency: SyntheticCurrencyConfig,
    quote_currency: SyntheticCurrencyConfig,
    price_precision: u8,
    size_precision: u8,
    price_increment: f64,
    size_increment: f64,
    trades: Vec<CatalogTradeConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticCurrencyConfig {
    code: String,
    precision: u8,
    iso4217: u16,
    name: String,
    currency_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogTradeConfig {
    price: f64,
    size: f64,
    aggressor_side: String,
    trade_id: String,
    timestamp_ns: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct NoOverwriteProofTranscript {
    proof_name: String,
    requirement_ref: String,
    store_uri: String,
    conditional_put_probe: ConditionalPutProbeTranscript,
    concurrency_proof: ConcurrencyProofTranscript,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ConditionalPutProbeTranscript {
    first_create_result: String,
    second_create_result: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ConcurrencyProofTranscript {
    claim: String,
    writer_count: usize,
    successful_puts: usize,
    already_exists: usize,
    silent_overwrite_observed: bool,
    winning_writer_index: usize,
    stored_payload_sha256: String,
}

struct WriterOutcome {
    writer_index: usize,
    payload: Vec<u8>,
    result: Result<(), ObjectStoreError>,
}

fn enabled_config() -> Option<SmokeConfig> {
    let config: SmokeConfig = toml::from_str(SMOKE_CONFIG).expect("S3 smoke config parses");
    config
        .ci_minio
        .validate_against_store(&config.store)
        .expect("CI MinIO fixture values are internally consistent");
    match env::var(&config.opt_in.env_var) {
        Ok(value) if value == config.opt_in.enabled_value => Some(config),
        _ => {
            if env::var(&config.ci.env_var).as_deref() == Ok(config.ci.required_value.as_str()) {
                panic!(
                    "{} must be set to {} in CI so MinIO-backed S3 smoke tests cannot silently skip",
                    config.opt_in.env_var, config.opt_in.enabled_value
                );
            }
            eprintln!(
                "skipping MinIO-backed S3 catalog smoke; set {}={} to enable",
                config.opt_in.env_var, config.opt_in.enabled_value
            );
            None
        }
    }
}

fn s3_store(config: &StoreConfig) -> anyhow::Result<AmazonS3> {
    AmazonS3Builder::new()
        .with_bucket_name(&config.bucket)
        .with_endpoint(&config.endpoint_url)
        .with_region(&config.region)
        .with_access_key_id(&config.access_key_id)
        .with_secret_access_key(&config.secret_access_key)
        .with_allow_http(config.allow_http)
        .with_virtual_hosted_style_request(config.virtual_hosted_style_request)
        .with_conditional_put(S3ConditionalPut::ETagMatch)
        .build()
        .context("build MinIO-backed S3 object store")
}

fn storage_options(config: &StoreConfig) -> AHashMap<String, String> {
    let mut options = AHashMap::new();
    options.insert("endpoint_url".to_string(), config.endpoint_url.clone());
    options.insert("region".to_string(), config.region.clone());
    options.insert("access_key_id".to_string(), config.access_key_id.clone());
    options.insert(
        "secret_access_key".to_string(),
        config.secret_access_key.clone(),
    );
    options.insert("allow_http".to_string(), config.allow_http.to_string());
    options.insert(
        "virtual_hosted_style_request".to_string(),
        config.virtual_hosted_style_request.to_string(),
    );
    options
}

async fn prepare_minio(config: &SmokeConfig) -> anyhow::Result<AmazonS3> {
    ensure_bucket(&config.store).await?;
    s3_store(&config.store)
}

async fn ensure_bucket(config: &StoreConfig) -> anyhow::Result<()> {
    let endpoint = config.store_endpoint_without_trailing_slash();
    let bucket_url = format!("{endpoint}/{}", config.bucket);
    let url = reqwest::Url::parse(&bucket_url).context("parse MinIO bucket URL")?;
    let host = host_header(&url)?;
    let now = Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let canonical_uri = format!("/{}", config.bucket);
    let canonical_headers = format!(
        "host:{host}\nx-amz-content-sha256:{EMPTY_PAYLOAD_SHA256}\nx-amz-date:{amz_date}\n"
    );
    let canonical_request = format!(
        "PUT\n{canonical_uri}\n\n{canonical_headers}\n{SIGNED_HEADERS}\n{EMPTY_PAYLOAD_SHA256}"
    );
    let credential_scope = format!(
        "{date_stamp}/{}/{AWS_S3_SERVICE}/{AWS_SIGNING_REQUEST_TYPE}",
        config.region
    );
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign = format!(
        "{AWS_SIGNING_ALGORITHM}\n{amz_date}\n{credential_scope}\n{canonical_request_hash}"
    );
    let signing_key = signing_key(&config.secret_access_key, &date_stamp, &config.region);
    let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());
    let authorization = format!(
        "{AWS_SIGNING_ALGORITHM} Credential={}/{credential_scope}, SignedHeaders={SIGNED_HEADERS}, Signature={signature}",
        config.access_key_id
    );

    let response = reqwest::Client::new()
        .put(bucket_url)
        .header("host", host)
        .header("x-amz-content-sha256", EMPTY_PAYLOAD_SHA256)
        .header("x-amz-date", amz_date)
        .header("authorization", authorization)
        .send()
        .await
        .context("send MinIO CreateBucket request")?;
    let status = response.status();
    if status.is_success() || status == StatusCode::CONFLICT {
        return Ok(());
    }
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("failed to read error body: {error}"));
    bail!("MinIO CreateBucket failed with status {status}: {body}");
}

impl StoreConfig {
    fn store_endpoint_without_trailing_slash(&self) -> &str {
        self.endpoint_url.trim_end_matches('/')
    }
}

impl CiMinioConfig {
    fn validate_against_store(&self, store: &StoreConfig) -> anyhow::Result<()> {
        let endpoint = reqwest::Url::parse(&store.endpoint_url).context("parse store endpoint")?;
        let endpoint_port = endpoint
            .port_or_known_default()
            .context("store endpoint must carry an explicit or known port")?;
        if endpoint_port != self.api_port {
            bail!(
                "store endpoint port {endpoint_port} must match CI MinIO API port {}",
                self.api_port
            );
        }
        if self.console_port == self.api_port {
            bail!("CI MinIO console_port must differ from api_port");
        }
        if self.image.is_empty()
            || self.container_name.is_empty()
            || self.container_data_dir.is_empty()
        {
            bail!("CI MinIO image, container_name, and container_data_dir must be non-empty");
        }
        if !self.health_path.starts_with('/') {
            bail!("CI MinIO health_path must be absolute");
        }
        if self.readiness_attempts == 0 || self.readiness_sleep_seconds == 0 {
            bail!("CI MinIO readiness attempts and sleep seconds must be positive");
        }
        if self.readiness_connect_timeout_seconds == 0 || self.readiness_max_time_seconds == 0 {
            bail!("CI MinIO readiness connect timeout and max time seconds must be positive");
        }
        if self.readiness_max_time_seconds < self.readiness_connect_timeout_seconds {
            bail!(
                "CI MinIO readiness_max_time_seconds {} must be >= readiness_connect_timeout_seconds {}",
                self.readiness_max_time_seconds,
                self.readiness_connect_timeout_seconds
            );
        }
        Ok(())
    }
}

fn host_header(url: &reqwest::Url) -> anyhow::Result<String> {
    let host = url.host_str().context("MinIO bucket URL missing host")?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn signing_key(secret_access_key: &str, date_stamp: &str, region: &str) -> Vec<u8> {
    let date_key = hmac_sha256(
        format!("AWS4{secret_access_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let date_region_key = hmac_sha256(&date_key, region.as_bytes());
    let date_region_service_key = hmac_sha256(&date_region_key, AWS_S3_SERVICE.as_bytes());
    hmac_sha256(
        &date_region_service_key,
        AWS_SIGNING_REQUEST_TYPE.as_bytes(),
    )
}

fn hmac_sha256(key: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha256_hex(key: &[u8], payload: &[u8]) -> String {
    hex::encode(hmac_sha256(key, payload))
}

fn sha256_hex(payload: &[u8]) -> String {
    hex::encode(Sha256::digest(payload))
}

async fn delete_if_exists(store: &AmazonS3, location: &ObjectPath) -> anyhow::Result<()> {
    match store.delete(location).await {
        Ok(()) | Err(ObjectStoreError::NotFound { .. }) => Ok(()),
        Err(error) => Err(error).context("delete MinIO object"),
    }
}

async fn delete_prefix(store: &AmazonS3, prefix: &str) -> anyhow::Result<()> {
    let prefix = ObjectPath::from(prefix);
    let mut objects = store.list(Some(&prefix));
    while let Some(object) = objects.try_next().await.context("list MinIO prefix")? {
        delete_if_exists(store, &object.location).await?;
    }
    Ok(())
}

fn prefixed_path(prefix: &str, key: &str) -> ObjectPath {
    ObjectPath::from(format!("{prefix}/{key}"))
}

async fn assert_conditional_put_probe(
    store: &AmazonS3,
    config: &ProbeConfig,
) -> anyhow::Result<ConditionalPutProbeTranscript> {
    let sentinel = prefixed_path(&config.prefix, &config.sentinel_key);
    delete_if_exists(store, &sentinel).await?;
    store
        .put_opts(
            &sentinel,
            config.payload_stem.clone().into(),
            PutOptions {
                mode: PutMode::Create,
                ..PutOptions::default()
            },
        )
        .await
        .context("first conditional put probe must create sentinel")?;
    let second = store
        .put_opts(
            &sentinel,
            config.payload_stem.clone().into(),
            PutOptions {
                mode: PutMode::Create,
                ..PutOptions::default()
            },
        )
        .await;
    match second {
        Err(ObjectStoreError::AlreadyExists { .. }) => {
            delete_if_exists(store, &sentinel).await?;
            Ok(ConditionalPutProbeTranscript {
                first_create_result: "Ok".to_string(),
                second_create_result: "AlreadyExists".to_string(),
            })
        }
        Err(ObjectStoreError::NotImplemented { .. }) => {
            bail!("MinIO S3 conditional put is disabled; PutMode::Create returned NotImplemented")
        }
        Err(error) => Err(error).context("second conditional put probe"),
        Ok(_) => {
            bail!("second conditional put probe overwrote the sentinel instead of AlreadyExists")
        }
    }
}

async fn race_create_only_puts(
    store: &AmazonS3,
    config: &ProbeConfig,
) -> anyhow::Result<ConcurrencyProofTranscript> {
    if config.writer_count < 2 {
        bail!("writer_count must be at least 2 for the §4.3.5 concurrency proof");
    }
    let race_path = prefixed_path(&config.prefix, &config.race_key);
    delete_if_exists(store, &race_path).await?;
    let store = Arc::new(store.clone());
    let race_path = Arc::new(race_path);
    let outcomes = join_all((0..config.writer_count).map(|writer_index| {
        let store = Arc::clone(&store);
        let race_path = Arc::clone(&race_path);
        let payload = format!("{}-{writer_index}", config.payload_stem).into_bytes();
        async move {
            let result = store
                .put_opts(
                    race_path.as_ref(),
                    payload.clone().into(),
                    PutOptions {
                        mode: PutMode::Create,
                        ..PutOptions::default()
                    },
                )
                .await
                .map(|_| ());
            WriterOutcome {
                writer_index,
                payload,
                result,
            }
        }
    }))
    .await;

    let mut successes = Vec::new();
    let mut already_exists = 0usize;
    for outcome in outcomes {
        match outcome.result {
            Ok(()) => successes.push(outcome),
            Err(ObjectStoreError::AlreadyExists { .. }) => already_exists += 1,
            Err(ObjectStoreError::NotImplemented { .. }) => {
                bail!("MinIO S3 conditional put is disabled; race returned NotImplemented")
            }
            Err(error) => return Err(error).context("race PutMode::Create writer"),
        }
    }

    assert_eq!(successes.len(), 1, "§4.3.5 requires exactly one PUT winner");
    assert_eq!(
        already_exists,
        config.writer_count - 1,
        "§4.3.5 requires losing writers to observe AlreadyExists"
    );
    let winner = successes.pop().expect("one success");
    let stored = store
        .get(race_path.as_ref())
        .await
        .context("read race object")?
        .bytes()
        .await
        .context("read race object bytes")?;
    assert_eq!(
        stored.as_ref(),
        winner.payload.as_slice(),
        "the winning payload must remain intact; silent overwrite is forbidden"
    );

    Ok(ConcurrencyProofTranscript {
        claim: "exactly one PUT wins and the losers observe AlreadyExists; never two distinct successful PUTs to one key, never a silent overwrite".to_string(),
        writer_count: config.writer_count,
        successful_puts: 1,
        already_exists,
        silent_overwrite_observed: false,
        winning_writer_index: winner.writer_index,
        stored_payload_sha256: sha256_hex(stored.as_ref()),
    })
}

#[tokio::test(flavor = "current_thread")]
async fn minio_put_mode_create_conformance_records_no_overwrite_proof() -> anyhow::Result<()> {
    let Some(config) = enabled_config() else {
        return Ok(());
    };
    let store = prepare_minio(&config).await?;
    delete_prefix(&store, &config.probe.prefix).await?;
    let conditional_put_probe = assert_conditional_put_probe(&store, &config.probe).await?;
    let concurrency_proof = race_create_only_puts(&store, &config.probe).await?;
    let transcript = NoOverwriteProofTranscript {
        proof_name: config.probe.proof_name.clone(),
        requirement_ref: config.probe.requirement_ref.clone(),
        store_uri: format!("s3://{}/{}", config.store.bucket, config.probe.prefix),
        conditional_put_probe,
        concurrency_proof,
    };
    let transcript_path = prefixed_path(&config.probe.prefix, &config.probe.transcript_key);
    store
        .put_opts(
            &transcript_path,
            serde_json::to_vec(&transcript)
                .context("serialize no-overwrite proof transcript")?
                .into(),
            PutOptions {
                mode: PutMode::Create,
                ..PutOptions::default()
            },
        )
        .await
        .context("record no-overwrite proof transcript")?;
    let recorded: NoOverwriteProofTranscript = serde_json::from_slice(
        store
            .get(&transcript_path)
            .await
            .context("read no-overwrite proof transcript")?
            .bytes()
            .await
            .context("read no-overwrite proof transcript bytes")?
            .as_ref(),
    )
    .context("decode no-overwrite proof transcript")?;
    assert_eq!(recorded, transcript);
    Ok(())
}

#[test]
fn nt_catalog_round_trips_trade_ticks_over_minio_s3_uri() -> anyhow::Result<()> {
    let Some(config) = enabled_config() else {
        return Ok(());
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build MinIO setup runtime")?;
    let store = runtime.block_on(prepare_minio(&config))?;
    runtime.block_on(delete_prefix(&store, &config.catalog.prefix))?;

    let instrument = catalog_instrument(&config.catalog)?;
    let instrument_id = instrument.id();
    let trades = catalog_trades(&config.catalog, instrument_id)?;
    let expected_instruments = vec![InstrumentAny::CurrencyPair(instrument)];
    let instrument_filter = vec![instrument_id.to_string()];
    let catalog_uri = format!("s3://{}/{}", config.store.bucket, config.catalog.prefix);
    let mut catalog = ParquetDataCatalog::from_uri(
        &catalog_uri,
        Some(storage_options(&config.store)),
        None,
        None,
        None,
    )
    .context("open MinIO-backed S3 catalog")?;
    catalog
        .write_instruments(expected_instruments.clone())
        .context("write instrument to MinIO-backed S3 catalog")?;
    catalog
        .write_to_parquet(&trades, None, None, None)
        .context("write trade ticks to MinIO-backed S3 catalog")?;

    let loaded_instruments = catalog
        .query_instruments(Some(&instrument_filter))
        .context("query instrument from MinIO-backed S3 catalog")?;
    assert_eq!(loaded_instruments, expected_instruments);

    let loaded: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(Some(instrument_filter), None, None, None, None, true)
        .context("query trade ticks from MinIO-backed S3 catalog")?;
    assert_eq!(loaded.len(), trades.len());
    for (actual, expected) in loaded.iter().zip(trades.iter()) {
        assert_eq!(actual.instrument_id, expected.instrument_id);
        assert_eq!(actual.price, expected.price);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.aggressor_side, expected.aggressor_side);
        assert_eq!(actual.trade_id, expected.trade_id);
        assert_eq!(actual.ts_event, expected.ts_event);
        assert_eq!(actual.ts_init, expected.ts_init);
    }
    Ok(())
}

fn catalog_instrument(config: &CatalogConfig) -> anyhow::Result<CurrencyPair> {
    let instrument_id = InstrumentId::new(
        Symbol::from(config.symbol.as_str()),
        Venue::from(config.venue.as_str()),
    );
    let base_currency = synthetic_currency(&config.base_currency)?;
    let quote_currency = synthetic_currency(&config.quote_currency)?;

    Ok(CurrencyPair::new(
        instrument_id,
        Symbol::from(config.symbol.as_str()),
        base_currency,
        quote_currency,
        config.price_precision,
        config.size_precision,
        Price::new(config.price_increment, config.price_precision),
        Quantity::new(config.size_increment, config.size_precision),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn synthetic_currency(config: &SyntheticCurrencyConfig) -> anyhow::Result<Currency> {
    let currency_type = match config.currency_type.as_str() {
        "Crypto" => CurrencyType::Crypto,
        "Fiat" => CurrencyType::Fiat,
        "CommodityBacked" => CurrencyType::CommodityBacked,
        value => bail!("unsupported synthetic currency type {value}"),
    };
    let currency = Currency::new(
        config.code.as_str(),
        config.precision,
        config.iso4217,
        config.name.as_str(),
        currency_type,
    );
    Currency::register(currency, false)
        .with_context(|| format!("register synthetic currency {}", config.code))?;
    Ok(currency)
}

fn catalog_trades(
    config: &CatalogConfig,
    instrument_id: InstrumentId,
) -> anyhow::Result<Vec<TradeTick>> {
    config
        .trades
        .iter()
        .map(|trade| {
            let ts = UnixNanos::from(trade.timestamp_ns);
            Ok(TradeTick::new(
                instrument_id,
                Price::new(trade.price, config.price_precision),
                Quantity::new(trade.size, config.size_precision),
                aggressor_side(&trade.aggressor_side)?,
                TradeId::from(trade.trade_id.as_str()),
                ts,
                ts,
            ))
        })
        .collect()
}

fn aggressor_side(value: &str) -> anyhow::Result<AggressorSide> {
    match value {
        "buyer" => Ok(AggressorSide::Buyer),
        "seller" => Ok(AggressorSide::Seller),
        other => bail!("unsupported aggressor_side {other:?}"),
    }
}
