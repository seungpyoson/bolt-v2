use nautilus_model::enums::LiquiditySide;
use nautilus_polymarket::execution::parse::{adjust_market_buy_amount, compute_commission};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::{
    bolt_v3_operator_artifacts::{BoltV3OperatorArtifactError, json_artifact_sha256},
    bolt_v3_providers::{
        ClobV2FeeBehaviorSourceMaterialization, ClobV2FeeBehaviorSourceMaterializationRequest,
    },
};

const CLOB_V2_FEE_BEHAVIOR_SOURCE_REQUIREMENTS_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_fee_behavior_source_requirements.v1";
const CLOB_V2_FEE_BEHAVIOR_ASSUMPTIONS_RECORD_KIND: &str =
    "bolt_v3.pre_run_clob_v2_fee_behavior_assumptions.v1";
const CLOB_V2_FEE_BEHAVIOR_SELF_TEST_PRICE: &str = "0.55";
const CLOB_V2_FEE_BEHAVIOR_SELF_TEST_FEE_RATE: &str = "0.04";
const CLOB_V2_FEE_BEHAVIOR_SELF_TEST_SIZE: &str = "10";
const CLOB_V2_FEE_BEHAVIOR_SELF_TEST_BALANCE: &str = "10";
const CLOB_V2_FEE_BEHAVIOR_SELF_TEST_BUILDER_FEE_RATE: &str = "0";
const CLOB_V2_FEE_BEHAVIOR_SELF_TEST_EXPONENT: f64 = 1.0;

pub fn materialize_clob_v2_fee_behavior_source_from_nt_fee_sources(
    request: ClobV2FeeBehaviorSourceMaterializationRequest<'_>,
) -> Result<ClobV2FeeBehaviorSourceMaterialization, BoltV3OperatorArtifactError> {
    let fee_behavior_source_sha256 = clob_v2_fee_behavior_source_requirements_sha256(request)?;
    let price = clob_v2_fee_behavior_decimal("price", CLOB_V2_FEE_BEHAVIOR_SELF_TEST_PRICE)?;
    let fee_rate =
        clob_v2_fee_behavior_decimal("fee_rate", CLOB_V2_FEE_BEHAVIOR_SELF_TEST_FEE_RATE)?;
    let size = clob_v2_fee_behavior_decimal("size", CLOB_V2_FEE_BEHAVIOR_SELF_TEST_SIZE)?;
    let balance = clob_v2_fee_behavior_decimal("balance", CLOB_V2_FEE_BEHAVIOR_SELF_TEST_BALANCE)?;
    let builder_fee_rate = clob_v2_fee_behavior_decimal(
        "builder_fee_rate",
        CLOB_V2_FEE_BEHAVIOR_SELF_TEST_BUILDER_FEE_RATE,
    )?;

    let (maker_commission, taker_commission) =
        clob_v2_fee_behavior_commissions(fee_rate, size, price);
    let adjusted_market_buy_amount = adjust_market_buy_amount(
        size,
        balance,
        price,
        fee_rate,
        CLOB_V2_FEE_BEHAVIOR_SELF_TEST_EXPONENT,
        builder_fee_rate,
    )
    .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "market_buy_fee_adjustment_verified",
    })?;

    let maker_zero_fee_verified = maker_commission.is_zero();
    let taker_fee_schedule_verified = taker_commission > Decimal::ZERO;
    let market_buy_fee_adjustment_verified =
        adjusted_market_buy_amount > Decimal::ZERO && adjusted_market_buy_amount < size;
    if !maker_zero_fee_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "maker_zero_fee_verified",
        });
    }
    if !taker_fee_schedule_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "taker_fee_schedule_verified",
        });
    }
    if !market_buy_fee_adjustment_verified {
        return Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
            field: "market_buy_fee_adjustment_verified",
        });
    }

    let assumptions = ClobV2FeeBehaviorAssumptionsProof {
        schema_version: request.schema_version,
        record_kind: CLOB_V2_FEE_BEHAVIOR_ASSUMPTIONS_RECORD_KIND,
        fee_behavior_source_sha256: &fee_behavior_source_sha256,
        price: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_PRICE,
        fee_rate: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_FEE_RATE,
        size: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_SIZE,
        balance: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_BALANCE,
        builder_fee_rate: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_BUILDER_FEE_RATE,
        fee_exponent: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_EXPONENT,
        maker_commission: maker_commission.to_string(),
        taker_commission: taker_commission.to_string(),
        adjusted_market_buy_amount: adjusted_market_buy_amount.to_string(),
        maker_zero_fee_verified,
        taker_fee_schedule_verified,
        market_buy_fee_adjustment_verified,
    };
    let fee_assumptions_sha256 = json_artifact_sha256(&assumptions)?;

    Ok(ClobV2FeeBehaviorSourceMaterialization {
        maker_zero_fee_verified,
        taker_fee_schedule_verified,
        market_buy_fee_adjustment_verified,
        price: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_PRICE.to_string(),
        fee_rate: CLOB_V2_FEE_BEHAVIOR_SELF_TEST_FEE_RATE.to_string(),
        fee_behavior_source_sha256,
        fee_assumptions_sha256,
    })
}

