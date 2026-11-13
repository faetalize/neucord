//! Starting a thread, from the attach menu's Create Thread (a new thread in
//! the channel) or from a message's: who the client offers it to, what it
//! asks Discord for, what the thread is called when nothing is typed, and
//! how a refusal reads. Which threads the sidebar lists afterwards is in
//! [`crate::threads`].

use serde_json::{Value, json};

use crate::Error;
use crate::model::{Channel, ChannelKind, Guild, Message};
use crate::permissions::{Permissions, in_channel};
use crate::verification::Account;

/// `location` of a request from the attach menu, and from the message menu
/// (what `Tv` in 747926 is given).
pub const FROM_ATTACH_MENU: &str = "Plus Button";
pub const FROM_MESSAGE_MENU: &str = "Message";
/// `location` of /thread's request (12115).
pub const FROM_SLASH_COMMAND: &str = "Slash Command";

/// The longest name the client's name box takes, in UTF-16 units (`rm` in
/// 289287).
pub const NAME_MAX: usize = 100;

/// What the name box shows where nothing is typed (`Nb2/RE`), and what a
/// thread from a message is called when the message has nothing to name it
/// by (`7Xm5QI`).
pub const NAME_PLACEHOLDER: &str = "New Thread";
const NAME_FALLBACK: &str = "Thread";

/// Minutes before a thread archives itself, where the channel says nothing
/// (`Gl` in 268761).
pub(crate) const AUTO_ARCHIVE_FALLBACK: u32 = 4320;

const ANNOUNCEMENT_THREAD: u8 = 10;
const PUBLIC_THREAD: u8 = 11;
const PRIVATE_THREAD: u8 = 12;

/// Whether the parent's kind takes threads (`wE` in 95701), before
/// permissions. The client lists app channels (21) too, which Neucord has no
/// kind for.
fn takes_threads(kind: ChannelKind) -> bool {
    matches!(kind, ChannelKind::Text | ChannelKind::Announcement | ChannelKind::Forum | ChannelKind::Media)
}

/// `C` in 406704: a kind that takes threads, `allowed` by permissions, and
/// for a message one that is somebody's words with no thread yet.
fn offered(channel: &Channel, message: Option<&Message>, allowed: bool) -> bool {
    allowed && takes_threads(channel.kind) && message.is_none_or(|m| !m.has_thread() && m.is_user_message())
}

/// Whether the client offers a thread in `channel`, public or announcement,
/// or from `message` (`n` in 406704, and `D1` for the keyboard): Create
/// Public Threads with Read Message History, or Send Messages in a forum
/// or media channel.
pub fn can_start(guild: &Guild, channel: &Channel, message: Option<&Message>, me: &Account, now: i64) -> bool {
    let needs = match channel.kind.is_forum_like() {
        true => Permissions::SEND_MESSAGES,
        false => Permissions::CREATE_PUBLIC_THREADS | Permissions::READ_MESSAGE_HISTORY,
    };
    offered(channel, message, in_channel(guild, channel, me, now).contains(needs))
}

/// Whether the client offers a private thread in `channel` (`Tb`): Create
/// Private Threads in a text channel.
pub fn can_start_private(guild: &Guild, channel: &Channel, me: &Account, now: i64) -> bool {
    let allowed = in_channel(guild, channel, me, now).contains(Permissions::CREATE_PRIVATE_THREADS);
    offered(channel, None, allowed) && channel.kind == ChannelKind::Text
}

/// Whether the attach menu offers Create Thread in `channel`.
pub fn offered_in_attach_menu(guild: &Guild, channel: &Channel, me: &Account, now: i64) -> bool {
    can_start(guild, channel, None, me, now) || can_start_private(guild, channel, me, now)
}

/// Whether a new thread in `channel` may be private (`EN` in 971389): the
/// switch is on and fixed where private threads are all we may start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Privacy {
    /// No switch: public.
    Public,
    Choice,
    PrivateOnly,
}

impl Privacy {
    pub fn of(guild: &Guild, channel: &Channel, me: &Account, now: i64) -> Self {
        match (can_start_private(guild, channel, me, now), can_start(guild, channel, None, me, now)) {
            (false, _) => Self::Public,
            (true, true) => Self::Choice,
            (true, false) => Self::PrivateOnly,
        }
    }

