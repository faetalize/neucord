//! Logging in by QR code, through Discord's remote auth gateway.
//!
//! Each session has its own RSA key, made on the spot and kept in memory.
//! The gateway checks we hold it by having us decrypt a nonce, then names
//! the session by the key's fingerprint, which the QR code carries. The
//! phone app that scans it encrypts the user to our key, and once they
//! approve, sends a ticket that we trade for the token, encrypted to our
//! key again. No password passes through Neucord, and a stolen ticket is
//! useless without the private key.
//!
//! A session lasts a few minutes. Its end is reported as an [`Event`]; the
//! caller starts a new one to show a fresh code.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use openssl::base64;
use openssl::encrypt::Decrypter;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::{Padding, Rsa};
use serde_json::{Value, json};
use soup::prelude::*;

use crate::{Error, Rest, User, identity};

const URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
/// The gateway only serves Discord's own web origins.
const ORIGIN: &str = "https://discord.com";
const CLOSE_TIMEOUT: u16 = 4003;

#[derive(Debug)]
pub enum Event {
    /// Show this URL as a QR code.
    Code(String),
    /// Someone scanned the code; they confirm on their phone next. Only the
    /// id, username and avatar are known.
    Scanned(User),
    /// They confirmed. The session is over.
    Token(String),
    /// They declined on the phone. The session is over.
    Cancelled,
    /// Nobody finished in time. The session is over.
    Expired,
    Failed(Error),
}

/// A live session. Dropping it ends the session.
pub struct RemoteAuth {
    conn: soup::WebsocketConnection,
}

impl Drop for RemoteAuth {
    fn drop(&mut self) {
        if self.conn.state() == soup::WebsocketState::Open {
            self.conn.close(1000, None);
        }
    }
}

struct State {
    keys: Keys,
    /// A ticket arrived or the phone declined; how the connection then
    /// closes no longer matters.
    settled: Cell<bool>,
    acked: Cell<bool>,
    events: async_channel::Sender<Event>,
}

impl State {
    fn finish(&self, event: Event) {
        self.settled.set(true);
        let _ = self.events.try_send(event);
        self.events.close();
    }
}

impl RemoteAuth {
    pub async fn connect() -> Result<(RemoteAuth, async_channel::Receiver<Event>), Error> {
        let keys = gio::spawn_blocking(Keys::generate).await.expect("key generation does not panic")?;
        let session = soup::Session::new();
        session.set_user_agent(&identity::user_agent());
        let msg = soup::Message::new("GET", URL).expect("remote auth URL is valid");
        let conn = session
            .websocket_connect_async_future(&msg, Some(ORIGIN), &[], glib::Priority::DEFAULT)
            .await
            .map_err(Error::Transport)?;

        let (tx, rx) = async_channel::unbounded();
        let state = Rc::new(State { keys, settled: Cell::new(false), acked: Cell::new(true), events: tx });
        conn.connect_message({
            let state = state.clone();
            move |conn, kind, bytes| {
                if kind != soup::ffi::SOUP_WEBSOCKET_DATA_TEXT || state.settled.get() {
                    return;
                }
                match state.keys.handle(bytes) {
                    Ok(step) => act(conn, &state, step),
                    Err(e) => {
                        state.finish(Event::Failed(e));
                        conn.close(1000, None);
                    }
                }
            }
        });
        conn.connect_closed({
            let state = state.clone();
            move |conn| {
                if state.settled.get() {
                    return;
                }
                state.finish(match conn.close_code() {
                    CLOSE_TIMEOUT => Event::Expired,
                    code => Event::Failed(Error::Protocol(format!("QR login closed with code {code}"))),
                });
            }
        });
        Ok((RemoteAuth { conn }, rx))
    }
}

fn act(conn: &soup::WebsocketConnection, state: &Rc<State>, step: Step) {
    match step {
        Step::Hello { heartbeat, init } => {
            send(conn, &init);
            spawn_heartbeat(conn, state, heartbeat);
        }
        Step::Send(value) => send(conn, &value),
        Step::Code(url) => {
            let _ = state.events.try_send(Event::Code(url));
        }
        Step::Scanned(user) => {
            let _ = state.events.try_send(Event::Scanned(user));
        }
        Step::Ticket(ticket) => {
            state.settled.set(true);
            let state = state.clone();
            glib::spawn_future_local(async move {
                let token = Rest::anonymous()
                    .remote_auth_login(&ticket)
                    .await
                    .and_then(|encrypted| state.keys.decrypt(&encrypted))
                    .and_then(|token| {
                        String::from_utf8(token).map_err(|_| Error::Protocol("token is not text".into()))
                    });
                state.finish(match token {
                    Ok(token) => Event::Token(token),
                    Err(e) => Event::Failed(e),
                });
            });
        }
        Step::Cancelled => state.finish(Event::Cancelled),
        Step::Ack => state.acked.set(true),
        Step::Ignore => {}
    }
}

