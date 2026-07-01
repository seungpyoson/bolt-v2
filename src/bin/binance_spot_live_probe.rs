use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{
        BoltV3LiveNodeRuntime, build_bolt_v3_strategy_free_data_client_probe_live_node,
    },
    bolt_v3_providers::{
        SpotLiveProbeClientDescription, classify_spot_live_probe_failure,
        configured_spot_live_probe_client, live_node_exited_before_price_reason,
        no_live_price_reason, sanitize_spot_live_probe_error,
    },
};
use clap::Parser;
use nautilus_common::msgbus::{self, MStr, Pattern, TypedHandler, switchboard};
use nautilus_live::node::LiveNodeHandle;
use nautilus_model::{
    data::QuoteTick,
    identifiers::{ClientId, InstrumentId},
};
use nautilus_network::http::HttpClient;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    client_key: String,
    #[arg(long)]
    instrument: String,
    #[arg(long)]
    timeout_secs: u64,
    #[arg(long)]
    source_ip_url: String,
    #[arg(long)]
    source_ip_timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeVerdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone)]
struct QuoteObservation {
    price: String,
    bid_price: String,
    ask_price: String,
    ts_event_ns: u64,
    ts_init_ns: u64,
}

struct QuoteObserver {
    pattern: MStr<Pattern>,
    handler: TypedHandler<QuoteTick>,
}

impl QuoteObserver {
    fn register(
        instrument_id: InstrumentId,
        slot: Arc<Mutex<Option<QuoteObservation>>>,
        stop_handle: LiveNodeHandle,
    ) -> Self {
        let pattern: MStr<Pattern> = switchboard::get_quotes_topic(instrument_id).into();
        let handler = TypedHandler::from(move |quote: &QuoteTick| {
            if quote.instrument_id != instrument_id {
                return;
            }
            let Ok(mut guard) = slot.lock() else {
                stop_handle.stop();
                return;
            };
            if guard.is_none() {
                *guard = Some(QuoteObservation::from(quote));
                stop_handle.stop();
            }
        });
        msgbus::subscribe_quotes(pattern, handler.clone(), None);
        Self { pattern, handler }
    }

    fn unregister(self) {
        msgbus::unsubscribe_quotes(self.pattern, &self.handler);
    }
}

impl QuoteObservation {
    fn from(quote: &QuoteTick) -> Self {
        Self {
            price: quote.bid_price.to_string(),
            bid_price: quote.bid_price.to_string(),
            ask_price: quote.ask_price.to_string(),
            ts_event_ns: quote.ts_event.as_u64(),
            ts_init_ns: quote.ts_init.as_u64(),
        }
    }
}

fn main() -> ExitCode {
    let result = run_cli(Cli::parse());
    match result {
        Ok(ProbeVerdict::Pass) => ExitCode::SUCCESS,
        Ok(ProbeVerdict::Fail) => ExitCode::FAILURE,
        Err(error) => {
            let reason = sanitize_spot_live_probe_error(&error.to_string(), []);
            let mut stdout = std::io::stdout().lock();
            writeln!(&mut stdout, "VERDICT: FAIL reason={reason}")
                .expect("failed to write spot live probe verdict");
            ExitCode::FAILURE
        }
    }
}

fn run_cli(cli: Cli) -> Result<ProbeVerdict> {
    let loaded = load_bolt_v3_config(&cli.config)
        .with_context(|| format!("failed to load config {}", cli.config.display()))?;
    let client =
        configured_spot_live_probe_client(&loaded, &cli.client_key).map_err(anyhow::Error::msg)?;
    let instrument_id = InstrumentId::from(cli.instrument.as_str());
    let client_id = ClientId::from(cli.client_key.as_str());
    let source_ip = fetch_source_ip_blocking(&cli.source_ip_url, cli.source_ip_timeout_secs)?;

    print_probe_header(&loaded, &client, &cli, &source_ip);

    let (mut runtime, probe_loaded) =
        build_bolt_v3_strategy_free_data_client_probe_live_node(&loaded, &cli.client_key)
            .context("failed to build strategy-free spot data LiveNode")?;
    {
        let mut stdout = std::io::stdout().lock();
        writeln!(
            &mut stdout,
            "connect_status=started strategy_free_live_node"
        )?;
    }

    let quote_observation = Arc::new(Mutex::new(None));
    let observer = QuoteObserver::register(
        instrument_id,
        Arc::clone(&quote_observation),
        runtime.handle(),
    );
    if let Err(error) = runtime.subscribe_strategy_free_quotes(client_id, instrument_id) {
        observer.unregister();
        let reason = sanitize_spot_live_probe_error(
            &error.to_string(),
            runtime
                .redaction_values()
                .iter()
                .map(|value| value.as_str()),
        );
        print_failure(&cli.instrument, cli.timeout_secs, &source_ip, &reason)?;
        return Ok(ProbeVerdict::Fail);
    }
    {
        let mut stdout = std::io::stdout().lock();
        writeln!(
            &mut stdout,
            "subscribe_status=submitted client={} instrument={} kind=quote",
            cli.client_key, cli.instrument
        )?;
    }

    let run_result =
        run_strategy_free_probe_blocking(&mut runtime, &probe_loaded, cli.timeout_secs);
    runtime.unsubscribe_strategy_free_quotes(client_id, instrument_id);
    observer.unregister();

    match run_result {
        Ok(timed_out) => {
            let observed = quote_observation
                .lock()
                .ok()
                .and_then(|guard| guard.clone());
            if let Some(observed) = observed {
                let mut stdout = std::io::stdout().lock();
                writeln!(
                    &mut stdout,
                    "live_update=PASS observed=true within_secs={} price={} bid={} ask={} ts_event_ns={} ts_init_ns={}",
                    cli.timeout_secs,
                    observed.price,
                    observed.bid_price,
                    observed.ask_price,
                    observed.ts_event_ns,
                    observed.ts_init_ns
                )?;
                writeln!(&mut stdout, "VERDICT: PASS")?;
                Ok(ProbeVerdict::Pass)
            } else {
                let reason = if timed_out {
                    no_live_price_reason(&cli.instrument, cli.timeout_secs)
                } else {
                    live_node_exited_before_price_reason(&cli.instrument)
                };
                print_failure(&cli.instrument, cli.timeout_secs, &source_ip, &reason)?;
                Ok(ProbeVerdict::Fail)
            }
        }
        Err(error) => {
            let reason = sanitize_spot_live_probe_error(
                &error.to_string(),
                runtime
                    .redaction_values()
                    .iter()
                    .map(|value| value.as_str()),
            );
            print_failure(&cli.instrument, cli.timeout_secs, &source_ip, &reason)?;
            Ok(ProbeVerdict::Fail)
        }
    }
}

