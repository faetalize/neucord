//! Discord's HTTP API over libsoup, on the GLib main loop.

use serde::de::DeserializeOwned;
use serde_json::json;
use soup::prelude::*;

use crate::endpoints::{Call, call};
use crate::{Channel, Error, Message, Snowflake, identity};

/// Error code for accepting a stranger's friend request without confirming.
pub const STRANGER_NOT_CONFIRMED: u64 = 80013;

/// One HTTP 429 retry is enough for interactive use; further limits surface as errors.
const MAX_RATE_LIMIT_RETRIES: u32 = 1;

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

#[derive(Clone)]
pub struct Rest {
    session: soup::Session,
    token: Option<String>,
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
        Rest { session, token: None }
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

    async fn request<T: DeserializeOwned>(&self, call: Call, body: Option<serde_json::Value>) -> Result<T, Error> {
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

    /// The status and body of a request, retrying once on a rate limit.
    pub(crate) async fn send(&self, call: Call, body: Option<serde_json::Value>) -> Result<(u32, glib::Bytes), Error> {
        let mut retries = 0;
        loop {
            let msg = soup::Message::new(call.method(), &format!("{}{}", identity::API_BASE, call.path()))
                .expect("API URL is valid");
            let headers = msg.request_headers().expect("request has headers");
            if let Some(token) = &self.token {
                headers.replace("Authorization", token);
            }
            headers.replace("X-Discord-Locale", "en-US");
            if let Some(body) = &body {
                let bytes = glib::Bytes::from_owned(body.to_string().into_bytes());
                msg.set_request_body_from_bytes(Some("application/json"), Some(&bytes));
            }

            let response =
                self.session.send_and_read_future(&msg, glib::Priority::DEFAULT).await.map_err(Error::Transport)?;
            let status = msg.status_code();
            if status != 429 || retries >= MAX_RATE_LIMIT_RETRIES {
                return Ok((status, response));
            }
            retries += 1;
            let wait = serde_json::from_slice::<serde_json::Value>(&response)
                .ok()
                .and_then(|v| v["retry_after"].as_f64())
                .unwrap_or(1.0);
            glib::timeout_future(std::time::Duration::from_secs_f64(wait.min(10.0))).await;
        }
    }
}

/// Discord answers a request it wants a captcha for with a 400 whose body
/// has `captcha_key`, whatever the endpoint.
pub(crate) fn is_captcha(body: &serde_json::Value) -> bool {
    body.get("captcha_key").is_some()
}
