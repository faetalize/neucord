//! Discord's HTTP API over libsoup, on the GLib main loop.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use soup::prelude::*;

use crate::endpoints::{Call, call};
use crate::{Channel, Error, Message, Snowflake, identity};

/// Error code for accepting a stranger's friend request without confirming.
pub const STRANGER_NOT_CONFIRMED: u64 = 80013;

/// Error code for a message refused because the channel's slowmode hasn't
/// run out for us. Its 429 says how long is left in `retry_after`.
pub const SLOWMODE_RATE_LIMITED: u64 = 20016;

/// One HTTP 429 retry is enough for interactive use; further limits surface as errors.
const MAX_RATE_LIMIT_RETRIES: u32 = 1;

/// The longest a request waits on a 429 before trying again. Past this it
/// fails instead: a retry sent early would only be refused again.
const MAX_RATE_LIMIT_WAIT: f64 = 10.0;

/// Captchas shown for one request before giving up, in case Discord keeps
/// turning the answers down.
const MAX_CAPTCHAS: u32 = 3;

/// Which messages of a channel to fetch.
#[derive(Clone, Copy, Debug)]
pub enum Page {
    Newest,
    /// Older than this message, for scrollback.
    Before(Snowflake),
    /// Newer than this message, for scrolling down from a jump.
    After(Snowflake),
    /// Centred on this message, to jump to it.
    Around(Snowflake),
}

/// Send a message as a reply.
#[derive(Clone, Copy, Debug)]
pub struct Reply {
    pub to: Snowflake,
    /// The server the channel is in; `None` in DMs.
    pub guild: Option<Snowflake>,
    /// Ping the author of the message replied to.
    pub ping: bool,
}

/// A captcha Discord wants solved before it carries out a request.
#[derive(Clone, Debug)]
pub struct Captcha {
    /// `hcaptcha` in practice; the official client also knows `recaptcha`,
    /// `recaptcha_enterprise` and `turnstile`.
    pub service: String,
    pub sitekey: String,
    /// Enterprise challenges fail unless the widget is given this.
    pub rqdata: Option<String>,
    /// Run the widget invisibly; it shows a challenge only if it wants one.
    pub invisible: bool,
    /// Sent back with the answer, as Discord asks.
    rqtoken: Option<String>,
    session_id: Option<String>,
}

impl Captcha {
    fn from_response(body: &[u8]) -> Option<Captcha> {
        let body: Value = serde_json::from_slice(body).ok()?;
        if !is_captcha(&body) {
            return None;
        }
        let text = |key: &str| body[key].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        Some(Captcha {
            service: text("captcha_service").unwrap_or_else(|| "hcaptcha".into()),
            sitekey: text("captcha_sitekey")?,
            rqdata: text("captcha_rqdata"),
            invisible: body["should_serve_invisible"].as_bool().unwrap_or(false),
            rqtoken: text("captcha_rqtoken"),
            session_id: text("captcha_session_id"),
        })
    }
}

/// Shows a captcha and resolves to its answer, or to `None` if it went
/// unsolved.
pub type CaptchaSolver = Rc<dyn Fn(Captcha) -> Pin<Box<dyn Future<Output = Option<String>>>>>;

#[derive(Clone)]
pub struct Rest {
    session: soup::Session,
    token: Option<String>,
    captchas: Option<CaptchaSolver>,
}

impl Rest {
    pub fn new(token: impl Into<String>) -> Self {
        Rest { token: Some(token.into()), ..Rest::anonymous() }
    }

    /// For the login endpoints, before there is a token.
    pub fn anonymous() -> Self {
        let session = soup::Session::new();
        session.set_user_agent(&identity::user_agent());
        session.set_timeout(30);
        Rest { session, token: None, captchas: None }
    }

    /// Have `solver` answer captchas, and send the request again with the
    /// answer, instead of failing with [`Error::Captcha`].
    pub fn solving_captchas(self, solver: CaptchaSolver) -> Self {
        Rest { captchas: Some(solver), ..self }
    }

    /// Up to `limit` (max 100) messages, oldest first, from where `page` says.
    pub async fn messages(&self, channel: Snowflake, limit: u8, page: Page) -> Result<Vec<Message>, Error> {
        let from = match page {
            Page::Newest => String::new(),
            Page::Before(id) => format!("&before={id}"),
            Page::After(id) => format!("&after={id}"),
            Page::Around(id) => format!("&around={id}"),
        };
        let call = call!(MESSAGES, channel).query(&format!("limit={limit}{from}"));
        let mut messages: Vec<Message> = self.request(call, None).await?;
        // Discord sends newest first, whichever way the page goes.
        messages.sort_by_key(|m| m.id);
        Ok(messages)
    }

    /// Look up any user by id, to name a mention whose user we have not seen.
    pub async fn user(&self, id: Snowflake) -> Result<crate::User, Error> {
        self.request(call!(USER, user = id), None).await
    }