fn print_probe_header(
    loaded: &LoadedBoltV3Config,
    client: &SpotLiveProbeClientDescription,
    cli: &Cli,
    source_ip: &str,
) {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        &mut stdout,
        "venue={} client={} instrument={} product_type={} environment={} market_data_mode={}",
        client.venue,
        cli.client_key,
        cli.instrument,
        client.product_type,
        client.environment,
        client.spot_market_data_mode
    )
    .expect("failed to write spot live probe header");
    writeln!(
        &mut stdout,
        "endpoints=http:{} ws:{} source_ip={} aws_region={}",
        client.base_url_http, client.base_url_ws, source_ip, loaded.root.aws.region
    )
    .expect("failed to write spot live probe endpoints");
}

fn print_failure(
    instrument: &str,
    timeout_secs: u64,
    source_ip: &str,
    raw_reason: &str,
) -> Result<()> {
    let failure = classify_spot_live_probe_failure(raw_reason, source_ip);
    let mut stdout = std::io::stdout().lock();
    writeln!(
        &mut stdout,
        "live_update=FAIL observed=false within_secs={timeout_secs} instrument={instrument}"
    )?;
    writeln!(&mut stdout, "VERDICT: FAIL reason={}", failure.reason)?;
    Ok(())
}

fn fetch_source_ip_blocking(source_ip_url: &str, timeout_secs: u64) -> Result<String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build source-IP lookup runtime")?;
    runtime.block_on(fetch_source_ip(source_ip_url, timeout_secs))
}

async fn fetch_source_ip(source_ip_url: &str, timeout_secs: u64) -> Result<String> {
    let client = HttpClient::new(
        HashMap::new(),
        Vec::new(),
        Vec::new(),
        None,
        Some(timeout_secs),
        None,
    )
    .context("failed to construct source-IP HTTP client")?;
    let response = client
        .get(
            source_ip_url.to_owned(),
            None,
            None,
            Some(timeout_secs),
            None,
        )
        .await
        .with_context(|| format!("source-IP lookup failed via {source_ip_url}"))?;
    if !response.status.is_success() {
        anyhow::bail!(
            "source-IP lookup via {} returned HTTP {}",
            source_ip_url,
            response.status.as_u16()
        );
    }
    let source_ip = String::from_utf8_lossy(&response.body).trim().to_owned();
    if source_ip.is_empty() {
        anyhow::bail!("source-IP lookup via {source_ip_url} returned an empty response");
    }
    Ok(source_ip)
}

fn run_strategy_free_probe_blocking(
    runtime: &mut BoltV3LiveNodeRuntime,
    probe_loaded: &LoadedBoltV3Config,
    timeout_secs: u64,
) -> Result<bool> {
    let stop_timeout_secs = strategy_free_stop_timeout_secs(probe_loaded)?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build strategy-free probe runtime")?;
    let local = tokio::task::LocalSet::new();
    tokio_runtime.block_on(local.run_until(async {
        runtime
            .run_strategy_free_until_stop_or_timeout(
                Duration::from_secs(timeout_secs),
                Duration::from_secs(stop_timeout_secs),
            )
            .await
            .map_err(anyhow::Error::from)
    }))
}

fn strategy_free_stop_timeout_secs(loaded: &LoadedBoltV3Config) -> Result<u64> {
    loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .context("strategy-free stop timeout sum overflowed config-owned Nautilus timeout fields")
}
