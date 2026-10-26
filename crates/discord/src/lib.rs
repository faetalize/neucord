//! Discord protocol layer for Neucord: data model, REST client, gateway
//! connection and keyring-backed session storage.
//!
//! Depends on GLib/GIO/libsoup (it runs on the GLib main loop) but never on
//! GTK, so the UI can be replaced without touching this crate.

pub mod cdn;
pub mod gateway;
mod identity;
pub mod markup;
pub mod model;
pub mod notifications;
pub mod read_state;
pub mod relationships;
pub mod rest;
pub mod session;
pub mod settings;
pub mod shape;

pub use gateway::{Event, Gateway, Ready, Resume};
pub use model::{Attachment, Channel, ChannelKind, Guild, Message, Snowflake, User};
pub use rest::Rest;

/// Errors surfaced to the UI. Never contains the token.
#[derive(Debug)]
pub enum Error {
    /// Network or TLS failure.
    Transport(glib::Error),
    /// Discord answered with a non-success status.
    Http { status: u32, body: String },
    /// The token was rejected (HTTP 401 or gateway close code 4004).
    Unauthorized,
    /// Discord sent something we could not parse.
    Decode(serde_json::Error),
    /// The secret service (keyring) failed.
    Keyring(glib::Error),
    /// Discord wants a captcha solved first, which Neucord cannot do yet.
    Captcha,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "network error: {e}"),
            Error::Http { status, body } => {
                // Error bodies are JSON with a readable `message`; show that if there is one.
                let message = serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| v["message"].as_str().map(String::from));
                write!(f, "Discord returned HTTP {status}: {}", message.as_deref().unwrap_or(body))
            }
            Error::Unauthorized => write!(f, "Discord rejected the token"),
            Error::Decode(e) => write!(f, "unexpected data from Discord: {e}"),
            Error::Keyring(e) => write!(f, "keyring error: {e}"),
            Error::Captcha => write!(f, "Discord asked for a captcha, which Neucord can't show yet"),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e)
    }
}
