//! Discord's HTTP API over libsoup, on the GLib main loop.

use serde::de::DeserializeOwned;
use serde_json::json;
use soup::prelude::*;

use crate::{Error, Message, Snowflake, identity};

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
    /// Ping the author of the message replied to.
    pub ping: bool,
}

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

    /// Up to `limit` (max 100) messages, oldest first, from where `page` says.
    pub async fn messages(&self, channel: Snowflake, limit: u8, page: Page) -> Result<Vec<Message>, Error> {
        let from = match page {
            Page::Newest => String::new(),
            Page::Before(id) => format!("&before={id}"),
            Page::After(id) => format!("&after={id}"),
            Page::Around(id) => format!("&around={id}"),
        };
        let mut messages: Vec<Message> =
            self.request("GET", &format!("/channels/{channel}/messages?limit={limit}{from}"), None).await?;
        // Discord sends newest first, whichever way the page goes.
        messages.sort_by_key(|m| m.id);
        Ok(messages)
    }

    /// Look up any user by id, to name a mention whose user we have not seen.
    pub async fn user(&self, id: Snowflake) -> Result<crate::User, Error> {
        self.request("GET", &format!("/users/{id}"), None).await
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
            body["message_reference"] = json!({ "message_id": reply.to, "channel_id": channel });
            if !reply.ping {
                // Mentions in the text still ping; only the reply itself doesn't.
                body["allowed_mentions"] = json!({ "parse": ["users", "roles", "everyone"], "replied_user": false });
            }
        }
        self.request("POST", &format!("/channels/{channel}/messages"), Some(body)).await
    }

    /// Mark everything in `channel` up to `message` as read, for all our clients.
    pub async fn ack(&self, channel: Snowflake, message: Snowflake) -> Result<(), Error> {
        let body = json!({ "token": null });
        let _: serde_json::Value =
            self.request("POST", &format!("/channels/{channel}/messages/{message}/ack"), Some(body)).await?;
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
                200..=299 => return Ok(serde_json::from_slice(&response)?),
                401 => return Err(Error::Unauthorized),
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
