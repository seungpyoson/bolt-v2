use bolt_v2::bolt_v3_maker_event_fence::{
    ClientOrderId, ExpectedIdentity, FenceReject, OrderIdentity, VenueReport, VenueReportKind,
};
use bolt_v2::bolt_v3_quote_lifecycle::LegEvent;

fn client_order_id(value: &str) -> ClientOrderId {
    ClientOrderId::new(value.to_string())
}

fn identity(value: &str, generation: u64) -> OrderIdentity {
    OrderIdentity::new(client_order_id(value), generation)
}

fn report(value: &str, generation: u64, kind: VenueReportKind) -> VenueReport {
    VenueReport {
        client_order_id: client_order_id(value),
        generation,
        kind,
    }
}

#[test]
fn stale_accepted_venue_report_is_rejected() {
    let fence = ExpectedIdentity::submitting(identity("order-a", 7));

    assert_eq!(
        fence.admit(&report("order-a", 6, VenueReportKind::Accepted)),
        Err(FenceReject::StaleGeneration)
    );
}

#[test]
fn wrong_client_order_id_is_rejected() {
    let fence = ExpectedIdentity::submitting(identity("order-a", 7));

    assert_eq!(
        fence.admit(&report("order-b", 7, VenueReportKind::Accepted)),
        Err(FenceReject::ForeignClientId)
    );
}

#[test]
fn wrong_future_generation_is_rejected() {
    let fence = ExpectedIdentity::submitting(identity("order-a", 7));

    assert_eq!(
        fence.admit(&report("order-a", 8, VenueReportKind::Accepted)),
        Err(FenceReject::UnknownOrder)
    );
}

#[test]
fn correct_report_is_admitted_as_lifecycle_event() {
    let fence = ExpectedIdentity::submitting(identity("order-a", 7));

    assert_eq!(
        fence.admit(&report("order-a", 7, VenueReportKind::Filled)),
        Ok(LegEvent::Filled)
    );
}

#[test]
fn clear_and_idle_reject_reports_as_unknown() {
    let mut fence = ExpectedIdentity::submitting(identity("order-a", 7));
    fence.clear();

    assert_eq!(fence.expected(), None);
    assert_eq!(
        fence.admit(&report("order-a", 7, VenueReportKind::Filled)),
        Err(FenceReject::UnknownOrder)
    );

    let idle = ExpectedIdentity::idle();
    assert_eq!(
        idle.admit(&report("order-a", 7, VenueReportKind::Accepted)),
        Err(FenceReject::UnknownOrder)
    );
}

#[test]
fn requote_generation_transition_rehomes_the_expected_order() {
    let mut fence = ExpectedIdentity::submitting(identity("order-a", 7));

    assert!(!fence.requote_to(identity("order-b", 7)));
    assert_eq!(fence.expected(), Some(&identity("order-a", 7)));

    assert!(fence.requote_to(identity("order-b", 8)));
    assert_eq!(fence.expected(), Some(&identity("order-b", 8)));
    assert_eq!(
        fence.admit(&report("order-b", 8, VenueReportKind::Accepted)),
        Ok(LegEvent::Accepted)
    );
    assert_eq!(
        fence.admit(&report("order-a", 7, VenueReportKind::Canceled)),
        Err(FenceReject::ForeignClientId)
    );
}
