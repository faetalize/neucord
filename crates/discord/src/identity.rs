//! How Neucord identifies itself to Discord: honestly, as Neucord. REST and
//! the gateway both read from here.

use serde_json::{Value, json};

/// The API version is the one the official client uses (its `API_VERSION`):
/// a version no official client speaks would set every request apart (#73).
/// `tools/discord-watch` reads it from `API_BASE` and reports when the
/// client moves on.
pub const API_BASE: &str = "https://discord.com/api/v9";
pub const GATEWAY_QUERY: &str = "v=9&encoding=json";
pub const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=9&encoding=json";

const NAME: &str = "Neucord";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn user_agent() -> String {
    format!("{NAME}/{VERSION} (Linux; libsoup)")
}

/// The `properties` object sent in IDENTIFY.
pub fn client_properties() -> Value {
    json!({
        "os": std::env::consts::OS,
        "browser": NAME,
        "device": NAME,
    })
}

/// [`client_properties`] in base64, for the `X-Super-Properties` header.
pub(crate) fn super_properties() -> String {
    openssl::base64::encode_block(client_properties().to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_api_version_everywhere() {
        let version = API_BASE.rsplit_once("/v").map(|(_, v)| v).unwrap();
        assert!(GATEWAY_QUERY.starts_with(&format!("v={version}&")));
        assert!(GATEWAY_URL.ends_with(&format!("?{GATEWAY_QUERY}")));
    }
}
