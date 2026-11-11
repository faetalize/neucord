//! Discord's HTTP API over libsoup, on the GLib main loop.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use soup::prelude::*;

use crate::endpoints::{Call, call};
use crate::forum::Search;
use crate::gifs::{Gif, Picker, Request, Tile, Tiles};
use crate::interaction::{Failure, Press};
use crate::invalid::{Change, InvalidRequests};
use crate::model::Emoji;
use crate::post_creation;
use crate::profile::{self, Profile};
use crate::reactions::{Location, Refusal};
use crate::scheduled::{self, Scheduled};
use crate::stickers::Pack;
use crate::threads::ArchivedPage;
use crate::uploads::Sent;
use crate::{Channel, Error, Message, Snowflake, identity, notes};

/// Error code for accepting a stranger's friend request without confirming.
pub const STRANGER_NOT_CONFIRMED: u64 = 80013;

/// Error code for a message refused because the channel's slowmode hasn't
/// run out for us. Its 429 says how long is left in `retry_after`.
pub const SLOWMODE_RATE_LIMITED: u64 = 20016;

/// One HTTP 429 retry is enough for interactive use; further limits surface as errors.
const MAX_RATE_LIMIT_RETRIES: u32 = 1;

/// How long the client lets an interaction request take (`timeout` in 964363).
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(3);

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

impl Page {
    /// In the order the client's `fetchMessages` builds it (138399).
    fn query(self, limit: u8) -> String {
        match self {
            Page::Newest => format!("limit={limit}"),
            Page::Before(id) => format!("before={id}&limit={limit}"),
            Page::After(id) => format!("after={id}&limit={limit}"),
            Page::Around(id) => format!("limit={limit}&around={id}"),
        }
    }
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

/// Whether a reply pings its author, as Reply sets it up in the official
/// client (`shouldMention`, `showMentionToggle` in the pending reply).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyPing {
    /// The ping's starting state, and its final one where there is no toggle.
    pub ping: bool,
    /// Whether the reply bar offers the ping toggle.
    pub toggle: bool,
}

impl ReplyPing {
    /// For a click on Reply. `own`: the message is ours. `private`: the
    /// channel is a DM or group DM. `shift`: Shift was held, which turns
    /// the ping off whether or not the toggle is there.
    pub fn for_click(own: bool, private: bool, shift: bool) -> Self {
        Self { ping: !shift && !own, toggle: !private && !own }
    }

    /// The ping where the toggle doesn't decide it.
    pub fn fixed(self) -> Option<bool> {
        (!self.toggle).then_some(self.ping)
    }
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

/// Told how long requests are held back for, each time one is.
pub type PauseHandler = Rc<dyn Fn(Duration)>;

thread_local! {
    /// One for every [`Rest`]: Discord counts refusals per IP, not per token.
    static INVALID: RefCell<InvalidRequests> = RefCell::default();
}

/// What sets one request apart from the usual.
#[derive(Clone, Copy)]
struct Sending<'a> {
    /// Try again once after a 429, which is how requests usually go.
    retry_rate_limit: bool,
    /// Requests that failed before this one, for `X-Failed-Requests`.
    failed: u32,
    /// The hash of a file before it was converted, for
    /// `X-Discord-Original-MD5` (724165's `buildHeadersForMd5`).
    original_md5: Option<&'a str>,
}

impl Default for Sending<'_> {
    fn default() -> Self {
        Sending { retry_rate_limit: true, failed: 0, original_md5: None }
    }
}