fn send(conn: &soup::WebsocketConnection, value: &Value) {
    if conn.state() == soup::WebsocketState::Open {
        conn.send_text(&value.to_string());
    }
}

fn spawn_heartbeat(conn: &soup::WebsocketConnection, state: &Rc<State>, interval: Duration) {
    let conn = conn.downgrade();
    let state = state.clone();
    glib::spawn_future_local(async move {
        glib::timeout_future(interval.mul_f64(glib::random_double_range(0.0, 1.0))).await;
        loop {
            let Some(conn) = conn.upgrade() else { break };
            if conn.state() != soup::WebsocketState::Open {
                break;
            }
            if !state.acked.get() {
                conn.close(1000, Some("heartbeat not acknowledged"));
                break;
            }
            state.acked.set(false);
            send(&conn, &json!({ "op": "heartbeat" }));
            drop(conn);
            glib::timeout_future(interval).await;
        }
    });
}

/// What a gateway message asks of us.
#[derive(Debug)]
enum Step {
    /// Send our key, and heartbeat at this interval.
    Hello {
        heartbeat: Duration,
        init: Value,
    },
    Send(Value),
    Code(String),
    Scanned(User),
    Ticket(String),
    Cancelled,
    Ack,
    Ignore,
}

/// The session's key pair, and the protocol steps that need it.
struct Keys {
    key: PKey<Private>,
    /// The public key as DER SubjectPublicKeyInfo.
    public: Vec<u8>,
}

impl Keys {
    fn generate() -> Result<Keys, Error> {
        let key = PKey::from_rsa(Rsa::generate(2048)?)?;
        let public = key.public_key_to_der()?;
        Ok(Keys { key, public })
    }

    /// What the QR code identifies us by.
    fn fingerprint(&self) -> String {
        base64url(&openssl::sha::sha256(&self.public))
    }

    fn handle(&self, text: &[u8]) -> Result<Step, Error> {
        let message: Value = serde_json::from_slice(text)?;
        let field = |name: &str| {
            message[name].as_str().ok_or_else(|| Error::Protocol(format!("QR login message without {name}")))
        };
        Ok(match message["op"].as_str().unwrap_or_default() {
            "hello" => Step::Hello {
                heartbeat: Duration::from_millis(message["heartbeat_interval"].as_u64().unwrap_or(41_250)),
                init: json!({ "op": "init", "encoded_public_key": base64::encode_block(&self.public) }),
            },
            "nonce_proof" => {
                let nonce = self.decrypt(field("encrypted_nonce")?)?;
                Step::Send(json!({ "op": "nonce_proof", "nonce": base64url(&nonce) }))
            }
            // Check the gateway named our key, not someone else's.
            "pending_remote_init" if field("fingerprint")? == self.fingerprint() => {
                Step::Code(format!("https://discord.com/ra/{}", self.fingerprint()))
            }
            "pending_remote_init" => return Err(Error::Protocol("QR login fingerprint does not match our key".into())),
            "pending_ticket" => {
                let payload = self.decrypt(field("encrypted_user_payload")?)?;
                Step::Scanned(
                    parse_user(&String::from_utf8_lossy(&payload))
                        .ok_or_else(|| Error::Protocol("unreadable user in QR login".into()))?,
                )
            }
            "pending_login" => Step::Ticket(field("ticket")?.to_owned()),
            "cancel" => Step::Cancelled,
            "heartbeat_ack" => Step::Ack,
            _ => Step::Ignore,
        })
    }

    /// RSA-OAEP with SHA-256, of base64 `data`.
    fn decrypt(&self, data: &str) -> Result<Vec<u8>, Error> {
        let data = base64::decode_block(data)?;
        let mut decrypter = Decrypter::new(&self.key)?;
        decrypter.set_rsa_padding(Padding::PKCS1_OAEP)?;
        decrypter.set_rsa_oaep_md(MessageDigest::sha256())?;
        decrypter.set_rsa_mgf1_md(MessageDigest::sha256())?;
        let mut out = vec![0; decrypter.decrypt_len(&data)?];
        let len = decrypter.decrypt(&data, &mut out)?;
        out.truncate(len);
        Ok(out)
    }
}