    /// Whether the thread is private, given the switch (`Iy`).
    pub fn is_private(self, switch: bool) -> bool {
        self == Self::PrivateOnly || (self == Self::Choice && switch)
    }
}

/// What the request creates: private if asked, else an announcement thread
/// under an announcement channel, else a public one.
fn thread_type(parent: &Channel, private: bool) -> u8 {
    match (private, parent.kind) {
        (true, _) => PRIVATE_THREAD,
        (false, ChannelKind::Announcement) => ANNOUNCEMENT_THREAD,
        (false, _) => PUBLIC_THREAD,
    }
}

/// What starting a thread in `parent` sends (`r$` in 971389). From a message
/// the thread is public or announcement whatever `private` says, as the
/// switch isn't offered there.
pub fn body(parent: &Channel, name: &str, private: bool, location: &str) -> Value {
    json!({
        "name": name,
        "type": thread_type(parent, private),
        "auto_archive_duration": parent.default_auto_archive_duration.unwrap_or(AUTO_ARCHIVE_FALLBACK),
        "location": location,
    })
}

/// What /thread sends (`Nw` in 971389, from 12115): always a public thread
/// (`PUBLIC_THREAD`, even under an announcement channel, where the form
/// asks for an announcement thread), in `channel`, the one the box is in.
pub fn slash_body(channel: &Channel, name: &str) -> Value {
    json!({
        "name": name,
        "type": PUBLIC_THREAD,
        "auto_archive_duration": channel.default_auto_archive_duration.unwrap_or(AUTO_ARCHIVE_FALLBACK),
        "location": FROM_SLASH_COMMAND,
    })
}

/// Why the client turns the form back before sending anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    Name,
    Message,
}

/// What the form lacks (`submit` in 289287): a name where the thread isn't
/// from a message, and something to say, text, a sticker or a file. A name
/// of spaces counts as empty here, though it is sent as it is.
pub fn missing(from_message: bool, name: &str, text: &str, has_more: bool) -> Vec<Missing> {
    let name = (!from_message && name.trim().is_empty()).then_some(Missing::Name);
    let message = (text.is_empty() && !has_more).then_some(Missing::Message);
    name.into_iter().chain(message).collect()
}

/// A name box's text as the client keeps it (`A` in 474078): line breaks
/// become spaces, and on leaving the box it is trimmed.
pub fn single_line(text: &str, trim: bool) -> String {
    let text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
    if trim { text.trim().to_owned() } else { text }
}

/// `text` cut to `max` UTF-16 units, at a character.
pub fn cut(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &text[..at];
        }
    }
    text
}

/// What a thread from a message is called when no name is typed (`V` in
/// 971389): the first embed's title (40 units), else a poll's question (80),
/// else the first line of the message, as plain text, cut after whole words
/// within 40; a name that doesn't fit is cut and ends in `...`.
pub fn default_name(embed_title: Option<&str>, poll_question: Option<&str>, text: &str) -> String {
    let truncated = |s: &str, max: usize| match cut(s, max).len() < s.len() {
        true => format!("{}...", cut(s, max)),
        false => s.to_owned(),
    };
    if let Some(title) = embed_title.filter(|t| !t.is_empty()) {
        return truncated(title, 40);
    }
    if let Some(question) = poll_question.filter(|q| !q.is_empty()) {
        return truncated(question, 80);
    }
    let line = single_line(text.split('\n').next().unwrap_or_default(), true);
    let line = line.trim_start_matches([' ', '#', '-']);
    // Words and the runs between them, which the name takes while it fits.
    let separator = |c: char| c.is_whitespace() || "!@#$%^&*()_-+={}[]:\";'<>?,./".contains(c);
    let mut name = String::new();
    let mut rest = line;
    let mut first = true;
    let mut pieces = Vec::new();
    loop {
        let Some(at) = rest.find(separator) else {
            pieces.push(rest);
            break;
        };
        let run = rest[at..].find(|c| !separator(c)).map_or(rest.len(), |len| at + len);
        pieces.push(&rest[..at]);
        pieces.push(&rest[at..run]);
        rest = &rest[run..];
    }
    for piece in pieces {
        if !first && crate::limits::message_length(&name) + piece.encode_utf16().count() > 40 {
            break;
        }
        name.push_str(piece);
        first = false;
    }
    match truncated(&name, 40) {
        name if name.is_empty() => NAME_FALLBACK.to_owned(),
        name => name,
    }
}