#[derive(Clone)]
pub struct Rest {
    session: soup::Session,
    token: Option<String>,
    captchas: Option<CaptchaSolver>,
    pauses: Option<PauseHandler>,
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
        Rest { session, token: None, captchas: None, pauses: None }
    }

    /// Have `solver` answer captchas, and send the request again with the
    /// answer, instead of failing with [`Error::Captcha`].
    pub fn solving_captchas(self, solver: CaptchaSolver) -> Self {
        Rest { captchas: Some(solver), ..self }
    }

    /// Have `handler` told when Discord has refused so many requests that
    /// they are held back, failing with [`Error::Paused`].
    pub fn reporting_pauses(self, handler: PauseHandler) -> Self {
        Rest { pauses: Some(handler), ..self }
    }

    /// Up to `limit` (max 100) messages, oldest first, from where `page` says.
    pub async fn messages(&self, channel: Snowflake, limit: u8, page: Page) -> Result<Vec<Message>, Error> {
        let call = call!(MESSAGES, channel).query(&page.query(limit));
        let mut messages: Vec<Message> = self.request(call, None).await?;
        // Discord sends newest first, whichever way the page goes.
        messages.sort_by_key(|m| m.id);
        Ok(messages)
    }

    /// Look up any user by id, to name a mention whose user we have not seen.
    pub async fn user(&self, id: Snowflake) -> Result<crate::User, Error> {
        self.request(call!(USER, user = id), None).await
    }

    /// A profile, with the query a popout sends (`eO` in 803306).
    pub async fn profile(&self, request: &profile::Request) -> Result<Profile, Error> {
        let call = call!(USER_PROFILE, user = request.user).query(&request.query());
        let answer: Value = self.request(call, None).await?;
        Profile::from_value(&answer).ok_or_else(|| Error::Protocol("a profile without a user".into()))
    }

    /// Our note on `user`, which doesn't exist (a 404) until we write one.
    pub async fn note(&self, user: Snowflake) -> Result<Option<String>, Error> {
        let answer: Value = self.request(call!(NOTE, user), None).await?;
        Ok(notes::from_answer(&answer, user))
    }

    pub async fn set_note(&self, user: Snowflake, note: &str) -> Result<(), Error> {
        self.request(call!(SET_NOTE, user), Some(json!({ "note": note }))).await
    }

    /// Discord's standard sticker packs, as the client fetches them when the
    /// sticker picker opens (`fetchStickerPacks` in 631576): the locale is
    /// the only query field.
    pub async fn sticker_packs(&self) -> Result<Vec<Pack>, Error> {
        let call = call!(STICKER_PACKS).query(&format!("locale={}", identity::LOCALE));
        let answer: Value = self.request(call, None).await?;
        Ok(Pack::list(&answer))
    }

    /// What the GIF picker asks Discord for (never the provider), as the
    /// client does when [`crate::gifs::Picker`] says. The queries are
    /// [`Request::query`]'s.
    pub async fn gif_tiles(&self) -> Result<Vec<Tile>, Error> {
        let answer: Value = self.request(call!(GIFS_TRENDING).query(&Request::Trending.query()), None).await?;
        Ok(Tiles::parse(&answer))
    }

    pub async fn gifs_trending(&self) -> Result<Vec<Gif>, Error> {
        let answer: Value = self.request(call!(GIFS_TRENDING_GIFS).query(&Request::TrendingGifs.query()), None).await?;
        Ok(Gif::list(&answer))
    }

    pub async fn gifs_search(&self, query: &str) -> Result<Vec<Gif>, Error> {
        let answer: Value =
            self.request(call!(GIFS_SEARCH).query(&Request::Search(query.to_owned()).query()), None).await?;
        Ok(Gif::list(&answer))
    }

    pub async fn gifs_suggest(&self, query: &str) -> Result<Vec<String>, Error> {
        let answer: Value =
            self.request(call!(GIFS_SUGGEST).query(&Request::Suggest(query.to_owned()).query()), None).await?;
        Ok(Picker::parse_suggestions(&answer))
    }

    /// Post `content`, the uploaded `attachments` and a `sticker`, which
    /// with any one of them may be all of it.
    pub async fn send_message(
        &self,
        channel: Snowflake,
        content: &str,
        reply: Option<Reply>,
        attachments: &[Sent<'_>],
        sticker: Option<Snowflake>,
    ) -> Result<Message, Error> {
        let body = message_body(channel, content, &Snowflake::now().to_string(), 0, reply, attachments, sticker);
        self.request(call!(SEND_MESSAGE, channel), Some(body)).await
    }

    /// Post a recorded voice message: no text, `IS_VOICE_MESSAGE`, and the one
    /// uploaded recording (`em` in 841817).
    pub async fn send_voice_message(
        &self,
        channel: Snowflake,
        reply: Option<Reply>,
        recording: Sent<'_>,
    ) -> Result<Message, Error> {
        let flags = Message::IS_VOICE_MESSAGE;
        let body = message_body(channel, "", &Snowflake::now().to_string(), flags, reply, &[recording], None);
        self.request(call!(SEND_MESSAGE, channel), Some(body)).await
    }

    /// Post `poll` (a [`crate::poll_draft::Draft::body`]) on its own: the
    /// editor's `createPoll` (180662) sends no text, reply or files with it.
    pub async fn send_poll(&self, channel: Snowflake, poll: Value) -> Result<Message, Error> {
        let body = poll_body(channel, &Snowflake::now().to_string(), poll);
        self.request(call!(SEND_MESSAGE, channel), Some(body)).await
    }

    /// Our scheduled messages, which the client asks for when the inbox's
    /// Scheduled tab opens, and not before (713024).
    pub async fn scheduled_messages(&self) -> Result<Vec<Scheduled>, Error> {
        let list: Value = self.request(call!(LIST_SCHEDULED_MESSAGES), None).await?;
        Ok(Scheduled::parse_list(&list))
    }

    /// Have Discord post `content`, the uploaded `attachments`, a `sticker`
    /// or a `poll` (a [`crate::poll_draft::Draft::body`], which then stands
    /// alone) in `channel` at `at`, an ISO 8601 time in UTC.
    #[expect(clippy::too_many_arguments, reason = "the fields of one request")]
    pub async fn schedule_message(
        &self,
        channel: Snowflake,
        at: &str,
        content: &str,
        reply: Option<Reply>,
        attachments: &[Sent<'_>],
        sticker: Option<Snowflake>,
        poll: Option<Value>,
    ) -> Result<(), Error> {
        let body = schedule_body(channel, at, content, reply, attachments, sticker, poll);
        let _: Value = self.request(call!(SCHEDULE_MESSAGE), Some(body)).await?;
        Ok(())
    }

    /// Move scheduled message `scheduled` to `at`, an ISO 8601 time in UTC.
    pub async fn move_scheduled_message(&self, scheduled: Snowflake, at: &str) -> Result<Scheduled, Error> {
        let body = json!({ "scheduled_timestamp": at });
        self.change_scheduled_message(scheduled, body).await
    }

    /// Change the text of scheduled message `scheduled`, whose flags are `flags`.
    pub async fn edit_scheduled_message(
        &self,
        scheduled: Snowflake,
        content: &str,
        flags: u64,
    ) -> Result<Scheduled, Error> {
        self.change_scheduled_message(scheduled, edit_scheduled_body(content, flags)).await
    }

    async fn change_scheduled_message(&self, scheduled: Snowflake, body: Value) -> Result<Scheduled, Error> {
        let sent: Value = self.request(call!(EDIT_SCHEDULED_MESSAGE, scheduled), Some(body)).await?;
        Scheduled::parse(&sent).ok_or_else(|| Error::Protocol("a scheduled message without its id or channel".into()))
    }

    /// Take scheduled message `scheduled` back, unsent.
    pub async fn cancel_scheduled_message(&self, scheduled: Snowflake) -> Result<(), Error> {
        self.request(call!(CANCEL_SCHEDULED_MESSAGE, scheduled), None).await
    }

    /// Have Discord post scheduled message `scheduled` now.
    pub async fn send_scheduled_message_now(&self, scheduled: Snowflake) -> Result<(), Error> {
        self.request(call!(SEND_SCHEDULED_MESSAGE, scheduled), None).await
    }

    /// Change the text of `message`, ours, to `content`.
    pub async fn edit_message(&self, message: &Message, content: &str) -> Result<Message, Error> {
        let call = call!(EDIT_MESSAGE, channel = message.channel_id, message = message.id);
        self.request(call, Some(edit_body(message, content))).await
    }

    /// Delete `message`, ours or, with Manage Messages, anyone's.
    pub async fn delete_message(&self, channel: Snowflake, message: Snowflake) -> Result<(), Error> {
        self.request(call!(DELETE_MESSAGE, channel, message), None).await
    }

    /// React to `message` with `emoji`, once and without retrying a 429: what
    /// to do about a refusal is [`crate::reactions::judge`]'s (`BB` in
    /// 649963).
    pub async fn add_reaction(
        &self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &Emoji,
        location: Location,
    ) -> Result<(), Refusal> {
        self.reaction_request(add_reaction_call(channel, message, emoji, location)).await
    }

    /// Take back a reaction, ours or with Manage Messages another's, as
    /// [`Self::add_reaction`] sends it (`et` in 649963).
    pub async fn remove_reaction(
        &self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &Emoji,
        location: Location,
        user: Option<Snowflake>,
    ) -> Result<(), Refusal> {
        self.reaction_request(remove_reaction_call(channel, message, emoji, location, user)).await
    }

    /// Remove everyone's reactions with `emoji` (`Jf` in 649963).
    pub async fn remove_emoji_reactions(
        &self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &Emoji,
    ) -> Result<(), Refusal> {
        let emoji = emoji.path();
        self.reaction_request(call!(REMOVE_EMOJI_REACTIONS, channel, message, emoji)).await
    }

    /// Remove every reaction from `message` (`Go` in 649963).
    pub async fn remove_reactions(&self, channel: Snowflake, message: Snowflake) -> Result<(), Refusal> {
        self.reaction_request(call!(REMOVE_REACTIONS, channel, message)).await
    }

    /// Up to `limit` of the people who reacted with `emoji` normally, after
    /// `after` (`ao` in 649963). Not retried on a 429, as the client doesn't.
    pub async fn reactors(
        &self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &Emoji,
        limit: u32,
        after: Option<Snowflake>,
    ) -> Result<Vec<crate::User>, Error> {
        let sending = Sending { retry_rate_limit: false, ..Sending::default() };
        let (status, response) =
            self.send_with(reactors_call(channel, message, emoji, limit, after), None, sending).await?;
        Self::answer(status, &response)
    }

    async fn reaction_request(&self, call: Call) -> Result<(), Refusal> {
        let sending = Sending { retry_rate_limit: false, ..Sending::default() };
        let result = match self.send_full(call, None, sending).await {
            Ok((status, response, retry_after)) => {
                Self::answer::<serde_json::Value>(status, &response).map(|_| ()).map_err(|e| (e, retry_after))
            }
            Err(e) => Err((e, None)),
        };
        result.map_err(|(error, retry_after)| Refusal { error, retry_after })
    }

    /// Reopen an archived thread, as the official client does before an
    /// edit or delete there.
    pub async fn unarchive_thread(&self, thread: Snowflake) -> Result<(), Error> {
        let call = call!(EDIT_CHANNEL, channel = thread);
        let _: serde_json::Value = self.request(call, Some(json!({ "archived": false }))).await?;
        Ok(())
    }

    /// Join `thread` (`joinThread` in 7909): no body, and the `location` the
    /// client names where the join came from. Discord answers with who is in
    /// it now, which comes as events too.
    pub async fn join_thread(&self, thread: Snowflake, location: &str) -> Result<(), Error> {
        let _: Value = self.request(join_thread_call(thread, location), None).await?;
        Ok(())
    }

    /// Leave `thread` (`leaveThread` in 7909), the same way.
    pub async fn leave_thread(&self, thread: Snowflake, location: &str) -> Result<(), Error> {
        let _: Value = self.request(leave_thread_call(thread, location), None).await?;
        Ok(())
    }

    /// The page of a channel's archived threads that `search` names
    /// (`loadArchivedThreads` in 7909). Unlike the client, a failure isn't
    /// tried again (its `retries: 2`): fewer requests, never more.
    pub async fn archived_threads(&self, search: &Search) -> Result<ArchivedPage, Error> {
        let answer: Value = self.request(archived_threads_call(search), None).await?;
        ArchivedPage::from_value(&answer).ok_or_else(|| Error::Protocol("archived threads without threads".into()))
    }

    /// A thread we don't know, by id (`loadThread` in 353202), with the
    /// server it is in. The client asks only for what it has no record of.
    pub async fn thread(&self, id: Snowflake) -> Result<(Snowflake, Channel), Error> {
        let answer: Value = self.request(call!(GET_CHANNEL, channel = id), None).await?;
        let thread = Channel::deserialize(&answer)?;
        match answer["guild_id"].as_str().and_then(|g| g.parse().ok()) {
            Some(guild) if thread.kind.is_thread() => Ok((Snowflake(guild), thread)),
            _ => Err(Error::Protocol("not a thread in a server".into())),
        }
    }

    /// Pin a forum post, or unpin it: its flags as they should be
    /// (`updateFlags` in 7909). Pinning an archived post reopens it in the
    /// same request.
    pub async fn set_thread_flags(&self, thread: Snowflake, flags: u64, reopen: bool) -> Result<(), Error> {
        let call = call!(EDIT_CHANNEL, channel = thread);
        let body = if reopen { json!({ "flags": flags, "archived": false }) } else { json!({ "flags": flags }) };
        let _: serde_json::Value = self.request(call, Some(body)).await?;
        Ok(())
    }

    /// Set the tags a forum post carries (`updateForumPostTags` in 919577),
    /// which the client does after reopening the post if it was archived.
    pub async fn set_post_tags(&self, thread: Snowflake, tags: &[Snowflake]) -> Result<(), Error> {
        let call = call!(EDIT_CHANNEL, channel = thread);
        let _: serde_json::Value = self.request(call, Some(json!({ "applied_tags": tags }))).await?;
        Ok(())
    }

    /// The first messages of up to ten of a forum's posts, as `post-data`
    /// answers (473503).
    pub async fn post_data(&self, forum: Snowflake, posts: &[Snowflake]) -> Result<Value, Error> {
        self.request(call!(POST_DATA, channel = forum), Some(json!({ "thread_ids": posts }))).await
    }

    /// Cast our vote on a poll: `answers`, or none to take it back. It
    /// replaces any vote we had, and the ids go out as strings
    /// (`handlePollSubmitVote` in 180662, which sends `answer_ids` through
    /// module 335759's `PUT`).
    pub async fn vote(&self, channel: Snowflake, message: Snowflake, answers: &[u32]) -> Result<(), Error> {
        let _: serde_json::Value = self.request(call!(VOTE, channel, message), Some(vote_body(answers))).await?;
        Ok(())
    }

    /// End our own poll now (`endPollEarly`, and 335759's `POST`): no body.
    pub async fn end_poll(&self, channel: Snowflake, message: Snowflake) -> Result<(), Error> {
        let _: serde_json::Value = self.request(call!(END_POLL, channel, message), None).await?;
        Ok(())
    }

    /// Press a button or pick from a menu: `POST /interactions` with the
    /// press as `nonce`, which comes back in the events that answer it. The
    /// client gives it three seconds and doesn't send it again after a 429
    /// (964363), and nor do we. A success only means Discord took it.
    pub async fn interact(&self, press: &Press<'_>, nonce: Snowflake) -> Result<(), Failure> {
        let sending = Sending { retry_rate_limit: false, ..Sending::default() };
        let sent = self.send_with(call!(INTERACT), Some(press.body(nonce)), sending);
        match glib::future_with_timeout(INTERACTION_TIMEOUT, sent).await {
            Ok(Ok((status, _))) if (200..300).contains(&status) => Ok(()),
            Ok(Ok((status, response))) => Err(Failure::from_response(status, &response)),
            // No answer in time, or none at all: nothing to say why.
            _ => Err(Failure::default()),
        }
    }

    /// Start a thread in `channel`, from `message` if given, with what
    /// [`crate::thread_creation::body`] makes. The thread arrives on the
    /// gateway as well.
    pub async fn create_thread(
        &self,
        channel: Snowflake,
        message: Option<Snowflake>,
        body: serde_json::Value,
    ) -> Result<Channel, Error> {
        let call = match message {
            Some(message) => call!(CREATE_THREAD_FROM_MESSAGE, channel, message),
            None => call!(CREATE_THREAD, channel),
        };
        self.request(call, Some(body)).await
    }

    /// Start a post in the forum or media channel `forum`, with what
    /// [`crate::post_creation::body`] makes: the same request as a thread's,
    /// with its query. The post arrives on the gateway as well.
    pub async fn create_post(&self, forum: Snowflake, body: serde_json::Value) -> Result<Channel, Error> {
        self.request(create_post_call(forum), Some(body)).await
    }

    /// Mark everything in `channel` up to `message` as read, for all our clients.
    pub async fn ack(&self, channel: Snowflake, message: Snowflake) -> Result<(), Error> {
        let body = json!({ "token": null });
        let call = call!(ACK, channel, message);
        let _: serde_json::Value = self.request(call, Some(body)).await?;
        Ok(())
    }

    /// Tell Discord we are typing in `channel`, which shows for ten seconds
    /// (`TYPING_START_LOCAL` in 741961). Not retried after a 429, as the
    /// client doesn't. Returns how many milliseconds Discord says we must
    /// wait before posting in the channel: slowmode we may not have known of
    /// (`message_send_cooldown_ms`, only in a 200), or 0.
    pub async fn typing(&self, channel: Snowflake) -> Result<i64, Error> {
        let (status, response) = self
            .send_with(call!(TYPING, channel), None, Sending { retry_rate_limit: false, ..Sending::default() })
            .await?;
        let answer: Value = Self::answer(status, &response)?;
        let cooldown = if status == 200 { answer["message_send_cooldown_ms"].as_f64() } else { None };
        Ok(cooldown.map_or(0, |ms| ms as i64))
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

    /// A request of the upload flow: not retried on a rate limit, which the
    /// client's HTTP layer never does, and saying how many requests failed
    /// before it in `X-Failed-Requests`, as it does for a retry (620632).
    pub(crate) async fn send_upload_request(
        &self,
        call: Call,
        body: Option<Value>,
        failed: u32,
        original_md5: Option<&str>,
    ) -> Result<(u32, glib::Bytes), Error> {
        self.send_with(call, body, Sending { retry_rate_limit: false, failed, original_md5 }).await
    }

    /// `PUT` to an upload URL, which is Google's and not Discord's: it takes
    /// no token and none of our headers. `body` is a stream and how long it
    /// is, `sent` is told how many bytes have gone, and `content_range` is
    /// for a resumed upload. Gives the status and the `Range` Google answers
    /// with.
    pub(crate) async fn put(
        &self,
        url: &str,
        body: Option<(gio::InputStream, isize)>,
        content_range: Option<&str>,
        sent: Rc<dyn Fn(u64)>,
    ) -> Result<(u32, Option<String>), Error> {
        let msg = soup::Message::new("PUT", url).map_err(|_| Error::Protocol("the upload URL isn't one".into()))?;
        // A 308 here means "resume", and has no Location to follow.
        msg.add_flags(soup::MessageFlags::NO_REDIRECT);
        if let Some(range) = content_range {
            msg.request_headers().expect("request has headers").replace("Content-Range", range);
        }
        if let Some((stream, length)) = &body {
            msg.set_request_body(Some("application/octet-stream"), Some(stream), *length);
        }
        let total = Cell::new(0u64);
        msg.connect_wrote_body_data(move |_, chunk| {
            total.set(total.get() + u64::from(chunk));
            sent(total.get());
        });
        self.session.send_and_read_future(&msg, glib::Priority::DEFAULT).await.map_err(Error::Transport)?;
        let range = msg.response_headers().and_then(|h| h.one("Range")).map(|r| r.to_string());
        Ok((msg.status_code(), range))
    }

    /// Take back an upload (`delete` in 48325): the client sends it and
    /// ignores what comes of it.
    pub(crate) async fn delete_upload(&self, uploaded_filename: &str) {
        let _ = self.send_upload_request(call!(DELETE_UPLOAD, uploaded_filename), None, 0, None).await;
    }

    pub(crate) async fn request<T: DeserializeOwned>(
        &self,
        call: Call,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        let (status, response) = self.send(call, body).await?;
        Self::answer(status, &response)
    }

    /// What a response makes of a request: its body, or the error.
    fn answer<T: DeserializeOwned>(status: u32, response: &[u8]) -> Result<T, Error> {
        match status {
            // 204 No Content reads as `null`, which `()` accepts.
            200..=299 if response.is_empty() => Ok(serde_json::from_value(serde_json::Value::Null)?),
            200..=299 => Ok(serde_json::from_slice(response)?),
            401 => Err(Error::Unauthorized),
            400 if serde_json::from_slice(response).is_ok_and(|v| is_captcha(&v)) => Err(Error::Captcha),
            _ => Err(Error::Http { status, body: String::from_utf8_lossy(response).chars().take(300).collect() }),
        }
    }

    /// The status and body of a request, retrying once on a rate limit and
    /// after each captcha solved. Fails without sending while too many
    /// requests have been refused, rather than wait: requests queued for
    /// minutes would all go at once when the pause ends.
    pub(crate) async fn send(&self, call: Call, body: Option<Value>) -> Result<(u32, glib::Bytes), Error> {
        self.send_with(call, body, Sending::default()).await
    }

    async fn send_with(
        &self,
        call: Call,
        body: Option<Value>,
        options: Sending<'_>,
    ) -> Result<(u32, glib::Bytes), Error> {
        self.send_full(call, body, options).await.map(|(status, response, _)| (status, response))
    }

    /// [`Self::send_with`], and the `Retry-After` header the response had.
    async fn send_full(
        &self,
        call: Call,
        body: Option<Value>,
        options: Sending<'_>,
    ) -> Result<(u32, glib::Bytes, Option<String>), Error> {
        let mut retries = 0;
        let mut captchas = 0;
        let mut answer: Option<(String, Captcha)> = None;
        loop {
            if let Some(wait) = INVALID.with_borrow_mut(|invalid| invalid.paused(Instant::now())) {
                self.report_pause(wait);
                return Err(Error::Paused(wait));
            }
            let msg = soup::Message::new(call.method(), &format!("{}{}", identity::API_BASE, call.path()))
                .expect("API URL is valid");
            let headers = msg.request_headers().expect("request has headers");
            for (name, value) in base_headers(self.token.as_deref()) {
                headers.replace(name, &value);
            }
            if options.failed > 0 {
                headers.replace("X-Failed-Requests", &options.failed.to_string());
            }
            if let Some(md5) = options.original_md5.filter(|md5| is_md5(md5)) {
                headers.replace("X-Discord-Original-MD5", md5);
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
            let scope = msg.response_headers().and_then(|h| h.one("X-RateLimit-Scope"));
            if InvalidRequests::counts(status, scope.as_deref()) {
                match INVALID.with_borrow_mut(|invalid| invalid.record(Instant::now())) {
                    Some(Change::Warn(count)) => eprintln!("rest: Discord refused {count} requests in 10 minutes"),
                    Some(Change::Pause(wait)) => {
                        eprintln!("rest: Discord refused too many requests; holding back for {}s", wait.as_secs());
                        self.report_pause(wait);
                    }
                    None => {}
                }
            }
            if status == 429
                && options.retry_rate_limit
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
            let retry_after = msg.response_headers().and_then(|h| h.one("Retry-After")).map(|v| v.to_string());
            return Ok((status, response, retry_after));
        }
    }

    fn report_pause(&self, wait: Duration) {
        if let Some(handler) = &self.pauses {
            handler(wait);
        }
    }
}

/// The request that adds our reaction: the emoji in the path, and as the
/// query `location` and `type`, 0 being a normal reaction.
fn add_reaction_call(channel: Snowflake, message: Snowflake, emoji: &Emoji, location: Location) -> Call {
    let emoji = emoji.path();
    let location = glib::uri_escape_string(location.name(), None::<&str>, false);
    call!(ADD_REACTION, channel, message, emoji).query(&format!("location={location}&type=0"))
}

/// The request that takes one back, `user`'s or ours: the type is in the path
/// too, and the query is `location` and `burst`.
fn remove_reaction_call(
    channel: Snowflake,
    message: Snowflake,
    emoji: &Emoji,
    location: Location,
    user: Option<Snowflake>,
) -> Call {
    let emoji = emoji.path();
    let user = user.map_or_else(|| "@me".to_owned(), |u| u.to_string());
    let location = glib::uri_escape_string(location.name(), None::<&str>, false);
    call!(REMOVE_REACTION, channel, message, emoji, user).query(&format!("location={location}&burst=false"))
}

/// The request that joins a thread: `location` is its only query field.
fn join_thread_call(thread: Snowflake, location: &str) -> Call {
    let location = glib::uri_escape_string(location, None::<&str>, false);
    call!(JOIN_THREAD, channel = thread).query(&format!("location={location}"))
}

fn leave_thread_call(thread: Snowflake, location: &str) -> Call {
    let location = glib::uri_escape_string(location, None::<&str>, false);
    call!(LEAVE_THREAD, channel = thread).query(&format!("location={location}"))
}

/// The request for a page of archived threads, its query in the order
/// `loadArchivedThreads` writes it ([`Search::query`]).
fn archived_threads_call(search: &Search) -> Call {
    call!(SEARCH_THREADS, channel = search.channel).query(&search.query())
}

/// The request that starts a post: a thread's endpoint, with the query the
/// client adds (`createForumPost` in 971389).
fn create_post_call(forum: Snowflake) -> Call {
    call!(CREATE_THREAD, channel = forum).query(post_creation::QUERY)
}

/// The request for who reacted: `limit`, `after` when paging, and `type`,
/// in the order `ao` writes them.
fn reactors_call(channel: Snowflake, message: Snowflake, emoji: &Emoji, limit: u32, after: Option<Snowflake>) -> Call {
    let emoji = emoji.path();
    let after = after.map(|a| format!("&after={a}")).unwrap_or_default();
    call!(REACTORS, channel, message, emoji).query(&format!("limit={limit}{after}&type=0"))
}

/// What a vote sends: the picked answers' ids, as strings in the order picked.
fn vote_body(answers: &[u32]) -> Value {
    json!({ "answer_ids": answers.iter().map(u32::to_string).collect::<Vec<_>>() })
}

/// What sending a message sends, as `_sendMessage` in 138399 builds it.
fn message_body(
    channel: Snowflake,
    content: &str,
    nonce: &str,
    flags: u64,
    reply: Option<Reply>,
    attachments: &[Sent<'_>],
    sticker: Option<Snowflake>,
) -> Value {
    let mut body = json!({
        "content": content,
        "nonce": nonce,
        "tts": false,
        "flags": flags,
    });
    if let Some(reply) = reply {
        add_reply(&mut body, channel, reply);
    }
    if !attachments.is_empty() {
        body["attachments"] = attachments.iter().enumerate().map(|(index, a)| a.body(index)).collect();
    }
    if let Some(sticker) = sticker {
        // A list, though the picker only ever picks one.
        body["sticker_ids"] = json!([sticker]);
    }
    body
}

/// The fields a reply adds, as the official client sends them.
fn add_reply(body: &mut Value, channel: Snowflake, reply: Reply) {
    body["message_reference"] = json!({ "message_id": reply.to, "channel_id": channel });
    if let Some(guild) = reply.guild {
        body["message_reference"]["guild_id"] = json!(guild);
    }
    if !reply.ping {
        // Mentions in the text still ping; only the reply itself doesn't.
        body["allowed_mentions"] = json!({ "parse": ["users", "roles", "everyone"], "replied_user": false });
    }
}

/// What scheduling a message sends, as `pr` in 151282 builds it from what
/// `_sendMessage` made of the message. Of its fields, only the message's own
/// go along (no nonce, `tts` or analytics), and `attachments` is always
/// there. A leading `@silent` becomes the flag, as for any message.
fn schedule_body(
    channel: Snowflake,
    at: &str,
    content: &str,
    reply: Option<Reply>,
    attachments: &[Sent<'_>],
    sticker: Option<Snowflake>,
    poll: Option<Value>,
) -> Value {
    let (content, flags) = scheduled::split_silent(content, 0);
    let mut body = json!({
        "channel_id": channel,
        "content": content,
        "scheduled_timestamp": at,
        "flags": flags,
        "attachments": attachments.iter().enumerate().map(|(index, a)| a.body(index)).collect::<Vec<_>>(),
    });
    if let Some(reply) = reply {
        add_reply(&mut body, channel, reply);
    }
    if let Some(sticker) = sticker {
        body["sticker_ids"] = json!([sticker]);
    }
    if let Some(poll) = poll {
        body["poll"] = poll;
    }
    body
}

/// What editing a scheduled message's text sends: the box's text, with the
/// flags it already had, `@silent` set or unset by what was written
/// (`Eg` in 151282).
fn edit_scheduled_body(content: &str, flags: u64) -> Value {
    let (content, flags) = scheduled::split_silent(content, flags & !scheduled::SUPPRESS_NOTIFICATIONS);
    json!({ "content": content, "flags": flags })
}

/// What posting a poll sends: an empty message with the `poll`.
fn poll_body(channel: Snowflake, nonce: &str, poll: Value) -> Value {
    let mut body = message_body(channel, "", nonce, 0, None, &[], None);
    body["poll"] = poll;
    body
}

/// What editing `message` to say `content` sends, as `editMessage` in 138399
/// builds it.
fn edit_body(message: &Message, content: &str) -> Value {
    let mut body = json!({ "content": content });
    if message.is_quiet_reply() {
        // Mentions are worked out again from the new text, which would
        // otherwise ping the author of what it answers.
        body["allowed_mentions"] = json!({ "parse": ["users", "roles", "everyone"], "replied_user": false });
    }
    body
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
/// What `buildHeadersForMd5` (724165) lets through as the original's hash:
/// 32 lowercase hex digits, or no header at all.
fn is_md5(hash: &str) -> bool {
    hash.len() == 32 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn base_headers(token: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = vec![("X-Discord-Locale", identity::LOCALE.to_owned())];
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
    use serde_json::json;

    use super::{
        Captcha, Page, Reply, ReplyPing, add_reaction_call, archived_threads_call, base_headers, create_post_call,
        edit_body, edit_scheduled_body, is_md5, join_thread_call, leave_thread_call, message_body, poll_body,
        rate_limit_wait, reactors_call, remove_reaction_call, schedule_body, vote_body,
    };
    use crate::endpoints::call;
    use crate::model::Emoji;
    use crate::reactions::Location;
    use crate::uploads::Sent;
    use crate::uploads::Voice;
    use crate::{Message, Snowflake, identity};

    #[test]
    fn reactions_are_asked_for_as_the_client_does() {
        // `C` in 649963: `name:id` for a custom emoji, the emoji for a Unicode one,
        // with the query as `BB` and `et` write it.
        let (channel, message) = (Snowflake(1), Snowflake(2));
        let blob = Emoji { id: Some(Snowflake(12)), name: Some("blob".into()), animated: false };
        let fire = Emoji { id: None, name: Some("🔥".into()), animated: false };
        let paths = |call: crate::endpoints::Call| (call.method(), call.path().to_owned());
        assert_eq!(
            paths(add_reaction_call(channel, message, &blob, Location::InlineButton)),
            ("PUT", "/channels/1/messages/2/reactions/blob:12/@me?location=Message%20Inline%20Button&type=0".into())
        );
        assert_eq!(
            paths(add_reaction_call(channel, message, &fire, Location::Message)),
            ("PUT", "/channels/1/messages/2/reactions/%F0%9F%94%A5/@me?location=Message&type=0".into())
        );
        assert_eq!(
            paths(remove_reaction_call(channel, message, &fire, Location::InlineButton, None)),
            (
                "DELETE",
                "/channels/1/messages/2/reactions/%F0%9F%94%A5/0/@me?location=Message%20Inline%20Button&burst=false"
                    .into()
            )
        );
        // Someone else's, from the list of who reacted.
        assert_eq!(
            paths(remove_reaction_call(channel, message, &blob, Location::Message, Some(Snowflake(7)))),
            ("DELETE", "/channels/1/messages/2/reactions/blob:12/0/7?location=Message&burst=false".into())
        );
        assert_eq!(
            paths(reactors_call(channel, message, &blob, 100, None)),
            ("GET", "/channels/1/messages/2/reactions/blob:12?limit=100&type=0".into())
        );
        assert_eq!(
            paths(reactors_call(channel, message, &fire, 3, Some(Snowflake(9)))).1,
            "/channels/1/messages/2/reactions/%F0%9F%94%A5?limit=3&after=9&type=0"
        );
        assert_eq!(
            paths(add_reaction_call(channel, message, &blob, Location::ReactionPicker)).1,
            "/channels/1/messages/2/reactions/blob:12/@me?location=Message%20Reaction%20Picker&type=0"
        );
    }

    #[test]
    fn shift_turns_the_ping_off_even_where_the_toggle_is_hidden() {
        // `$b` in 519222: shouldMention: !shift && !own, showMentionToggle: !private && !own.
        let ping = |own, private, shift| ReplyPing::for_click(own, private, shift);
        assert_eq!(ping(false, false, false), ReplyPing { ping: true, toggle: true });
        assert_eq!(ping(false, false, true), ReplyPing { ping: false, toggle: true });
        assert_eq!(ping(false, true, false), ReplyPing { ping: true, toggle: false });
        assert_eq!(ping(false, true, true), ReplyPing { ping: false, toggle: false });
        for (private, shift) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(ping(true, private, shift), ReplyPing { ping: false, toggle: false });
        }
        assert_eq!(ping(false, true, true).fixed(), Some(false));
        assert_eq!(ping(false, false, true).fixed(), None);
    }

    #[test]
    fn only_a_lowercase_md5_is_sent_as_the_originals_hash() {
        assert!(is_md5("d41d8cd98f00b204e9800998ecf8427e"));
        assert!(!is_md5("D41D8CD98F00B204E9800998ECF8427E"), "uppercase fails the client's regex");
        assert!(!is_md5("d41d8cd98f00b204e9800998ecf8427") && !is_md5("d41d8cd98f00b204e9800998ecf8427ef"));
        assert!(!is_md5("g41d8cd98f00b204e9800998ecf8427e") && !is_md5(""));
    }

    #[test]
    fn a_vote_sends_string_ids_and_nothing_else() {
        // `answer_ids` are the answers' ids as the client keeps them: strings.
        assert_eq!(vote_body(&[3, 1]).to_string(), r#"{"answer_ids":["3","1"]}"#);
        assert_eq!(vote_body(&[]).to_string(), r#"{"answer_ids":[]}"#, "taking a vote back");
    }

    #[test]
    fn history_queries_read_like_the_clients() {
        // `fetchMessages` in 138399 passes { before, after, limit, around }.
        let id = Snowflake(7);
        assert_eq!(Page::Newest.query(12), "limit=12");
        assert_eq!(Page::Before(id).query(24), "before=7&limit=24");
        assert_eq!(Page::After(id).query(24), "after=7&limit=24");
        assert_eq!(Page::Around(id).query(30), "limit=30&around=7");
    }

    #[test]
    fn thread_requests_read_like_the_clients() {
        let thread = Snowflake(7);
        let (join, leave) = (join_thread_call(thread, "Context Menu"), leave_thread_call(thread, "Context Menu"));
        assert_eq!((join.method(), join.path()), ("POST", "/channels/7/thread-members/@me?location=Context%20Menu"));
        assert_eq!(
            (leave.method(), leave.path()),
            ("DELETE", "/channels/7/thread-members/@me?location=Context%20Menu")
        );
        // `loadArchivedThreads` in 7909: no `tag`, since none is chosen.
        let page = archived_threads_call(&crate::forum::Search::plain(thread, 25));
        assert_eq!(page.method(), "GET");
        assert_eq!(
            page.path(),
            "/channels/7/threads/search?archived=true&sort_by=last_message_time&sort_order=desc&limit=25\
             &tag_setting=match_some&offset=25"
        );
        assert_eq!(call!(GET_CHANNEL, channel = thread).path(), "/channels/7");
        // A post is the thread request with the query `createForumPost` adds; a plain thread has none.
        let post = create_post_call(thread);
        assert_eq!((post.method(), post.path()), ("POST", "/channels/7/threads?use_nested_fields=true"));
        assert_eq!(call!(CREATE_THREAD, channel = thread).path(), "/channels/7/threads");
    }

    #[test]
    fn a_message_names_its_uploads() {
        let sent = |name: &'static str| Sent {
            filename: name,
            uploaded_filename: "u/f",
            description: None,
            spoiler: false,
            content_type: "image/png",
            voice: None,
        };
        let body = message_body(Snowflake(9), "", "1", 0, None, &[sent("a.png"), sent("b.png")], None);
        assert_eq!(body["content"], "", "files alone are a message");
        assert_eq!(
            (body["attachments"][0]["id"].as_str(), body["attachments"][1]["id"].as_str()),
            (Some("0"), Some("1"))
        );
        assert_eq!(body["attachments"][1]["filename"], "b.png");
        assert!(message_body(Snowflake(9), "hi", "1", 0, None, &[], None).get("attachments").is_none());
        // A reply to a message goes with the files.
        let reply = Reply { to: Snowflake(3), guild: None, ping: true };
        let body = message_body(Snowflake(9), "hi", "1", 0, Some(reply), &[sent("a.png")], None);
        assert_eq!(body["message_reference"], json!({ "message_id": "3", "channel_id": "9" }));
        assert_eq!(body["attachments"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_sticker_goes_by_id_alone_or_with_text_a_reply_and_files() {
        // `_sendMessage` (138399) adds `sticker_ids` to the same body, whatever else is in it.
        let body = message_body(Snowflake(9), "", "1", 0, None, &[], Some(Snowflake(5)));
        assert_eq!((body["content"].as_str(), body["sticker_ids"].clone()), (Some(""), json!(["5"])));
        assert!(message_body(Snowflake(9), "", "1", 0, None, &[], None).get("sticker_ids").is_none());
        let reply = Reply { to: Snowflake(3), guild: Some(Snowflake(2)), ping: false };
        let sent = Sent {
            filename: "a.png",
            uploaded_filename: "u/f",
            description: None,
            spoiler: false,
            content_type: "image/png",
            voice: None,
        };
        let body = message_body(Snowflake(9), "hi", "1", 0, Some(reply), &[sent], Some(Snowflake(5)));
        assert_eq!((body["content"].as_str(), body["sticker_ids"].clone()), (Some("hi"), json!(["5"])));
        assert_eq!(body["message_reference"]["message_id"], "3");
        assert_eq!(body["attachments"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_voice_message_is_flagged_and_has_no_text() {
        let recording = Sent {
            filename: "0recording.ogg",
            uploaded_filename: "u/0recording.ogg",
            description: None,
            spoiler: false,
            content_type: "audio/ogg",
            voice: Some(Voice { duration_secs: 3.0, waveform: "AAEC" }),
        };
        let body = message_body(Snowflake(9), "", "1", Message::IS_VOICE_MESSAGE, None, &[recording], None);
        assert_eq!(body["flags"], 8192);
        assert_eq!(body["content"], "");
        assert_eq!(body["attachments"][0]["duration_secs"], 3.0);
        assert_eq!(body["attachments"][0]["waveform"], "AAEC");
        assert_eq!(message_body(Snowflake(9), "hi", "1", 0, None, &[], None)["flags"], 0);
    }

    #[test]
    fn a_poll_is_a_message_of_nothing_else() {
        let poll = json!({ "question": { "text": "Q?" }, "answers": [], "duration": 24 });
        assert_eq!(
            poll_body(Snowflake(9), "1", poll.clone()),
            json!({ "content": "", "nonce": "1", "tts": false, "flags": 0, "poll": poll }),
        );
    }

    #[test]
    fn scheduling_sends_the_messages_own_fields_and_the_time() {
        let at = "2027-01-15T09:00:00.000Z";
        // Nothing but what the box held: no nonce or tts, and `attachments` even when empty.
        assert_eq!(
            schedule_body(Snowflake(9), at, "hi", None, &[], None, None),
            json!({ "channel_id": "9", "content": "hi", "scheduled_timestamp": at, "flags": 0, "attachments": [] }),
        );
        let reply = Reply { to: Snowflake(3), guild: Some(Snowflake(2)), ping: false };
        let sent = Sent {
            filename: "a.png",
            uploaded_filename: "u/f",
            description: Some("alt"),
            spoiler: false,
            content_type: "image/png",
            voice: None,
        };
        let body = schedule_body(Snowflake(9), at, "hi", Some(reply), &[sent], Some(Snowflake(5)), None);
        assert_eq!(body["message_reference"], json!({ "message_id": "3", "channel_id": "9", "guild_id": "2" }));
        assert_eq!(body["allowed_mentions"]["replied_user"], false);
        assert_eq!(body["sticker_ids"], json!(["5"]));
        assert_eq!(body["attachments"][0]["uploaded_filename"], "u/f");
        assert_eq!(body["attachments"][0]["description"], "alt");
        let poll = json!({ "question": { "text": "Q?" } });
        let body = schedule_body(Snowflake(9), at, "", None, &[], None, Some(poll.clone()));
        assert_eq!((body["poll"].clone(), body["content"].as_str()), (poll, Some("")));
    }

    #[test]
    fn a_quiet_message_is_scheduled_with_the_flag() {
        let at = "2027-01-15T09:00:00.000Z";
        let body = schedule_body(Snowflake(9), at, "@silent hi", None, &[], None, None);
        assert_eq!((body["content"].as_str(), body["flags"].as_u64()), (Some("hi"), Some(4096)));
    }

    #[test]
    fn editing_a_scheduled_message_sets_the_flag_from_the_text() {
        assert_eq!(edit_scheduled_body("hi", 0), json!({ "content": "hi", "flags": 0 }));
        assert_eq!(edit_scheduled_body("@silent hi", 0), json!({ "content": "hi", "flags": 4096 }));
        // Written out of the text, it's off; other flags stay.
        assert_eq!(edit_scheduled_body("hi", 4096 | 64), json!({ "content": "hi", "flags": 64 }));
        assert_eq!(edit_scheduled_body("@silent hi", 4096 | 64), json!({ "content": "hi", "flags": 4096 | 64 }));
    }

    #[test]
    fn edits_keep_a_quiet_reply_quiet() {
        let message = |mentions: &str| -> Message {
            let original = r#"{"id":"1","channel_id":"9","author":{"id":"2","username":"a"},"timestamp":""}"#;
            serde_json::from_str(&format!(
                r#"{{"id":"3","channel_id":"9","type":19,"author":{{"id":"5","username":"me"}},"timestamp":"",
                "message_reference":{{"message_id":"1"}},"referenced_message":{original},"mentions":{mentions}}}"#
            ))
            .unwrap()
        };
        assert_eq!(edit_body(&message(r#"[{"id":"2","username":"a"}]"#), "hi"), json!({ "content": "hi" }));
        let quiet = json!({
            "content": "hi",
            "allowed_mentions": { "parse": ["users", "roles", "everyone"], "replied_user": false },
        });
        assert_eq!(edit_body(&message("[]"), "hi"), quiet);
    }

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
