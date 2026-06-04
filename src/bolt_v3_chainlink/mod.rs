//! Shared Chainlink Data Streams protocol core for bolt-v3.
//!
//! This module owns the pure, config-free Chainlink protocol logic that was
//! previously inlined in `crate::bolt_v3_operator_artifacts`: REST request
//! authentication (`auth`) and V3 `fullReport` decoding (`report`), plus the
//! report-collection config holder shared between the offline operator
//! collectors and any runtime provider binding.
//!
//! Config resolution (TOML/SSM parameter parsing) and the offline file
//! collectors stay in `crate::bolt_v3_operator_artifacts`; this module is only
//! the reusable protocol core. It is NOT a `src/clients/` legacy default path.

mod auth;
mod report;
mod strike_source;

pub(crate) use auth::{
    chainlink_data_streams_auth_headers, chainlink_data_streams_credentials,
    chainlink_data_streams_report_request_url,
};
pub(crate) use report::{
    CHAINLINK_REPORT_MILLISECONDS_PER_SECOND, CHAINLINK_REPORT_NANOS_PER_MILLISECOND,
    ChainlinkDataStreamsReportApiResponse, DecodedPriceToBeatReport, PriceToBeatReportBinding,
    decode_price_to_beat_report, is_lowercase_chainlink_feed_id,
};
pub(crate) use strike_source::{
    ChainlinkStrikeFeedBinding, ChainlinkStrikeSourceConfig, ChainlinkStrikeSourceFactory,
    STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM, parse_feed_binding,
};

pub(crate) struct ChainlinkDataStreamsReportCollectionConfig {
    pub(crate) rest_base_url: String,
    pub(crate) report_endpoint_path: String,
    pub(crate) api_key_ssm_parameter: String,
    pub(crate) api_secret_ssm_parameter: String,
    pub(crate) http_timeout_secs: u64,
}
