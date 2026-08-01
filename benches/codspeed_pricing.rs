mod support;

use std::sync::OnceLock;

use bolt_v2::{
    bolt_v3_maker_microprice::{
        book_imbalance as calculate_book_imbalance, micro_price as calculate_micro_price,
        micro_price_anchor,
    },
    bolt_v3_maker_model::{GmReservationBand, gm_binary_quote},
    bolt_v3_market_families::maker_quote_targets_for_family,
    bolt_v3_quoting::FamilyQuoteInputs,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct FixtureFile {
    pricing: PricingFixture,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PricingFixture {
    best_bid: f64,
    best_ask: f64,
    bid_size: f64,
    ask_size: f64,
    oracle_fair: f64,
    micro_weight: f64,
    fair_probability: f64,
    informed_fraction: f64,
    family_key: String,
    inventory_skew: f64,
    half_spread_floor: f64,
    max_half_spread: f64,
    epsilon: f64,
    time_to_expiry: f64,
    reference_time_to_expiry: f64,
    time_widen_cap: f64,
    order_notional_target: f64,
    maximum_position_notional: f64,
}

fn main() {
    divan::main();
}

fn fixture() -> &'static PricingFixture {
    static FIXTURE: OnceLock<PricingFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let fixture = support::decode_fixtures::<FixtureFile>().pricing;
        validate_fixture(&fixture);
        fixture
    })
}

#[divan::bench]
fn book_imbalance(bencher: divan::Bencher<'_, '_>) {
    let fixture = fixture();
    bencher.bench(|| {
        divan::black_box(calculate_book_imbalance(
            divan::black_box(fixture.bid_size),
            divan::black_box(fixture.ask_size),
        ))
    });
}

#[divan::bench]
fn micro_price(bencher: divan::Bencher<'_, '_>) {
    let fixture = fixture();
    bencher.bench(|| {
        divan::black_box(calculate_micro_price(
            divan::black_box(fixture.best_bid),
            divan::black_box(fixture.best_ask),
            divan::black_box(fixture.bid_size),
            divan::black_box(fixture.ask_size),
        ))
    });
}

#[divan::bench]
fn anchored_micro_price(bencher: divan::Bencher<'_, '_>) {
    let fixture = fixture();
    bencher.bench(|| {
        let micro = calculate_micro_price(
            divan::black_box(fixture.best_bid),
            divan::black_box(fixture.best_ask),
            divan::black_box(fixture.bid_size),
            divan::black_box(fixture.ask_size),
        );
        divan::black_box(micro_price_anchor(
            divan::black_box(fixture.oracle_fair),
            divan::black_box(micro),
            divan::black_box(fixture.micro_weight),
        ))
    });
}

#[divan::bench]
fn quote_layout(bencher: divan::Bencher<'_, '_>) {
    let fixture = fixture();
    bencher.bench(|| {
        let targets = gm_binary_quote(
            divan::black_box(fixture.fair_probability),
            divan::black_box(fixture.informed_fraction),
        )
        .and_then(|band| {
            maker_quote_targets_for_family(
                divan::black_box(fixture.family_key.as_str()),
                quote_inputs(fixture, band),
            )
        });
        divan::black_box(targets)
    });
}

fn quote_inputs(fixture: &PricingFixture, band: GmReservationBand) -> FamilyQuoteInputs {
    FamilyQuoteInputs {
        band,
        inventory_skew: fixture.inventory_skew,
        half_spread_floor: fixture.half_spread_floor,
        max_half_spread: fixture.max_half_spread,
        eps: fixture.epsilon,
        tau: fixture.time_to_expiry,
        reference_tau: fixture.reference_time_to_expiry,
        time_widen_cap: fixture.time_widen_cap,
        order_notional_target: fixture.order_notional_target,
        maximum_position_notional: fixture.maximum_position_notional,
    }
}

fn validate_fixture(fixture: &PricingFixture) {
    let imbalance = calculate_book_imbalance(fixture.bid_size, fixture.ask_size)
        .expect("pricing fixture must produce a book imbalance");
    assert!(imbalance.is_finite());

    let micro = calculate_micro_price(
        fixture.best_bid,
        fixture.best_ask,
        fixture.bid_size,
        fixture.ask_size,
    )
    .expect("pricing fixture must produce a micro price");
    assert!(micro.is_finite());

    let anchor = micro_price_anchor(fixture.oracle_fair, Some(micro), fixture.micro_weight)
        .expect("pricing fixture must produce an anchored micro price");
    assert!(anchor.is_finite());

    let band = gm_binary_quote(fixture.fair_probability, fixture.informed_fraction)
        .expect("pricing fixture must produce a reservation band");
    let targets =
        maker_quote_targets_for_family(fixture.family_key.as_str(), quote_inputs(fixture, band))
            .expect("pricing fixture must produce quote targets");
    for leg in [targets.leg_a, targets.leg_b] {
        assert!(leg.price.is_finite() && leg.price > 0.0 && leg.price < 1.0);
        assert!(leg.size_notional.is_finite() && leg.size_notional > 0.0);
    }
}