fn clob_v2_fee_behavior_commissions(
    fee_rate: Decimal,
    size: Decimal,
    price: Decimal,
) -> (Decimal, Decimal) {
    let maker = compute_commission(
        fee_rate,
        CLOB_V2_FEE_BEHAVIOR_SELF_TEST_EXPONENT,
        size,
        price,
        LiquiditySide::Maker,
    );
    let taker = compute_commission(
        fee_rate,
        CLOB_V2_FEE_BEHAVIOR_SELF_TEST_EXPONENT,
        size,
        price,
        LiquiditySide::Taker,
    );
    (maker, taker)
}

fn clob_v2_fee_behavior_source_requirements_sha256(
    request: ClobV2FeeBehaviorSourceMaterializationRequest<'_>,
) -> Result<String, BoltV3OperatorArtifactError> {
    require_fee_source_marker(
        request.nt_http_parse_source,
        "maker_fee",
        "maker_fee_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_http_parse_source,
        "Decimal::ZERO",
        "maker_zero_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_http_parse_source,
        "taker_fee",
        "taker_fee_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_http_parse_source,
        "fee_schedule",
        "fee_schedule_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_execution_parse_source,
        "instrument_taker_fee",
        "instrument_taker_fee_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_execution_parse_source,
        "compute_commission",
        "compute_commission_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_execution_parse_source,
        "LiquiditySide::Taker",
        "liquidity_side_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_execution_parse_source,
        "Decimal::ONE - price",
        "fee_curve_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_execution_parse_source,
        "adjust_market_buy_amount",
        "market_buy_adjustment_source_marker",
    )?;
    require_fee_source_marker(
        request.nt_execution_parse_source,
        "price <= Decimal::ZERO || price >= Decimal::ONE",
        "market_buy_price_bound_source_marker",
    )?;

    let proof = ClobV2FeeBehaviorSourceRequirementsProof {
        schema_version: request.schema_version,
        record_kind: CLOB_V2_FEE_BEHAVIOR_SOURCE_REQUIREMENTS_RECORD_KIND,
        nt_execution_parse_source_sha256: &sha256_text(request.nt_execution_parse_source),
        nt_http_parse_source_sha256: &sha256_text(request.nt_http_parse_source),
        maker_zero_declared: true,
        taker_fee_schedule_declared: true,
        commission_formula_declared: true,
        market_buy_adjustment_declared: true,
    };
    json_artifact_sha256(&proof)
}

fn require_fee_source_marker(
    source: &str,
    marker: &'static str,
    field: &'static str,
) -> Result<(), BoltV3OperatorArtifactError> {
    if source.contains(marker) {
        Ok(())
    } else {
        Err(BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field })
    }
}

fn clob_v2_fee_behavior_decimal(
    field: &'static str,
    value: &str,
) -> Result<Decimal, BoltV3OperatorArtifactError> {
    value
        .parse::<Decimal>()
        .map_err(|_| BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid { field })
}

fn sha256_text(value: &str) -> String {
    crate::bolt_v3_source_integrity::sha256_hex_lower(value.as_bytes())
}

#[derive(Serialize)]
struct ClobV2FeeBehaviorSourceRequirementsProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    nt_execution_parse_source_sha256: &'a str,
    nt_http_parse_source_sha256: &'a str,
    maker_zero_declared: bool,
    taker_fee_schedule_declared: bool,
    commission_formula_declared: bool,
    market_buy_adjustment_declared: bool,
}

#[derive(Serialize)]
struct ClobV2FeeBehaviorAssumptionsProof<'a> {
    schema_version: u32,
    record_kind: &'static str,
    fee_behavior_source_sha256: &'a str,
    price: &'a str,
    fee_rate: &'a str,
    size: &'a str,
    balance: &'a str,
    builder_fee_rate: &'a str,
    fee_exponent: f64,
    maker_commission: String,
    taker_commission: String,
    adjusted_market_buy_amount: String,
    maker_zero_fee_verified: bool,
    taker_fee_schedule_verified: bool,
    market_buy_fee_adjustment_verified: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_behavior_verification_uses_decimal_commission_with_the_pinned_exponent() {
        let fee_rate = "0.04".parse::<Decimal>().expect("literal should parse");
        let size = "10".parse::<Decimal>().expect("literal should parse");
        let price = "0.55".parse::<Decimal>().expect("literal should parse");
        let (maker_commission, taker_commission) =
            clob_v2_fee_behavior_commissions(fee_rate, size, price);

        assert_eq!(maker_commission, Decimal::ZERO);
        assert_eq!(
            taker_commission,
            "0.099".parse::<Decimal>().expect("literal should parse")
        );
    }
}
