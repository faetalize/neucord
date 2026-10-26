//! Discord protocol layer for Neucord: data model, REST client, gateway
//! connection and keyring-backed session storage.
//!
//! Depends on GLib/GIO/libsoup (it runs on the GLib main loop) but never on
//! GTK, so the UI can be replaced without touching this crate.

mod identity;
pub mod gateway;
pub mod model;
pub mod rest;
pub mod session;
pub mod cdn;
pub mod markup;
pub mod shape;

pub use gateway::{Event, Gateway};
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "network error: {e}"),
            Error::Http { status, body } => write!(f, "Discord returned HTTP {status}: {body}"),
            Error::Unauthorized => write!(f, "Discord rejected the token"),
            Error::Decode(e) => write!(f, "unexpected data from Discord: {e}"),
            Error::Keyring(e) => write!(f, "keyring error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e)
    }
}
