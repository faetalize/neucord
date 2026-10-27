//! Discord protocol layer for Neucord: data model, REST client, gateway
//! connection, logging in and keyring-backed session storage.
//!
//! Depends on GLib/GIO/libsoup (it runs on the GLib main loop) but never on
//! GTK, so the UI can be replaced without touching this crate.

pub mod account;
pub mod auth;
pub mod cdn;
pub mod embed;
mod endpoints;
pub mod gateway;
mod identity;
pub mod markup;
pub mod model;
pub mod notifications;
pub mod read_state;
pub mod relationships;
pub mod remote_auth;
pub mod rest;
pub mod session;
pub mod settings;
pub mod shape;
pub mod system;

pub use gateway::{Event, Gateway, Ready, Resume};
pub use model::{
    Attachment, Channel, ChannelKind, Guild, Message, MessageEdit, MessageReference, Referenced, Snowflake, User,
};
pub use rest::{Captcha, CaptchaSolver, Rest};

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
    /// Discord sent something that parsed but makes no sense.
    Protocol(String),
    /// Discord turned a login down, and said why.
    Rejected(String),
    /// Key generation or decryption failed, for QR login.
    Crypto(openssl::error::ErrorStack),
    /// The secret service (keyring) failed.
    Keyring(glib::Error),
    /// Discord wanted a captcha solved first, and it wasn't: there was no
    /// [`rest::CaptchaSolver`], or the user closed it.
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
            Error::Protocol(e) => write!(f, "unexpected data from Discord: {e}"),
            Error::Rejected(why) => f.write_str(why),
            Error::Crypto(e) => write!(f, "encryption error: {e}"),
            Error::Keyring(e) => write!(f, "keyring error: {e}"),
            Error::Captcha => write!(f, "Discord asked for a captcha, and it wasn't solved"),
        }
    }
}

impl Error {
    /// Discord's JSON error code (`{"code": 80004, ...}`), if it sent one.
    pub fn code(&self) -> Option<u64> {
        match self {
            Error::Http { body, .. } => serde_json::from_str::<serde_json::Value>(body).ok()?["code"].as_u64(),
            _ => None,
        }
    }
}

impl std::error::Error for Error {}

impl From<openssl::error::ErrorStack> for Error {
    fn from(e: openssl::error::ErrorStack) -> Self {
        Error::Crypto(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e)
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn error_codes_and_messages() {
        let e = Error::Http { status: 400, body: r#"{"message": "Unknown User", "code": 80004}"#.into() };
        assert_eq!(e.code(), Some(80004));
        assert_eq!(e.to_string(), "Discord returned HTTP 400: Unknown User");
        let cut = Error::Http { status: 400, body: r#"{"message": "trunc"#.into() };
        assert_eq!(cut.code(), None, "a truncated body has no code");
        assert!(cut.to_string().ends_with(r#"{"message": "trunc"#));
    }
}
