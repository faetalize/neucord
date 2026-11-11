//! Every REST endpoint Neucord calls, and the only way to call one: `Rest`
//! sends nothing but a [`Call`], and only [`call!`] on an [`Endpoint`] makes
//! one, checking its parameters against the path at compile time.
//!
//! `tools/discord-watch` reads this file as text, so each endpoint stays on
//! one line: `pub(crate) const NAME: Endpoint = Endpoint::post("/path/{param}");`.
//! Paths name their parameters and never hold a query. A parameter written
//! `{+name}` keeps its `/`, for a value that is itself a path.

use std::fmt::Display;

pub(crate) const MESSAGES: Endpoint = Endpoint::get("/channels/{channel}/messages");
pub(crate) const SEND_MESSAGE: Endpoint = Endpoint::post("/channels/{channel}/messages");
pub(crate) const EDIT_MESSAGE: Endpoint = Endpoint::patch("/channels/{channel}/messages/{message}");
pub(crate) const DELETE_MESSAGE: Endpoint = Endpoint::delete("/channels/{channel}/messages/{message}");
// Kept on one line for discord-watch, past what rustfmt would.
#[rustfmt::skip]
pub(crate) const ADD_REACTION: Endpoint = Endpoint::put("/channels/{channel}/messages/{message}/reactions/{+emoji}/@me");
pub(crate) const REACTORS: Endpoint = Endpoint::get("/channels/{channel}/messages/{message}/reactions/{+emoji}");
#[rustfmt::skip]
pub(crate) const REMOVE_REACTION: Endpoint = Endpoint::delete("/channels/{channel}/messages/{message}/reactions/{+emoji}/0/{+user}");
#[rustfmt::skip]
pub(crate) const REMOVE_EMOJI_REACTIONS: Endpoint = Endpoint::delete("/channels/{channel}/messages/{message}/reactions/{+emoji}");
pub(crate) const REMOVE_REACTIONS: Endpoint = Endpoint::delete("/channels/{channel}/messages/{message}/reactions");
pub(crate) const LIST_SCHEDULED_MESSAGES: Endpoint = Endpoint::get("/users/@me/scheduled-messages");
pub(crate) const SCHEDULE_MESSAGE: Endpoint = Endpoint::post("/users/@me/scheduled-messages");
pub(crate) const EDIT_SCHEDULED_MESSAGE: Endpoint = Endpoint::patch("/users/@me/scheduled-messages/{scheduled}");
pub(crate) const CANCEL_SCHEDULED_MESSAGE: Endpoint = Endpoint::delete("/users/@me/scheduled-messages/{scheduled}");
pub(crate) const SEND_SCHEDULED_MESSAGE: Endpoint = Endpoint::post("/users/@me/scheduled-messages/{scheduled}/send");
pub(crate) const CREATE_ATTACHMENT: Endpoint = Endpoint::post("/channels/{channel}/attachments");
pub(crate) const DELETE_UPLOAD: Endpoint = Endpoint::delete("/attachments/{+uploaded_filename}");
pub(crate) const VOTE: Endpoint = Endpoint::put("/channels/{channel}/polls/{message}/answers/@me");
pub(crate) const END_POLL: Endpoint = Endpoint::post("/channels/{channel}/polls/{message}/expire");
pub(crate) const INTERACT: Endpoint = Endpoint::post("/interactions");
pub(crate) const COMMAND_INDEX_GUILD: Endpoint = Endpoint::get("/guilds/{guild}/application-command-index");
pub(crate) const COMMAND_INDEX_CHANNEL: Endpoint = Endpoint::get("/channels/{channel}/application-command-index");
pub(crate) const COMMAND_INDEX_USER: Endpoint = Endpoint::get("/users/@me/application-command-index");
pub(crate) const TYPING: Endpoint = Endpoint::post("/channels/{channel}/typing");
pub(crate) const ACK: Endpoint = Endpoint::post("/channels/{channel}/messages/{message}/ack");
pub(crate) const CREATE_THREAD: Endpoint = Endpoint::post("/channels/{channel}/threads");
#[rustfmt::skip]
pub(crate) const CREATE_THREAD_FROM_MESSAGE: Endpoint = Endpoint::post("/channels/{channel}/messages/{message}/threads");
pub(crate) const GET_CHANNEL: Endpoint = Endpoint::get("/channels/{channel}");
pub(crate) const EDIT_CHANNEL: Endpoint = Endpoint::patch("/channels/{channel}");
pub(crate) const SEARCH_THREADS: Endpoint = Endpoint::get("/channels/{channel}/threads/search");
pub(crate) const JOIN_THREAD: Endpoint = Endpoint::post("/channels/{channel}/thread-members/@me");
pub(crate) const LEAVE_THREAD: Endpoint = Endpoint::delete("/channels/{channel}/thread-members/@me");
pub(crate) const POST_DATA: Endpoint = Endpoint::post("/channels/{channel}/post-data");
pub(crate) const USER: Endpoint = Endpoint::get("/users/{user}");
pub(crate) const USER_PROFILE: Endpoint = Endpoint::get("/users/{user}/profile");
pub(crate) const MUTUAL_FRIENDS: Endpoint = Endpoint::get("/users/{user}/relationships");
pub(crate) const NOTE: Endpoint = Endpoint::get("/users/@me/notes/{user}");
pub(crate) const SET_NOTE: Endpoint = Endpoint::put("/users/@me/notes/{user}");
pub(crate) const OPEN_DM: Endpoint = Endpoint::post("/users/@me/channels");
pub(crate) const SEND_FRIEND_REQUEST: Endpoint = Endpoint::post("/users/@me/relationships");
pub(crate) const ACCEPT_FRIEND_REQUEST: Endpoint = Endpoint::put("/users/@me/relationships/{user}");
pub(crate) const REMOVE_RELATIONSHIP: Endpoint = Endpoint::delete("/users/@me/relationships/{user}");
pub(crate) const ACCEPT_MESSAGE_REQUEST: Endpoint = Endpoint::put("/channels/{channel}/recipients/@me");
pub(crate) const REJECT_MESSAGE_REQUEST: Endpoint = Endpoint::delete("/channels/{channel}/recipients/@me");
pub(crate) const LOGIN: Endpoint = Endpoint::post("/auth/login");
pub(crate) const LOGOUT: Endpoint = Endpoint::post("/auth/logout");
pub(crate) const SEND_MFA_SMS: Endpoint = Endpoint::post("/auth/mfa/sms/send");
pub(crate) const VERIFY_MFA: Endpoint = Endpoint::post("/auth/mfa/{method}");
pub(crate) const FORGOT_PASSWORD: Endpoint = Endpoint::post("/auth/forgot");
pub(crate) const REMOTE_AUTH_LOGIN: Endpoint = Endpoint::post("/users/@me/remote-auth/login");
pub(crate) const STICKER_PACKS: Endpoint = Endpoint::get("/sticker-packs");
pub(crate) const GIFS_SEARCH: Endpoint = Endpoint::get("/gifs/search");
pub(crate) const GIFS_TRENDING: Endpoint = Endpoint::get("/gifs/trending");
pub(crate) const GIFS_TRENDING_GIFS: Endpoint = Endpoint::get("/gifs/trending-gifs");
pub(crate) const GIFS_SUGGEST: Endpoint = Endpoint::get("/gifs/suggest");
pub(crate) const SAFETY_HUB: Endpoint = Endpoint::get("/safety-hub/@me");
pub(crate) const SAFETY_HUB_SUSPENDED: Endpoint = Endpoint::post("/safety-hub/suspended/@me");

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

    const fn patch(path: &'static str) -> Self {
        Endpoint { method: "PATCH", path }
    }

    const fn delete(path: &'static str) -> Self {
        Endpoint { method: "DELETE", path }
    }

    /// Panics unless `names` are exactly the path's parameters. [`call!`]
    /// runs it as a `const`, so a wrong name fails the build.
    pub(crate) const fn check(self, names: &[&str]) {
        let path = self.path.as_bytes();
        let (mut i, mut params) = (0, 0);
        while i < path.len() {
            if path[i] == b'{' {
                let start = if path[i + 1] == b'+' { i + 2 } else { i + 1 };
                while path[i] != b'}' {
                    i += 1;
                }
                if !names_contain(names, path, start, i) {
                    panic!("call! leaves out a parameter of this endpoint");
                }
                params += 1;
            }
            i += 1;
        }
        if params != names.len() {
            panic!("call! names a parameter this endpoint doesn't have");
        }
    }

    /// The call with every `{name}` in the path filled in, escaped. Use
    /// [`call!`], which checks the names first.
    pub(crate) fn fill(self, params: &[(&str, &dyn Display)]) -> Call {
        let mut path = self.path.to_owned();
        for (name, value) in params {
            let value = value.to_string();
            let plain = glib::uri_escape_string(&value, None::<&str>, false);
            path = path.replace(&format!("{{{name}}}"), &plain);
            // Whatever a path may hold, as a browser leaves it.
            let nested = glib::uri_escape_string(&value, Some("/!$&'()*+,;=:@"), false);
            path = path.replace(&format!("{{+{name}}}"), &nested);
        }
        Call { method: self.method, path }
    }
}