    pub async fn send_message(
        &self,
        channel: Snowflake,
        content: &str,
        reply: Option<Reply>,
    ) -> Result<Message, Error> {
        let mut body = json!({
            "content": content,
            "nonce": Snowflake::now().to_string(),
            "tts": false,
            "flags": 0,
        });
        if let Some(reply) = reply {
            // The same body the official client sends.
            body["message_reference"] = json!({ "message_id": reply.to, "channel_id": channel });
            if let Some(guild) = reply.guild {
                body["message_reference"]["guild_id"] = json!(guild);
            }
            if !reply.ping {
                // Mentions in the text still ping; only the reply itself doesn't.
                body["allowed_mentions"] = json!({ "parse": ["users", "roles", "everyone"], "replied_user": false });
            }
        }
        self.request(call!(SEND_MESSAGE, channel), Some(body)).await
    }

    /// Mark everything in `channel` up to `message` as read, for all our clients.
    pub async fn ack(&self, channel: Snowflake, message: Snowflake) -> Result<(), Error> {
        let body = json!({ "token": null });
        let call = call!(ACK, channel, message);
        let _: serde_json::Value = self.request(call, Some(body)).await?;
        Ok(())
    }

    /// Open (or reopen) the DM with `user`.
    pub async fn open_dm(&self, user: Snowflake) -> Result<Channel, Error> {
        self.request(call!(OPEN_DM), Some(json!({ "recipients": [user] }))).await
    }

    /// Send a friend request by username.
    pub async fn send_friend_request(&self, username: &str) -> Result<(), Error> {
        let body = json!({ "username": username, "discriminator": null });
        self.request(call!(SEND_FRIEND_REQUEST), Some(body)).await
    }

    /// Accept `user`'s friend request. A request from a stranger fails with
    /// [`STRANGER_NOT_CONFIRMED`] until sent again with `confirm_stranger`.
    pub async fn accept_friend_request(&self, user: Snowflake, confirm_stranger: bool) -> Result<(), Error> {
        let body = if confirm_stranger { json!({ "confirm_stranger_request": true }) } else { json!({}) };
        self.request(call!(ACCEPT_FRIEND_REQUEST, user), Some(body)).await
    }

    /// End any relationship with `user`: unfriend, unblock, ignore their
    /// request or cancel ours.
    pub async fn remove_relationship(&self, user: Snowflake) -> Result<(), Error> {
        self.request(call!(REMOVE_RELATIONSHIP, user), None).await
    }

    /// Accept a message request; the DM moves into the DM list.
    pub async fn accept_message_request(&self, channel: Snowflake) -> Result<Channel, Error> {
        // 2 is ACCEPTED, the only consent status users may set.
        let body = json!({ "consent_status": 2 });
        self.request(call!(ACCEPT_MESSAGE_REQUEST, channel), Some(body)).await
    }

    /// Reject a message request, which deletes the DM.
    pub async fn reject_message_request(&self, channel: Snowflake) -> Result<(), Error> {
        let call = call!(REJECT_MESSAGE_REQUEST, channel);
        let _: serde_json::Value = self.request(call, None).await?;
        Ok(())
    }

    /// End this token's session, which also takes it off the account's
    /// Devices list. The token stops working.
    pub async fn logout(&self) -> Result<(), Error> {
        // Push-notification registrations, which only the mobile clients have.
        let body = json!({ "provider": null, "token": null, "voip_provider": null, "voip_token": null });
        let _: serde_json::Value = self.request(call!(LOGOUT), Some(body)).await?;
        Ok(())
    }

    pub(crate) async fn request<T: DeserializeOwned>(
        &self,
        call: Call,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        let (status, response) = self.send(call, body).await?;
        match status {
            // 204 No Content reads as `null`, which `()` accepts.
            200..=299 if response.is_empty() => Ok(serde_json::from_value(serde_json::Value::Null)?),
            200..=299 => Ok(serde_json::from_slice(&response)?),
            401 => Err(Error::Unauthorized),
            400 if serde_json::from_slice(&response).is_ok_and(|v| is_captcha(&v)) => Err(Error::Captcha),
            _ => Err(Error::Http { status, body: String::from_utf8_lossy(&response).chars().take(300).collect() }),
        }
    }