/// Discord's answers to starting a thread that the client handles, and how:
/// the first four with an alert, the rest next to the form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    TooManyThreads,
    TooManyAnnouncementThreads,
    /// Slowmode, which the client records and says nothing of; the seconds
    /// to wait.
    Slowmode(u64),
    /// Any other rate limit.
    RateLimited,
    /// AutoMod blocked the name (`title`) or the message; its reason, if it
    /// gave one.
    AutoMod {
        title: bool,
        reason: Option<String>,
    },
    /// The name was refused.
    Name,
    Other,
}

const INVALID_FORM_BODY: u64 = 50035;
const TOO_MANY_THREADS: u64 = 160006;
const TOO_MANY_ANNOUNCEMENT_THREADS: u64 = 160007;
const AUTOMOD_MESSAGE_BLOCKED: u64 = 200000;
const AUTOMOD_TITLE_BLOCKED: u64 = 200001;

impl Refused {
    /// What `Y` in 971389 makes of a failed request.
    pub fn from(error: &Error) -> Self {
        let Error::Http { status, body } = error else { return Self::Other };
        let body: Value = serde_json::from_str(body).unwrap_or_default();
        match body["code"].as_u64() {
            Some(TOO_MANY_THREADS) => Self::TooManyThreads,
            Some(TOO_MANY_ANNOUNCEMENT_THREADS) => Self::TooManyAnnouncementThreads,
            Some(crate::rest::SLOWMODE_RATE_LIMITED) if *status == 429 => {
                Self::Slowmode(body["retry_after"].as_f64().unwrap_or(0.0).ceil() as u64)
            }
            _ if *status == 429 => Self::RateLimited,
            Some(code @ (AUTOMOD_MESSAGE_BLOCKED | AUTOMOD_TITLE_BLOCKED)) => Self::AutoMod {
                title: code == AUTOMOD_TITLE_BLOCKED,
                reason: body["message"].as_str().map(str::to_owned),
            },
            Some(INVALID_FORM_BODY) if !body["errors"]["name"].is_null() => Self::Name,
            _ => Self::Other,
        }
    }

