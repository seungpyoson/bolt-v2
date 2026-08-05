use serde::de::DeserializeOwned;

pub fn decode_fixtures<T: DeserializeOwned>() -> T {
    toml::from_str(include_str!("../fixtures/codspeed.toml"))
        .expect("CodSpeed benchmark fixtures must match their typed schema")
}