/// URL-safe base64 without padding.
fn base64url(data: &[u8]) -> String {
    base64::encode_block(data).trim_end_matches('=').replace('+', "-").replace('/', "_")
}

/// `id:discriminator:avatar:username`, with avatar `0` for none.
fn parse_user(payload: &str) -> Option<User> {
    let mut parts = payload.splitn(4, ':');
    let id = parts.next()?.parse().ok()?;
    let avatar = parts.nth(1)?;
    let username = parts.next()?;
    Some(User {
        id: crate::Snowflake(id),
        username: username.to_owned(),
        global_name: None,
        avatar: (avatar != "0").then(|| avatar.to_owned()),
        bot: false,
        system: false,
    })
}

#[cfg(test)]
mod tests {
    use openssl::encrypt::Encrypter;

    use super::*;

    /// What the gateway does: encrypt to the key it was sent.
    fn encrypt_to(keys: &Keys, data: &[u8]) -> String {
        let public = PKey::public_key_from_der(&keys.public).unwrap();
        let mut encrypter = Encrypter::new(&public).unwrap();
        encrypter.set_rsa_padding(Padding::PKCS1_OAEP).unwrap();
        encrypter.set_rsa_oaep_md(MessageDigest::sha256()).unwrap();
        encrypter.set_rsa_mgf1_md(MessageDigest::sha256()).unwrap();
        let mut out = vec![0; encrypter.encrypt_len(data).unwrap()];
        let len = encrypter.encrypt(data, &mut out).unwrap();
        base64::encode_block(&out[..len])
    }

    fn handle(keys: &Keys, message: Value) -> Result<Step, Error> {
        keys.handle(message.to_string().as_bytes())
    }

    #[test]
    fn handshake_to_ticket() {
        let keys = Keys::generate().unwrap();

        let Ok(Step::Hello { heartbeat, init }) =
            handle(&keys, json!({"op": "hello", "heartbeat_interval": 41250, "timeout_ms": 120000}))
        else {
            panic!("hello")
        };
        assert_eq!(heartbeat, Duration::from_millis(41250));
        let sent = base64::decode_block(init["encoded_public_key"].as_str().unwrap()).unwrap();
        assert_eq!(sent, keys.public);

        let nonce = [0xfb, 0xff, 0x01, 0x02, 0x03];
        let Ok(Step::Send(proof)) =
            handle(&keys, json!({"op": "nonce_proof", "encrypted_nonce": encrypt_to(&keys, &nonce)}))
        else {
            panic!("nonce proof")
        };
        assert_eq!(proof, json!({"op": "nonce_proof", "nonce": "-_8BAgM"}));

        let fingerprint = keys.fingerprint();
        let Ok(Step::Code(url)) = handle(&keys, json!({"op": "pending_remote_init", "fingerprint": fingerprint}))
        else {
            panic!("code")
        };
        assert_eq!(url, format!("https://discord.com/ra/{fingerprint}"));

        let payload = encrypt_to(&keys, b"123:0:0:someone");
        let Ok(Step::Scanned(user)) = handle(&keys, json!({"op": "pending_ticket", "encrypted_user_payload": payload}))
        else {
            panic!("scanned")
        };
        assert_eq!((user.id.0, user.username.as_str(), user.avatar), (123, "someone", None));

        assert!(
            matches!(handle(&keys, json!({"op": "pending_login", "ticket": "t"})), Ok(Step::Ticket(t)) if t == "t")
        );
    }

    #[test]
    fn rejects_a_fingerprint_for_another_key() {
        let keys = Keys::generate().unwrap();
        let other = Keys::generate().unwrap();
        let message = json!({"op": "pending_remote_init", "fingerprint": other.fingerprint()});
        assert!(matches!(handle(&keys, message), Err(Error::Protocol(_))));
    }

    #[test]
    fn user_payloads() {
        let user = parse_user("42:0001:a_abc:name:with:colons").unwrap();
        assert_eq!((user.username.as_str(), user.avatar.as_deref()), ("name:with:colons", Some("a_abc")));
        assert!(parse_user("not an id:0:0:x").is_none());
        assert!(parse_user("42:0").is_none());
    }
}
