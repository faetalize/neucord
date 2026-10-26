//! Discord's HTTP API over libsoup, on the GLib main loop.

use serde::de::DeserializeOwned;
use serde_json::json;
use soup::prelude::*;

use crate::{Channel, Error, Message, Snowflake, identity};

/// Error code for accepting a stranger's friend request without confirming.
pub const STRANGER_NOT_CONFIRMED: u64 = 80013;

/// One HTTP 429 retry is enough for interactive use; further limits surface as errors.
const MAX_RATE_LIMIT_RETRIES: u32 = 1;

#[derive(Clone)]
pub struct Rest {
    session: soup::Session,
    token: String,
}

impl Rest {
    pub fn new(token: impl Into<String>) -> Self {
        let session = soup::Session::new();
        session.set_user_agent(&identity::user_agent());
        session.set_timeout(30);
        Rest { session, token: token.into() }
    }

    /// Up to `limit` (max 100) messages, oldest first: the newest ones, or with
    /// `before` the ones just older than that message (for scrollback).
    pub async fn messages(
        &self,
        channel: Snowflake,
        limit: u8,
        before: Option<Snowflake>,
    ) -> Result<Vec<Message>, Error> {
        let before = before.map(|id| format!("&before={id}")).unwrap_or_default();
        let mut messages: Vec<Message> =
            self.request("GET", &format!("/channels/{channel}/messages?limit={limit}{before}"), None).await?;
        messages.reverse();
        Ok(messages)
    }

    /// Look up any user by id, to name a mention whose user we have not seen.
    pub async fn user(&self, id: Snowflake) -> Result<crate::User, Error> {
        self.request("GET", &format!("/users/{id}"), None).await
    }

    pub async fn send_message(&self, channel: Snowflake, content: &str) -> Result<Message, Error> {
        let body = json!({
            "content": content,
            "nonce": Snowflake::now().to_string(),
            "tts": false,
            "flags": 0,
        });
        self.request("POST", &format!("/channels/{channel}/messages"), Some(body)).await
    }

    /// Mark everything in `channel` up to `message` as read, for all our clients.
    pub async fn ack(&self, channel: Snowflake, message: Snowflake) -> Result<(), Error> {
        let body = json!({ "token": null });
        let _: serde_json::Value =
            self.request("POST", &format!("/channels/{channel}/messages/{message}/ack"), Some(body)).await?;
        Ok(())
    }

    /// Open (or reopen) the DM with `user`.
    pub async fn open_dm(&self, user: Snowflake) -> Result<Channel, Error> {
        self.request("POST", "/users/@me/channels", Some(json!({ "recipients": [user] }))).await
    }

    /// Send a friend request by username.
    pub async fn send_friend_request(&self, username: &str) -> Result<(), Error> {
        let body = json!({ "username": username, "discriminator": null });
        self.request("POST", "/users/@me/relationships", Some(body)).await
    }

    /// Accept `user`'s friend request. A request from a stranger fails with
    /// [`STRANGER_NOT_CONFIRMED`] until sent again with `confirm_stranger`.
    pub async fn accept_friend_request(&self, user: Snowflake, confirm_stranger: bool) -> Result<(), Error> {
        let body = if confirm_stranger { json!({ "confirm_stranger_request": true }) } else { json!({}) };
        self.request("PUT", &format!("/users/@me/relationships/{user}"), Some(body)).await
    }

    /// End any relationship with `user`: unfriend, unblock, ignore their
    /// request or cancel ours.
    pub async fn remove_relationship(&self, user: Snowflake) -> Result<(), Error> {
        self.request("DELETE", &format!("/users/@me/relationships/{user}"), None).await
    }

    /// Accept a message request; the DM moves into the DM list.
    pub async fn accept_message_request(&self, channel: Snowflake) -> Result<Channel, Error> {
        // 2 is ACCEPTED, the only consent status users may set.
        let body = json!({ "consent_status": 2 });
        self.request("PUT", &format!("/channels/{channel}/recipients/@me"), Some(body)).await
    }

    /// Reject a message request, which deletes the DM.
    pub async fn reject_message_request(&self, channel: Snowflake) -> Result<(), Error> {
        let _: serde_json::Value = self.request("DELETE", &format!("/channels/{channel}/recipients/@me"), None).await?;
        Ok(())
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        let mut retries = 0;
        loop {
            let msg = soup::Message::new(method, &format!("{}{path}", identity::API_BASE)).expect("API URL is valid");
            let headers = msg.request_headers().expect("request has headers");
            headers.replace("Authorization", &self.token);
            headers.replace("X-Discord-Locale", "en-US");
            if let Some(body) = &body {
                let bytes = glib::Bytes::from_owned(body.to_string().into_bytes());
                msg.set_request_body_from_bytes(Some("application/json"), Some(&bytes));
            }

            let response =
                self.session.send_and_read_future(&msg, glib::Priority::DEFAULT).await.map_err(Error::Transport)?;
            let status = msg.status_code();
            match status {
                // 204 No Content reads as `null`, which `()` accepts.
                200..=299 if response.is_empty() => return Ok(serde_json::from_value(serde_json::Value::Null)?),
                200..=299 => return Ok(serde_json::from_slice(&response)?),
                401 => return Err(Error::Unauthorized),
                400 if serde_json::from_slice::<serde_json::Value>(&response)
                    .is_ok_and(|v| v.get("captcha_key").is_some()) =>
                {
                    return Err(Error::Captcha);
                }
                429 if retries < MAX_RATE_LIMIT_RETRIES => {
                    retries += 1;
                    let wait = serde_json::from_slice::<serde_json::Value>(&response)
                        .ok()
                        .and_then(|v| v["retry_after"].as_f64())
                        .unwrap_or(1.0);
                    glib::timeout_future(std::time::Duration::from_secs_f64(wait.min(10.0))).await;
                }
                _ => {
                    return Err(Error::Http {
                        status,
                        body: String::from_utf8_lossy(&response).chars().take(300).collect(),
                    });
                }
            }
        }
    }
}