    /// The status and body of a request, retrying once on a rate limit and
    /// after each captcha solved.
    pub(crate) async fn send(&self, call: Call, body: Option<Value>) -> Result<(u32, glib::Bytes), Error> {
        let mut retries = 0;
        let mut captchas = 0;
        let mut answer: Option<(String, Captcha)> = None;
        loop {
            let msg = soup::Message::new(call.method(), &format!("{}{}", identity::API_BASE, call.path()))
                .expect("API URL is valid");
            let headers = msg.request_headers().expect("request has headers");
            for (name, value) in base_headers(self.token.as_deref()) {
                headers.replace(name, &value);
            }
            if let Some((key, captcha)) = &answer {
                headers.replace("X-Captcha-Key", key);
                if let Some(rqtoken) = &captcha.rqtoken {
                    headers.replace("X-Captcha-Rqtoken", rqtoken);
                }
                if let Some(id) = &captcha.session_id {
                    headers.replace("X-Captcha-Session-Id", id);
                }
            }
            if let Some(body) = &body {
                let bytes = glib::Bytes::from_owned(body.to_string().into_bytes());
                msg.set_request_body_from_bytes(Some("application/json"), Some(&bytes));
            }

            let response =
                self.session.send_and_read_future(&msg, glib::Priority::DEFAULT).await.map_err(Error::Transport)?;
            let status = msg.status_code();
            if status == 429
                && retries < MAX_RATE_LIMIT_RETRIES
                && let Some(wait) = rate_limit_wait(&response)
            {
                retries += 1;
                glib::timeout_future(std::time::Duration::from_secs_f64(wait)).await;
                continue;
            }
            if status == 400
                && captchas < MAX_CAPTCHAS
                && let Some(solver) = &self.captchas
                && let Some(captcha) = Captcha::from_response(&response)
                && let Some(key) = solver(captcha.clone()).await
            {
                captchas += 1;
                answer = Some((key, captcha));
                continue;
            }
            return Ok((status, response));
        }
    }
}

/// How long to wait before retrying a request Discord rate limited, or
/// `None` to fail at once. Like the official client's message queue, a
/// slowmode refusal is never retried: the cooldown is the user's to wait out.
fn rate_limit_wait(body: &[u8]) -> Option<f64> {
    let body = serde_json::from_slice::<Value>(body).unwrap_or_default();
    if body["code"].as_u64() == Some(SLOWMODE_RATE_LIMITED) {
        return None;
    }
    Some(body["retry_after"].as_f64().unwrap_or(1.0)).filter(|&wait| wait <= MAX_RATE_LIMIT_WAIT)
}

/// The headers every request starts with.
fn base_headers(token: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = vec![("X-Discord-Locale", "en-US".to_owned())];
    match token {
        Some(token) => headers.push(("Authorization", token.to_owned())),
        // Without it, logging in turns down valid two-factor codes.
        None => headers.push(("X-Super-Properties", identity::super_properties())),
    }
    headers
}

/// Discord answers a request it wants a captcha for with a 400 whose body
/// has `captcha_key`, whatever the endpoint.
pub(crate) fn is_captcha(body: &Value) -> bool {
    body.get("captcha_key").is_some()
}

#[cfg(test)]
mod tests {
    use super::{Captcha, base_headers, rate_limit_wait};
    use crate::identity;

    #[test]
    fn logging_in_sends_neucords_properties() {
        let header = |token, name| base_headers(token).into_iter().find(|(n, _)| *n == name).map(|(_, v)| v);
        let properties = header(None, "X-Super-Properties").expect("sent without a token");
        let decoded = openssl::base64::decode_block(&properties).unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&decoded).unwrap(), identity::client_properties());
        assert_eq!(header(None, "Authorization"), None);

        assert_eq!(header(Some("t"), "Authorization").as_deref(), Some("t"));
        assert_eq!(header(Some("t"), "X-Super-Properties"), None);
    }

    #[test]
    fn rate_limit_retries() {
        assert_eq!(rate_limit_wait(br#"{"message": "You are being rate limited.", "retry_after": 0.4}"#), Some(0.4));
        assert_eq!(rate_limit_wait(b""), Some(1.0), "no body: a second, as before");
        assert_eq!(rate_limit_wait(br#"{"retry_after": 30.0}"#), None, "too long to wait for");
        let slowmode = br#"{"message": "You are being rate limited.", "retry_after": 2.0, "code": 20016}"#;
        assert_eq!(rate_limit_wait(slowmode), None, "slowmode is never retried");
    }

    #[test]
    fn captcha_challenges() {
        let body = br#"{"captcha_key": ["captcha-required"], "captcha_sitekey": "k",
            "captcha_service": "hcaptcha", "captcha_session_id": "s", "captcha_rqdata": "d", "captcha_rqtoken": "t",
            "should_serve_invisible": true}"#;
        let captcha = Captcha::from_response(body).unwrap();
        assert_eq!(
            (captcha.service.as_str(), captcha.rqdata.as_deref(), captcha.invisible),
            ("hcaptcha", Some("d"), true)
        );
        assert_eq!((captcha.rqtoken.as_deref(), captcha.session_id.as_deref()), (Some("t"), Some("s")));

        let bare = br#"{"captcha_key": ["captcha-required"], "captcha_sitekey": "k", "captcha_service": "hcaptcha"}"#;
        let captcha = Captcha::from_response(bare).unwrap();
        assert_eq!((captcha.rqdata, captcha.rqtoken, captcha.invisible), (None, None, false));

        assert!(Captcha::from_response(br#"{"captcha_key": ["captcha-required"]}"#).is_none(), "no site key");
        assert!(Captcha::from_response(br#"{"code": 50035, "message": "Invalid Form Body"}"#).is_none());
    }
}
