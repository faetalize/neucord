//! How Neucord identifies itself to Discord: honestly, as Neucord. REST and
//! the gateway both read from here.

use serde_json::{Value, json};

/// The documented API version.
pub const API_BASE: &str = "https://discord.com/api/v10";
pub const GATEWAY_QUERY: &str = "v=10&encoding=json";
pub const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

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
