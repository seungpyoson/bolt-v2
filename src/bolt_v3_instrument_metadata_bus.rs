use nautilus_common::msgbus::{self, MStr, Pattern, TypedHandler, switchboard};
use nautilus_model::{identifiers::Venue, instruments::InstrumentAny};

pub(crate) fn metadata_instrument_pattern(venue: Venue) -> MStr<Pattern> {
    switchboard::get_instruments_pattern(venue)
}

pub(crate) fn attach_metadata_instrument_handler(
    pattern: MStr<Pattern>,
    handler: TypedHandler<InstrumentAny>,
) {
    msgbus::subscribe_instruments(pattern, handler, None);
}

pub(crate) fn detach_metadata_instrument_handler(
    pattern: MStr<Pattern>,
    handler: &TypedHandler<InstrumentAny>,
) {
    msgbus::unsubscribe_instruments(pattern, handler);
}
