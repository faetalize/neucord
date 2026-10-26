//! Every REST endpoint Neucord calls, and the only way to call one: `Rest`
//! sends nothing but a [`Call`], and only an [`Endpoint`] makes one.
//!
//! `tools/discord-watch` reads this file as text, so each endpoint stays on
//! one line: `pub(crate) const NAME: Endpoint = Endpoint::post("/path/{param}");`.
//! Paths name their parameters and never hold a query.

use std::fmt::Display;

pub(crate) const MESSAGES: Endpoint = Endpoint::get("/channels/{channel}/messages");
pub(crate) const SEND_MESSAGE: Endpoint = Endpoint::post("/channels/{channel}/messages");
pub(crate) const ACK: Endpoint = Endpoint::post("/channels/{channel}/messages/{message}/ack");
pub(crate) const USER: Endpoint = Endpoint::get("/users/{user}");
pub(crate) const OPEN_DM: Endpoint = Endpoint::post("/users/@me/channels");
pub(crate) const SEND_FRIEND_REQUEST: Endpoint = Endpoint::post("/users/@me/relationships");
pub(crate) const ACCEPT_FRIEND_REQUEST: Endpoint = Endpoint::put("/users/@me/relationships/{user}");
pub(crate) const REMOVE_RELATIONSHIP: Endpoint = Endpoint::delete("/users/@me/relationships/{user}");
pub(crate) const ACCEPT_MESSAGE_REQUEST: Endpoint = Endpoint::put("/channels/{channel}/recipients/@me");
pub(crate) const REJECT_MESSAGE_REQUEST: Endpoint = Endpoint::delete("/channels/{channel}/recipients/@me");
pub(crate) const LOGIN: Endpoint = Endpoint::post("/auth/login");
pub(crate) const SEND_MFA_SMS: Endpoint = Endpoint::post("/auth/mfa/sms/send");
pub(crate) const VERIFY_MFA: Endpoint = Endpoint::post("/auth/mfa/{method}");
pub(crate) const REMOTE_AUTH_LOGIN: Endpoint = Endpoint::post("/users/@me/remote-auth/login");

/// An HTTP method and a path template.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Endpoint {
    method: &'static str,
    path: &'static str,
}

impl Endpoint {
    const fn get(path: &'static str) -> Self {
        Endpoint { method: "GET", path }
    }

    const fn post(path: &'static str) -> Self {
        Endpoint { method: "POST", path }
    }

    const fn put(path: &'static str) -> Self {
        Endpoint { method: "PUT", path }
    }

    const fn delete(path: &'static str) -> Self {
        Endpoint { method: "DELETE", path }
    }

    /// The call with every `{name}` in the path filled in, escaped; a
    /// parameter missing or left over is a bug, so it panics.
    pub(crate) fn call(self, params: &[(&str, &dyn Display)]) -> Call {
        let mut path = self.path.to_owned();
        for (name, value) in params {
            let param = format!("{{{name}}}");
            assert!(path.contains(&param), "{} has no {param}", self.path);
            path = path.replace(&param, &glib::uri_escape_string(value.to_string(), None::<&str>, false));
        }
        assert!(!path.contains('{'), "{} needs more parameters", self.path);
        Call { method: self.method, path }
    }
}

/// An endpoint with its parameters filled in, ready to send.
#[derive(Debug)]
pub(crate) struct Call {
    method: &'static str,
    path: String,
}

impl Call {
    /// Add a query string, `limit=50&before=…`.
    pub(crate) fn query(mut self, query: &str) -> Self {
        self.path = format!("{}?{query}", self.path);
        self
    }

    pub(crate) fn method(&self) -> &'static str {
        self.method
    }

    /// The path and query, after the API base.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_parameters() {
        let call = ACK.call(&[("message", &2), ("channel", &1)]);
        assert_eq!((call.method(), call.path()), ("POST", "/channels/1/messages/2/ack"));
        assert_eq!(MESSAGES.call(&[("channel", &1)]).query("limit=50").path(), "/channels/1/messages?limit=50");
    }

    #[test]
    fn escapes_values() {
        // Reactions put emoji in the path: unicode, or `name:id` for custom ones.
        assert_eq!(USER.call(&[("user", &"🔥")]).path(), "/users/%F0%9F%94%A5");
        assert_eq!(USER.call(&[("user", &"blob:12/3")]).path(), "/users/blob%3A12%2F3");
    }

    #[test]
    #[should_panic(expected = "needs more parameters")]
    fn missing_parameter() {
        ACK.call(&[("channel", &1)]);
    }

    #[test]
    #[should_panic(expected = "has no {user}")]
    fn unknown_parameter() {
        OPEN_DM.call(&[("user", &1)]);
    }

    /// discord-watch reads the table as text, so a row written any other way
    /// would leave its endpoint unwatched.
    #[test]
    fn table_reads_as_text() {
        let source = include_str!("endpoints.rs");
        let table = &source[source.find("\npub(crate) const").unwrap()..source.find("/// An HTTP method").unwrap()];
        let mut seen = std::collections::HashSet::new();
        let mut rows = 0;
        for line in table.lines().filter(|l| l.contains("Endpoint::")) {
            let row = line
                .strip_prefix("pub(crate) const ")
                .and_then(|l| l.split_once(": Endpoint = Endpoint::"))
                .and_then(|(_, call)| call.strip_suffix("\");"))
                .and_then(|call| call.split_once("(\""));
            let Some((method, path)) = row else { panic!("not in the one-line form: {line}") };
            assert!(["get", "post", "put", "patch", "delete"].contains(&method), "{line}");
            assert!(path.starts_with('/') && !path.contains('?'), "{line}");
            assert!(seen.insert((method, path)), "listed twice: {line}");
            rows += 1;
        }
        assert_eq!(rows, table.matches("Endpoint::").count(), "one endpoint per line");
    }
}