    /// Whether /thread's refusal shows an alert: `Y` in 971389 alone
    /// handles it there, which says nothing of slowmode (it only records
    /// the cooldown) and rethrows AutoMod's and a refused name's unseen.
    /// Either way the command stays in the box.
    pub fn alerts_after_slash_command(&self) -> bool {
        matches!(self, Self::TooManyThreads | Self::TooManyAnnouncementThreads | Self::RateLimited | Self::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Snowflake;

    const GUILD: u64 = 1;
    const ME: Snowflake = Snowflake(5);

    fn me() -> Account {
        Account { id: ME, verified: true, email: true, phone: false, mfa: false, nsfw_allowed: false }
    }

    /// A server where @everyone has `granted`, and a channel of `kind`.
    fn place(granted: Permissions, kind: u8) -> (Guild, Channel) {
        let everyone = (Permissions::VIEW_CHANNEL | granted).0.to_string();
        let guild = serde_json::from_value(json!({
            "id": GUILD.to_string(), "name": "G", "owner_id": "2",
            "roles": [{"id": GUILD.to_string(), "name": "@everyone", "permissions": everyone}],
        }))
        .unwrap();
        (guild, serde_json::from_value(json!({"id": "9", "type": kind})).unwrap())
    }

    fn message(kind: u8, flags: u64) -> Message {
        serde_json::from_value(
            json!({"id": "30", "channel_id": "9", "author": {"id": "6", "username": "a"}, "timestamp": "t",
                   "type": kind, "flags": flags}),
        )
        .unwrap()
    }

    const PUBLIC: Permissions = Permissions(Permissions::CREATE_PUBLIC_THREADS.0 | Permissions::READ_MESSAGE_HISTORY.0);

    #[test]
    fn public_threads_need_creating_and_history() {
        let start = |granted, kind| {
            let (guild, channel) = place(granted, kind);
            can_start(&guild, &channel, None, &me(), 0)
        };
        assert!(start(PUBLIC, 0) && start(PUBLIC, 5));
        assert!(!start(Permissions::CREATE_PUBLIC_THREADS, 0), "history too");
        assert!(!start(Permissions::READ_MESSAGE_HISTORY, 0));
        assert!(!start(PUBLIC, 2), "not in a voice channel");
        assert!(!start(PUBLIC, 4));
        // Posts are started with Send Messages.
        assert!(start(Permissions::SEND_MESSAGES, 15) && !start(PUBLIC, 15));
    }

    #[test]
    fn a_message_takes_a_thread_once_and_only_if_somebody_wrote_it() {
        let (guild, channel) = place(PUBLIC, 0);
        let from = |m: &Message| can_start(&guild, &channel, Some(m), &me(), 0);
        assert!(from(&message(0, 0)) && from(&message(19, 0)), "a message and a reply");
        assert!(!from(&message(0, 1 << 5)), "it has one");
        assert!(!from(&message(7, 0)), "a join notice");
        assert!(!from(&message(21, 0)), "a thread's starter");
    }

    #[test]
    fn private_threads_are_for_text_channels_with_their_own_permission() {
        let privately = |granted, kind| {
            let (guild, channel) = place(granted, kind);
            (can_start_private(&guild, &channel, &me(), 0), Privacy::of(&guild, &channel, &me(), 0))
        };
        assert_eq!(privately(Permissions::CREATE_PRIVATE_THREADS | PUBLIC, 0), (true, Privacy::Choice));
        assert_eq!(privately(Permissions::CREATE_PRIVATE_THREADS, 0), (true, Privacy::PrivateOnly));
        assert_eq!(privately(PUBLIC, 0), (false, Privacy::Public));
        assert_eq!(privately(Permissions::CREATE_PRIVATE_THREADS, 5), (false, Privacy::Public), "not in announcements");
        let (guild, channel) = place(Permissions::CREATE_PRIVATE_THREADS, 0);
        assert!(offered_in_attach_menu(&guild, &channel, &me(), 0), "private alone offers it");
        let (guild, channel) = place(Permissions::NONE, 0);
        assert!(!offered_in_attach_menu(&guild, &channel, &me(), 0));
        assert!(Privacy::PrivateOnly.is_private(false) && !Privacy::Choice.is_private(false));
        assert!(Privacy::Choice.is_private(true) && !Privacy::Public.is_private(true));
    }

    #[test]
    fn the_request_names_the_kind_and_the_archive_time() {
        let channel = |kind, archive: serde_json::Value| -> Channel {
            serde_json::from_value(json!({"id": "9", "type": kind, "default_auto_archive_duration": archive})).unwrap()
        };
        let text = body(&channel(0, json!(1440)), "Plans", false, FROM_ATTACH_MENU);
        assert_eq!(
            text,
            json!({"name": "Plans", "type": 11, "auto_archive_duration": 1440, "location": "Plus Button"})
        );
        assert_eq!(body(&channel(0, json!(null)), "p", true, FROM_ATTACH_MENU)["type"], 12);
        let news = body(&channel(5, json!(60)), "n", false, FROM_MESSAGE_MENU);
        assert_eq!(
            (&news["type"], &news["auto_archive_duration"], &news["location"]),
            (&json!(10), &json!(60), &json!("Message"))
        );
        assert_eq!(body(&channel(0, json!(null)), "n", false, "x")["auto_archive_duration"], 4320, "no default");
    }

    #[test]
    fn the_form_wants_a_name_unless_it_comes_from_a_message() {
        assert_eq!(missing(false, "  ", "hi", false), [Missing::Name]);
        assert_eq!(missing(false, "a", "", false), [Missing::Message]);
        assert_eq!(missing(false, "", "", false), [Missing::Name, Missing::Message]);
        assert!(missing(true, "", "hi", false).is_empty(), "it is named from the message");
        assert!(missing(false, "a", "", true).is_empty(), "a file says it");
    }

    #[test]
    fn names_are_single_lines_cut_in_utf16() {
        assert_eq!(single_line(" a\r\nb\nc ", false), " a b c ");
        assert_eq!(single_line(" a\nb ", true), "a b");
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "abc");
        assert_eq!(cut("a🙂b", 2), "a", "an emoji is two units and isn't split");
        assert_eq!(cut("a🙂b", 3), "a🙂");
    }

    #[test]
    fn a_thread_is_named_from_its_message() {
        let name = |text: &str| default_name(None, None, text);
        assert_eq!(name("hello world, how are you"), "hello world, how are you");
        assert_eq!(name("# Heading\nbody"), "Heading", "the first line, without its heading mark");
        assert_eq!(name("- item"), "item");
        // Whole words while they fit in 40 units; the separator after the last stays.
        assert_eq!(name("alpha beta gamma delta epsilon zeta eta theta"), "alpha beta gamma delta epsilon zeta eta ");
        assert_eq!(name(&"x".repeat(45)), format!("{}...", "x".repeat(40)), "one long word is cut");
        assert_eq!(name(""), "Thread");
        assert_eq!(name("   "), "Thread");
        assert_eq!(name("@@@"), "@@@", "only separators");
        // The title of an embed, or a poll's question, comes first.
        assert_eq!(default_name(Some("A title"), Some("Q?"), "text"), "A title");
        assert_eq!(default_name(Some(""), Some("Q?"), "text"), "Q?");
        assert_eq!(default_name(None, Some(&"q".repeat(81)), "text"), format!("{}...", "q".repeat(80)));
        assert_eq!(default_name(Some(&"t".repeat(41)), None, ""), format!("{}...", "t".repeat(40)));
    }

    #[test]
    fn refusals_are_told_apart() {
        let refused = |status, body: &str| Refused::from(&Error::Http { status, body: body.into() });
        assert_eq!(refused(400, r#"{"code": 160006}"#), Refused::TooManyThreads);
        assert_eq!(refused(400, r#"{"code": 160007}"#), Refused::TooManyAnnouncementThreads);
        assert_eq!(refused(429, r#"{"code": 20016, "retry_after": 4.2}"#), Refused::Slowmode(5));
        assert_eq!(refused(429, r#"{"retry_after": 1.0}"#), Refused::RateLimited);
        let blocked = |title, reason: Option<&str>| Refused::AutoMod { title, reason: reason.map(Into::into) };
        assert_eq!(refused(400, r#"{"code": 200001, "message": "Blocked"}"#), blocked(true, Some("Blocked")));
        assert_eq!(refused(400, r#"{"code": 200000}"#), blocked(false, None));
        assert_eq!(refused(400, r#"{"code": 50035, "errors": {"name": {}}}"#), Refused::Name);
        assert_eq!(refused(400, r#"{"code": 50035, "errors": {"auto_archive_duration": {}}}"#), Refused::Other);
        assert_eq!(refused(500, "oops"), Refused::Other);
        assert_eq!(Refused::from(&Error::Captcha), Refused::Other);
    }

    #[test]
    fn a_slash_thread_is_public_with_its_location() {
        let channel = |kind, archive: serde_json::Value| -> Channel {
            serde_json::from_value(json!({"id": "9", "type": kind, "default_auto_archive_duration": archive})).unwrap()
        };
        assert_eq!(
            slash_body(&channel(0, json!(1440)), "Plans"),
            json!({"name": "Plans", "type": 11, "auto_archive_duration": 1440, "location": "Slash Command"})
        );
        // Public under an announcement channel too, unlike the form's.
        let news = slash_body(&channel(5, json!(null)), "n");
        assert_eq!((&news["type"], &news["auto_archive_duration"]), (&json!(11), &json!(4320)));
    }

    #[test]
    fn slash_thread_refusals_read_as_y() {
        let alerts = |refused: Refused| refused.alerts_after_slash_command();
        assert!(alerts(Refused::TooManyThreads) && alerts(Refused::TooManyAnnouncementThreads));
        assert!(alerts(Refused::RateLimited) && alerts(Refused::Other));
        assert!(!alerts(Refused::Slowmode(5)), "only the cooldown");
        assert!(!alerts(Refused::AutoMod { title: true, reason: None }) && !alerts(Refused::Name), "rethrown unseen");
    }
}