/// Whether `path[start..end]` is one of `names`, in a `const fn`.
const fn names_contain(names: &[&str], path: &[u8], start: usize, end: usize) -> bool {
    let mut n = 0;
    while n < names.len() {
        let name = names[n].as_bytes();
        let mut same = name.len() == end - start;
        let mut i = 0;
        while same && i < name.len() {
            same = name[i] == path[start + i];
            i += 1;
        }
        if same {
            return true;
        }
        n += 1;
    }
    false
}

/// An endpoint's [`Call`]: `call!(ACK, channel, message)` passes the
/// variables named like its parameters, `call!(USER, user = id)` anything
/// else.
macro_rules! call {
    ($endpoint:ident $(, $name:ident $(= $value:expr)?)* $(,)?) => {{
        // An item, not a `const {}` block: those go unevaluated in an
        // `async fn` until something polls it.
        const _: () = $crate::endpoints::$endpoint.check(&[$(stringify!($name)),*]);
        $crate::endpoints::$endpoint
            .fill(&[$((stringify!($name), &call!(@value $name $(= $value)?) as &dyn std::fmt::Display)),*])
    }};
    (@value $name:ident = $value:expr) => {
        $value
    };
    (@value $name:ident) => {
        $name
    };
}
pub(crate) use call;

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
        let channel = 1;
        let call = call!(ACK, message = 2, channel);
        assert_eq!((call.method(), call.path()), ("POST", "/channels/1/messages/2/ack"));
        assert_eq!(call!(MESSAGES, channel).query("limit=50").path(), "/channels/1/messages?limit=50");
        assert_eq!(call!(LOGIN).path(), "/auth/login");
        let (vote, end) = (call!(VOTE, channel, message = 4), call!(END_POLL, channel, message = 4));
        assert_eq!((vote.method(), vote.path()), ("PUT", "/channels/1/polls/4/answers/@me"));
        assert_eq!((end.method(), end.path()), ("POST", "/channels/1/polls/4/expire"));
    }

    #[test]
    fn plus_keeps_slashes() {
        let call = call!(DELETE_UPLOAD, uploaded_filename = "a-1/my file.png");
        assert_eq!((call.method(), call.path()), ("DELETE", "/attachments/a-1/my%20file.png"));
        let odd = call!(DELETE_UPLOAD, uploaded_filename = "u/a+(1)=b.png");
        assert_eq!(odd.path(), "/attachments/u/a+(1)=b.png", "a browser leaves these as they are");
        assert_eq!(call!(USER, user = "a/b").path(), "/users/a%2Fb", "plain parameters still escape it");
    }

    #[test]
    fn escapes_values() {
        // Reactions put emoji in the path: unicode, or `name:id` for custom ones.
        assert_eq!(call!(USER, user = "🔥").path(), "/users/%F0%9F%94%A5");
        assert_eq!(call!(USER, user = "blob:12/3").path(), "/users/blob%3A12%2F3");
    }

    #[test]
    fn reactions_name_their_emoji_as_a_browser_writes_the_path() {
        let (channel, message) = (1, 2);
        let emoji = "blob:12";
        let call = call!(ADD_REACTION, channel, message, emoji);
        assert_eq!((call.method(), call.path()), ("PUT", "/channels/1/messages/2/reactions/blob:12/@me"));
        let emoji = "🔥";
        let user = "@me";
        let call = call!(REMOVE_REACTION, channel, message, emoji, user);
        assert_eq!((call.method(), call.path()), ("DELETE", "/channels/1/messages/2/reactions/%F0%9F%94%A5/0/@me"));
        let user = 7;
        let call = call!(REMOVE_REACTION, channel, message, emoji, user);
        assert_eq!(call.path(), "/channels/1/messages/2/reactions/%F0%9F%94%A5/0/7");
        let call = call!(REMOVE_EMOJI_REACTIONS, channel, message, emoji);
        assert_eq!((call.method(), call.path()), ("DELETE", "/channels/1/messages/2/reactions/%F0%9F%94%A5"));
        assert_eq!(call!(REACTORS, channel, message, emoji).method(), "GET");
        assert_eq!(call!(REMOVE_REACTIONS, channel, message).path(), "/channels/1/messages/2/reactions");
    }

    // Run here at runtime; call! runs them at compile time.
    #[test]
    #[should_panic(expected = "leaves out a parameter")]
    fn missing_parameter() {
        ACK.check(&["channel"]);
    }

    #[test]
    #[should_panic(expected = "leaves out a parameter")]
    fn misspelled_parameter() {
        USER.check(&["usr"]);
    }

    #[test]
    #[should_panic(expected = "doesn't have")]
    fn extra_parameter() {
        OPEN_DM.check(&["user"]);
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
