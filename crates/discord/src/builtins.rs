//! The official client's own commands, which sit in the `/` suggestions
//! beside the apps': the list is `x` in 12115 (with 743361's /gif, /leave,
//! /schedule and /sticker first), offered by `F` there and by each one's
//! `predicate` through `zl` (46477), and run by `A` in 545152 through each
//! one's `execute`, never as an interaction. What a run sends, and the
//! lines Clyde says about it in the channel (`sendBotMessage`, 138399), are
//! here too.

use serde_json::{Value, json};

use crate::commands::{BuiltIns, Choice, Command, CommandOption, option_type};
use crate::model::{Channel, ChannelKind, Guild, Message, User};
use crate::permissions::{self, Permissions};
use crate::thread_creation;
use crate::verification::Account;
use crate::{Error, Snowflake};

/// A built-in command, in the client's order (`x`). `/roll-dice` (-18,
/// behind `2026-05-dice-roll-slash-command`) is left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltIn {
    Gif,
    Leave,
    Schedule,
    Sticker,
    Shrug,
    Tableflip,
    Unflip,
    Tts,
    Me,
    Spoiler,
    Nick,
    Thread,
    Kick,
    Ban,
    Timeout,
    Msg,
}

/// How a built-in takes what is typed (`y$` in 392054).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// `BUILT_IN`: it does something.
    Action,
    /// `BUILT_IN_TEXT`: it writes the message.
    Text,
    /// `BUILT_IN_INTEGRATION`: it searches, and the pick is sent.
    Integration(Integration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integration {
    Gif,
    Sticker,
}

impl BuiltIn {
    pub const ALL: [BuiltIn; 16] = [
        BuiltIn::Gif,
        BuiltIn::Leave,
        BuiltIn::Schedule,
        BuiltIn::Sticker,
        BuiltIn::Shrug,
        BuiltIn::Tableflip,
        BuiltIn::Unflip,
        BuiltIn::Tts,
        BuiltIn::Me,
        BuiltIn::Spoiler,
        BuiltIn::Nick,
        BuiltIn::Thread,
        BuiltIn::Kick,
        BuiltIn::Ban,
        BuiltIn::Timeout,
        BuiltIn::Msg,
    ];

    /// The client's id for it, negative so that no app's can be it; the gaps
    /// (-8, -9) are commands it dropped.
    pub fn id(self) -> &'static str {
        match self {
            BuiltIn::Gif => "-16",
            BuiltIn::Leave => "-15",
            BuiltIn::Schedule => "-19",
            BuiltIn::Sticker => "-17",
            BuiltIn::Shrug => "-1",
            BuiltIn::Tableflip => "-2",
            BuiltIn::Unflip => "-3",
            BuiltIn::Tts => "-4",
            BuiltIn::Me => "-5",
            BuiltIn::Spoiler => "-6",
            BuiltIn::Nick => "-7",
            BuiltIn::Thread => "-10",
            BuiltIn::Kick => "-11",
            BuiltIn::Ban => "-12",
            BuiltIn::Timeout => "-13",
            BuiltIn::Msg => "-14",
        }
    }

    pub fn from_id(id: &str) -> Option<BuiltIn> {
        BuiltIn::ALL.into_iter().find(|b| b.id() == id)
    }

    /// Its name after `/`, never translated.
    pub fn name(self) -> &'static str {
        match self {
            BuiltIn::Gif => "gif",
            BuiltIn::Leave => "leave",
            BuiltIn::Schedule => "schedule",
            BuiltIn::Sticker => "sticker",
            BuiltIn::Shrug => "shrug",
            BuiltIn::Tableflip => "tableflip",
            BuiltIn::Unflip => "unflip",
            BuiltIn::Tts => "tts",
            BuiltIn::Me => "me",
            BuiltIn::Spoiler => "spoiler",
            BuiltIn::Nick => "nick",
            BuiltIn::Thread => "thread",
            BuiltIn::Kick => "kick",
            BuiltIn::Ban => "ban",
            BuiltIn::Timeout => "timeout",
            BuiltIn::Msg => "msg",
        }
    }

    pub fn input(self) -> Input {
        match self {
            BuiltIn::Gif => Input::Integration(Integration::Gif),
            BuiltIn::Sticker => Input::Integration(Integration::Sticker),
            BuiltIn::Shrug | BuiltIn::Tableflip | BuiltIn::Unflip | BuiltIn::Tts | BuiltIn::Me | BuiltIn::Spoiler => {
                Input::Text
            }
            BuiltIn::Leave
            | BuiltIn::Schedule
            | BuiltIn::Nick
            | BuiltIn::Thread
            | BuiltIn::Kick
            | BuiltIn::Ban
            | BuiltIn::Timeout
            | BuiltIn::Msg => Input::Action,
        }
    }

    /// What the suggestions say of it. /unflip's shows another face than
    /// the one it sends, as the client's does.
    pub fn description(self) -> &'static str {
        match self {
            BuiltIn::Gif => "Search Animated GIFs on the Web",
            BuiltIn::Leave => "Leave Group",
            BuiltIn::Schedule => "Schedule a message to send later",
            BuiltIn::Sticker => "Search your stickers",
            BuiltIn::Shrug => "Appends ¯\\_(ツ)_/¯ to your message.",
            BuiltIn::Tableflip => "Appends (╯°□°)╯︵ ┻━┻ to your message.",
            BuiltIn::Unflip => "Appends ┬─┬ ノ( ゜-゜ノ) to your message.",
            BuiltIn::Tts => "Use text-to-speech to read the message to all members currently viewing the channel.",
            BuiltIn::Me => "Displays text with emphasis.",
            BuiltIn::Spoiler => "Marks your message as a spoiler.",
            BuiltIn::Nick => "Change nickname on this server.",
            BuiltIn::Thread => "Start new thread",
            BuiltIn::Kick => "Kick user",
            BuiltIn::Ban => "Ban user",
            BuiltIn::Timeout => "Timeout user",
            BuiltIn::Msg => "Message user",
        }
    }

    /// Its options, as the client defines them. `max_message_length` is the
    /// account's message limit (`a` in 135621), which /thread's, /msg's and
    /// /schedule's message may fill. An option's description is the one the
    /// client shows (`displayDescription`), which only /leave's `silent`
    /// has apart from its plain one ("silent").
    fn options(self, max_message_length: u64) -> Vec<CommandOption> {
        use option_type::{BOOLEAN, INTEGER, STRING, USER};
        let message = |description| vec![option("message", STRING, false, description)];
        let reason = |description| option("reason", STRING, false, description);
        let longest = |mut option: CommandOption| {
            option.max_length = Some(max_message_length);
            option
        };
        match self {
            BuiltIn::Gif => vec![option("query", STRING, true, "Search for a GIF")],
            BuiltIn::Sticker => vec![option("query", STRING, true, "Search for a sticker")],
            BuiltIn::Leave => vec![option("silent", BOOLEAN, false, "Leave without notifying other members")],
            BuiltIn::Schedule => vec![longest(option("message", STRING, true, "The message to send"))],
            BuiltIn::Shrug | BuiltIn::Tableflip | BuiltIn::Unflip => message("Your message"),
            BuiltIn::Tts | BuiltIn::Me | BuiltIn::Spoiler => vec![option("message", STRING, true, "Your message")],
            BuiltIn::Nick => {
                let mut nick = option("new_nick", STRING, false, "New nickname");
                nick.max_length = Some(NICK_MAX);
                vec![nick]
            }
            BuiltIn::Thread => {
                let mut name = option("name", STRING, true, "Type a name for your thread");
                name.max_length = Some(thread_creation::NAME_MAX as u64);
                vec![name, longest(option("message", STRING, true, "Type the first message in your thread"))]
            }
            BuiltIn::Kick => {
                vec![option("user", USER, true, "The user to kick"), reason("The reason for kicking, if any")]
            }
            BuiltIn::Ban => {
                let mut delete =
                    option("delete_messages", INTEGER, true, "How much of their recent message history to delete");
                delete.choices = Some(choices(&DELETE_MESSAGES));
                vec![option("user", USER, true, "The user to ban"), delete, reason("The reason for banning, if any")]
            }
            BuiltIn::Timeout => {
                let mut duration = option("duration", INTEGER, true, "How long they should be timed out for");
                duration.choices = Some(choices(&TIMEOUTS));
                vec![
                    option("user", USER, true, "The user to timeout"),
                    duration,
                    reason("The reason for timing them out, if any"),
                ]
            }
            BuiltIn::Msg => vec![
                option("user", USER, true, "The user to message"),
                longest(option("message", STRING, true, "Message to send")),
            ],
        }
    }

    /// It as a command the suggestions list and the box fills in.
    pub fn command(self, max_message_length: u64) -> Command {
        Command::built_in(self, self.description(), self.options(max_message_length))
    }

    /// Whether it is offered where `facts` say: its `predicate` (12115,
    /// 743361), given the permission context as `zl` gives it.
    fn offered_in(self, facts: &Facts) -> bool {
        let guild_wide = |permission| facts.guild.is_some_and(|p| p.contains(permission));
        match self {
            BuiltIn::Tts => {
                !facts.private && facts.tts_command && facts.channel.contains(Permissions::SEND_TTS_MESSAGES)
            }
            BuiltIn::Nick => {
                !facts.private
                    && (facts.channel.contains(Permissions::CHANGE_NICKNAME)
                        || facts.channel.contains(Permissions::MANAGE_NICKNAMES))
            }
            BuiltIn::Thread => facts.starts_threads,
            BuiltIn::Kick => guild_wide(Permissions::KICK_MEMBERS),
            BuiltIn::Ban => guild_wide(Permissions::BAN_MEMBERS),
            BuiltIn::Timeout => guild_wide(Permissions::MODERATE_MEMBERS),
            BuiltIn::Leave => facts.group_dm,
            // `kJ` in 566908.
            BuiltIn::Schedule => facts.private || facts.channel.contains(Permissions::SEND_MESSAGES),
            BuiltIn::Gif
            | BuiltIn::Sticker
            | BuiltIn::Shrug
            | BuiltIn::Tableflip
            | BuiltIn::Unflip
            | BuiltIn::Me
            | BuiltIn::Spoiler
            | BuiltIn::Msg => true,
        }
    }
}

/// The longest nickname /nick takes.
const NICK_MAX: u64 = 32;

/// /ban's `delete_messages` choices (`O.A.Seconds` in 927813).
const DELETE_MESSAGES: [(&str, u64); 7] = [
    ("Don't Delete Any", 0),
    ("Previous Hour", 3600),
    ("Previous 6 Hours", 6 * 3600),
    ("Previous 12 Hours", 12 * 3600),
    ("Previous 24 Hours", 86400),
    ("Previous 3 Days", 3 * 86400),
    ("Previous 7 Days", 7 * 86400),
];

/// /timeout's `duration` choices: `DisableCommunicationDuration` with
/// `getFriendlyDurationString`'s labels (200700).
const TIMEOUTS: [(&str, u64); 6] =
    [("60 secs", 60), ("5 mins", 300), ("10 mins", 600), ("1 hour", 3600), ("1 day", 86400), ("1 week", 604800)];

fn option(name: &str, kind: u8, required: bool, description: &str) -> CommandOption {
    CommandOption {
        name: name.into(),
        display_name: name.into(),
        server_name: None,
        description: description.into(),
        kind,
        required,
        choices: None,
        autocomplete: false,
        min_value: None,
        max_value: None,
        min_length: None,
        max_length: None,
        channel_types: None,
        file_types: Vec::new(),
    }
}

fn choices(list: &[(&str, u64)]) -> Vec<Choice> {
    list.iter().map(|(name, value)| Choice { name: (*name).into(), value: json!(value) }).collect()
}

/// What the predicates read of where the box is, from its permission
/// context: the channel, or a thread's parent (`Bh` in 240591, which is
/// what `zl` hands a predicate as `channel`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// A DM or a group DM (`isPrivate`).
    pub private: bool,
    pub group_dm: bool,
    /// Ours in the permission context; none in a DM.
    pub channel: Permissions,
    /// Ours in the server as a whole; none in a DM.
    pub guild: Option<Permissions>,
    /// A thread may be started in the permission context (`D1` in 406704).
    pub starts_threads: bool,
    /// `enable_tts_command` (see [`crate::settings::Settings::tts_command`]).
    pub tts_command: bool,
}

impl Facts {
    /// For the box in `channel`, one of `guild`'s or a DM, at `now`.
    pub fn of(guild: Option<&Guild>, channel: &Channel, me: &Account, tts_command: bool, now: i64) -> Facts {
        let Some(guild) = guild else {
            return Facts {
                private: channel.kind.is_private(),
                group_dm: channel.kind == ChannelKind::GroupDm,
                tts_command,
                ..Facts::default()
            };
        };
        let parent = channel.parent_id.filter(|_| channel.kind.is_thread());
        let context = parent.and_then(|p| guild.channel(p)).unwrap_or(channel);
        Facts {
            private: false,
            group_dm: false,
            channel: permissions::in_channel(guild, context, me, now),
            guild: Some(permissions::in_guild(guild, me, now)),
            starts_threads: thread_creation::can_start(guild, context, None, me, now),
            tts_command,
        }
    }
}

/// The built-ins offered where `facts` say, in the client's order (`F` in
/// 12115, then each predicate): all of them, or while a reply is pending
/// only those that write the message and the two that search.
pub fn offered(facts: &Facts, offer: BuiltIns) -> Vec<BuiltIn> {
    BuiltIn::ALL
        .into_iter()
        .filter(|b| offer == BuiltIns::All || b.input() != Input::Action)
        .filter(|b| b.offered_in(facts))
        .collect()
}

/// [`offered`] as the commands the suggestions list beside the apps'.
pub fn commands(facts: &Facts, offer: BuiltIns, max_message_length: u64) -> Vec<Command> {
    offered(facts, offer).into_iter().map(|b| b.command(max_message_length)).collect()
}

/// What running a built-in does, from the options [`crate::commands::run_options`]
/// made of what was typed (`T` in 545152), each read as `w` in 12115 reads
/// it, with its `execute`'s defaults.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The text ones: the message the box then sends as if it was typed
    /// (`handleSendMessage`, 699344), read aloud with `tts`.
    Message {
        content: String,
        tts: bool,
    },
    /// The nickname to set; empty resets it.
    Nick(String),
    Thread {
        name: String,
        message: String,
    },
    Kick {
        user: Snowflake,
        reason: String,
    },
    Ban {
        user: Snowflake,
        delete_message_seconds: u64,
        reason: String,
    },
    Timeout {
        user: Snowflake,
        seconds: u64,
        reason: String,
    },
    Msg {
        user: Snowflake,
        message: String,
    },
    /// Ask first, then leave the group DM.
    Leave {
        silent: bool,
    },
    /// Pick a time, then schedule the message.
    Schedule(String),
}

/// What running `builtin` with `options` does; none for /gif and /sticker,
/// which run only by picking a result (the client shakes), for /schedule
/// with no message, and where a required user isn't one.
pub fn outcome(builtin: BuiltIn, options: &[Value]) -> Option<Outcome> {
    let value = |name: &str| options.iter().find(|o| o["name"] == name).map(|o| &o["value"]);
    let text = |name: &str| value(name).and_then(Value::as_str).unwrap_or_default().to_owned();
    let number = |name: &str| value(name).and_then(Value::as_u64);
    let user = || value("user").and_then(Value::as_str).and_then(|u| u.parse().ok()).map(Snowflake);
    let message = |content: String| Some(Outcome::Message { content, tts: false });
    let appended = |face: &str| message(format!("{} {face}", text("message")).trim().to_owned());
    match builtin {
        BuiltIn::Gif | BuiltIn::Sticker => None,
        BuiltIn::Shrug => appended("¯\\_(ツ)_/¯"),
        BuiltIn::Tableflip => appended("(╯°□°)╯︵ ┻━┻"),
        BuiltIn::Unflip => appended("┬─┬ノ( º _ ºノ)"),
        BuiltIn::Tts => Some(Outcome::Message { content: text("message"), tts: true }),
        BuiltIn::Me => message(format!("_{}_", text("message"))),
        // `ZGg` in 652215.
        BuiltIn::Spoiler => message(format!("||{}||", text("message")).trim().to_owned()),
        BuiltIn::Nick => Some(Outcome::Nick(text("new_nick"))),
        BuiltIn::Thread => Some(Outcome::Thread { name: text("name"), message: text("message") }),
        BuiltIn::Kick => Some(Outcome::Kick { user: user()?, reason: text("reason") }),
        BuiltIn::Ban => Some(Outcome::Ban {
            user: user()?,
            delete_message_seconds: number("delete_messages").unwrap_or(0),
            reason: text("reason"),
        }),
        BuiltIn::Timeout => {
            Some(Outcome::Timeout { user: user()?, seconds: number("duration").unwrap_or(0), reason: text("reason") })
        }
        BuiltIn::Msg => Some(Outcome::Msg { user: user()?, message: text("message") }),
        BuiltIn::Leave => Some(Outcome::Leave { silent: value("silent").and_then(Value::as_bool).unwrap_or(false) }),
        BuiltIn::Schedule => Some(text("message")).filter(|m| !m.is_empty()).map(Outcome::Schedule),
    }
}

/// /nick's `PATCH /guilds/{guild}/members/@me/nick` (`changeNickname`,
/// 715943).
pub fn nick_body(nick: &str) -> Value {
    json!({ "nick": nick })
}

/// /kick's query (`kickUser`, 778858): `reason` is always there, empty when
/// none was typed, since the HTTP layer (620632) drops only what is null.
pub fn kick_query(reason: &str) -> String {
    format!("reason={}", crate::gifs::component(reason))
}

/// /ban's body (`banUser`, 778858).
pub fn ban_body(delete_message_seconds: u64) -> Value {
    json!({ "delete_message_seconds": delete_message_seconds })
}

/// When a timeout of `seconds` from `now_ms` ends, as `moment().add(…,
/// "s").toISOString()` writes it: UTC, with milliseconds.
pub fn timeout_until(now_ms: i64, seconds: u64) -> String {
    let at = now_ms + seconds as i64 * 1000;
    let whole = glib::DateTime::from_unix_utc(at.div_euclid(1000))
        .and_then(|t| t.format("%Y-%m-%dT%H:%M:%S"))
        .map_or_else(|_| String::new(), String::from);
    format!("{whole}.{:03}Z", at.rem_euclid(1000))
}

/// /timeout's body (`setCommunicationDisabledUntil`, 778858).
pub fn timeout_body(until: &str) -> Value {
    json!({ "communication_disabled_until": until })
}

/// `X-Audit-Log-Reason` as 620632 sets it on /ban and /timeout: the reason
/// as `encodeURIComponent` writes it, sent empty when none was typed.
pub fn audit_reason(reason: &str) -> String {
    crate::gifs::component(reason)
}

/// /leave's query (`closePrivateChannel`, 308528): `silent` is always there.
pub fn leave_query(silent: bool) -> String {
    format!("silent={silent}")
}

/// /leave's confirmation (743361 through 314116): the title and the body,
/// whose `**` is bold. `name` is the group's (`m1` in 47167); a group an
/// app manages (`isManaged`) has its own words.
pub fn leave_dialog(name: &str, managed: bool) -> (String, String) {
    if managed {
        (
            format!("Leave {name} Group"),
            format!(
                "Are you sure you want to leave? You won't be able to rejoin this group unless\n**{name}** adds you again."
            ),
        )
    } else {
        (
            format!("Leave '{name}'"),
            format!(
                "Are you sure you want to leave **{name}**? You won't be able to rejoin this group unless you are re-invited."
            ),
        )
    }
}

/// Who a Clyde line names: the target's tag ([`User::tag`]), else (only
/// /ban gets here with a user nobody knows) the raw id.
pub fn target_name(user: Option<&User>, id: Snowflake) -> String {
    user.map_or_else(|| id.to_string(), User::tag)
}

/// What Clyde says in the channel after a built-in ran (12115, 715943),
/// only to us: nothing goes to Discord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The nickname Discord answered with; none when it was reset.
    Nick(Option<String>),
    NickForbidden,
    NickFailed,
    /// Who was kicked, banned or timed out, by [`target_name`].
    Kicked(String),
    KickForbidden,
    KickFailed,
    Banned(String),
    BanForbidden,
    BanFailed,
    TimedOut {
        user: String,
        seconds: u64,
    },
    TimeoutForbidden,
    TimeoutFailed,
    /// The DM with the user couldn't be opened.
    MsgFailed,
}

impl Notice {
    /// 715943's two branches: the nick Discord answered with (none or empty
    /// when reset), else a 403 or anything else.
    pub fn of_nick(result: &Result<Option<String>, Error>) -> Notice {
        match result {
            Ok(nick) => Notice::Nick(nick.clone().filter(|n| !n.is_empty())),
            Err(Error::Http { status: 403, .. }) => Notice::NickForbidden,
            Err(_) => Notice::NickFailed,
        }
    }

    /// The client's words, `**` being bold as the chat draws it.
    pub fn text(&self) -> String {
        let oops = |what: &str| format!("Oops! We encountered an error when trying to {what} that user.");
        let forbidden = |what: &str| format!("You do not have the appropriate permissions to {what} that user.");
        match self {
            Notice::Nick(Some(nick)) => format!("Your nickname on this server has been changed to **{nick}**."),
            Notice::Nick(None) => "Your nickname on this server has been reset.".into(),
            Notice::NickForbidden => "You do not have the **Change Nickname** permission on this server.".into(),
            Notice::NickFailed => "Your nickname could not be changed on this server.".into(),
            Notice::Kicked(user) => format!("{user} was kicked from the server."),
            Notice::KickForbidden => forbidden("kick"),
            Notice::KickFailed => oops("kick"),
            Notice::Banned(user) => format!("{user} was banned from the server."),
            Notice::BanForbidden => forbidden("ban"),
            Notice::BanFailed => oops("ban"),
            // An ICU number, which en-US groups by thousands.
            Notice::TimedOut { user, seconds } => format!("{user} timed out for {} seconds.", grouped(*seconds)),
            Notice::TimeoutForbidden => forbidden("timeout"),
            Notice::TimeoutFailed => oops("timeout"),
            Notice::MsgFailed => oops("message"),
        }
    }

    /// The local message it is said in (`pO` in 963852): ephemeral (flags
    /// 64), a plain message (type 0) from Clyde, a bot with the id 1. The
    /// client draws a bundled picture for it, so it has no avatar to fetch.
    /// `id` is a new snowflake and `now` the time, as ISO 8601.
    pub fn message(&self, channel: Snowflake, id: Snowflake, now: &str) -> Message {
        serde_json::from_value(json!({
            "id": id.to_string(),
            "type": 0,
            "flags": EPHEMERAL,
            "content": self.text(),
            "channel_id": channel.to_string(),
            "author": { "id": "1", "username": "Clyde", "discriminator": "0000", "bot": true },
            "attachments": [],
            "embeds": [],
            "mentions": [],
            "timestamp": now,
            "tts": false,
        }))
        .expect("a local message reads as one")
    }
}

/// `EPHEMERAL` in the message flags.
const EPHEMERAL: u64 = 1 << 6;

/// `n` with a comma between each three digits, as en-US writes numbers.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: &str = "100000000000000010";
    const TEXT: Snowflake = Snowflake(100_000_000_000_000_020);
    const THREAD: Snowflake = Snowflake(100_000_000_000_000_021);
    const ME: Snowflake = Snowflake(100_000_000_000_000_001);

    fn me() -> Account {
        Account { id: ME, verified: true, email: true, phone: false, mfa: false, nsfw_allowed: false }
    }

    /// A server whose `@everyone` has `everyone`, with a text channel whose
    /// overwrite for `@everyone` denies `denied`, and a thread in it.
    fn guild(everyone: Permissions, denied: Permissions) -> Guild {
        serde_json::from_value(json!({
            "id": GUILD, "name": "G", "owner_id": "2",
            "roles": [{"id": GUILD, "name": "@everyone", "permissions": everyone.0.to_string()}],
            "channels": [
                {"id": TEXT.to_string(), "type": 0,
                 "permission_overwrites": [{"id": GUILD, "type": 0, "allow": "0", "deny": denied.0.to_string()}]},
                {"id": THREAD.to_string(), "type": 11, "parent_id": TEXT.to_string()},
            ],
        }))
        .unwrap()
    }

    fn facts_in(guild: &Guild, channel: Snowflake, tts_command: bool) -> Facts {
        Facts::of(Some(guild), guild.channel(channel).unwrap(), &me(), tts_command, 0)
    }

    fn private(kind: u8) -> Facts {
        let channel: Channel = serde_json::from_value(json!({"id": "9", "type": kind})).unwrap();
        Facts::of(None, &channel, &me(), true, 0)
    }

    fn names(builtins: &[BuiltIn]) -> Vec<&str> {
        builtins.iter().map(|b| b.name()).collect()
    }

    const SEE: Permissions = Permissions(Permissions::VIEW_CHANNEL.0 | Permissions::SEND_MESSAGES.0);

    #[test]
    fn each_built_in_has_its_own_id() {
        for builtin in BuiltIn::ALL {
            assert_eq!(BuiltIn::from_id(builtin.id()), Some(builtin));
            assert!(builtin.id().starts_with('-'));
        }
        assert_eq!(BuiltIn::from_id("-8"), None, "dropped");
        assert_eq!(BuiltIn::from_id("910"), None);
    }

    #[test]
    fn the_list_is_the_clients_in_its_order() {
        let ids: Vec<&str> = BuiltIn::ALL.iter().map(|b| b.id()).collect();
        assert_eq!(
            ids,
            ["-16", "-15", "-19", "-17", "-1", "-2", "-3", "-4", "-5", "-6", "-7", "-10", "-11", "-12", "-13", "-14"]
        );
        let shrug = BuiltIn::Shrug.command(2000);
        assert_eq!((shrug.id.as_str(), shrug.name.as_str(), shrug.display_name.as_str()), ("-1", "shrug", "shrug"));
        assert_eq!(shrug.description, "Appends ¯\\_(ツ)_/¯ to your message.");
        assert_eq!(shrug.kind, crate::commands::command_type::CHAT);
        assert_eq!((shrug.builtin, shrug.root_id()), (Some(BuiltIn::Shrug), None));
        // /unflip shows one face and sends another.
        assert_eq!(BuiltIn::Unflip.description(), "Appends ┬─┬ ノ( ゜-゜ノ) to your message.");
        // /leave's option shows its display description, not "silent".
        let leave = BuiltIn::Leave.command(2000);
        assert_eq!(leave.description, "Leave Group");
        assert_eq!(
            (leave.options[0].name.as_str(), leave.options[0].description.as_str(), leave.options[0].required),
            ("silent", "Leave without notifying other members", false)
        );
        let gif = BuiltIn::Gif.command(2000);
        assert_eq!(
            (gif.description.as_str(), gif.options[0].description.as_str()),
            ("Search Animated GIFs on the Web", "Search for a GIF")
        );
        assert!(gif.options[0].required && gif.integration() == Some(Integration::Gif));
        let sticker = BuiltIn::Sticker.command(2000);
        assert_eq!(
            (sticker.description.as_str(), sticker.options[0].description.as_str()),
            ("Search your stickers", "Search for a sticker")
        );
        assert_eq!(sticker.integration(), Some(Integration::Sticker));
        let kick = BuiltIn::Kick.command(2000);
        let kinds: Vec<(&str, u8, bool)> = kick.options.iter().map(|o| (o.name.as_str(), o.kind, o.required)).collect();
        assert_eq!(kinds, [("user", option_type::USER, true), ("reason", option_type::STRING, false)]);
        assert_eq!(BuiltIn::Timeout.command(2000).description, "Timeout user");
    }

    #[test]
    fn options_carry_the_built_ins_limits_and_choices() {
        let nick = BuiltIn::Nick.command(2000);
        assert_eq!(
            (nick.options[0].name.as_str(), nick.options[0].max_length, nick.options[0].required),
            ("new_nick", Some(32), false)
        );
        let thread = BuiltIn::Thread.command(4000);
        assert_eq!((thread.options[0].max_length, thread.options[1].max_length), (Some(100), Some(4000)));
        assert_eq!(BuiltIn::Msg.command(2000).options[1].max_length, Some(2000));
        assert_eq!(BuiltIn::Schedule.command(4000).options[0].max_length, Some(4000));
        let ban = BuiltIn::Ban.command(2000);
        let choices: Vec<(String, Value)> =
            ban.options[1].choices.clone().unwrap().into_iter().map(|c| (c.name, c.value)).collect();
        assert_eq!(choices[0], ("Don't Delete Any".into(), json!(0)));
        assert_eq!(choices[6], ("Previous 7 Days".into(), json!(604800)));
        assert!(ban.options[1].required, "delete_messages is required");
        let timeout = BuiltIn::Timeout.command(2000);
        let labels: Vec<String> = timeout.options[1].choices.clone().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(labels, ["60 secs", "5 mins", "10 mins", "1 hour", "1 day", "1 week"]);
    }

    #[test]
    fn a_reply_keeps_only_the_text_ones() {
        let all = Permissions::ALL;
        let facts = facts_in(&guild(all, Permissions::NONE), TEXT, true);
        assert_eq!(
            names(&offered(&facts, BuiltIns::TextOnly)),
            ["gif", "sticker", "shrug", "tableflip", "unflip", "tts", "me", "spoiler"]
        );
        assert_eq!(offered(&facts, BuiltIns::All).len(), 15, "everything but /leave in a server");
    }

    #[test]
    fn tts_needs_a_server_channel_the_setting_and_send_tts() {
        let tts = |facts: Facts| offered(&facts, BuiltIns::All).contains(&BuiltIn::Tts);
        let server = guild(SEE | Permissions::SEND_TTS_MESSAGES, Permissions::NONE);
        assert!(tts(facts_in(&server, TEXT, true)));
        assert!(!tts(facts_in(&server, TEXT, false)), "turned off in the settings");
        assert!(!tts(facts_in(
            &guild(SEE | Permissions::SEND_TTS_MESSAGES, Permissions::SEND_TTS_MESSAGES),
            TEXT,
            true
        )));
        assert!(!tts(private(1)) && !tts(private(3)));
    }

    #[test]
    fn nick_needs_change_or_manage_nicknames_and_a_server() {
        let nick = |facts: Facts| offered(&facts, BuiltIns::All).contains(&BuiltIn::Nick);
        assert!(nick(facts_in(&guild(SEE | Permissions::CHANGE_NICKNAME, Permissions::NONE), TEXT, true)));
        assert!(nick(facts_in(&guild(SEE | Permissions::MANAGE_NICKNAMES, Permissions::NONE), TEXT, true)));
        assert!(!nick(facts_in(&guild(SEE, Permissions::NONE), TEXT, true)));
        // A channel's overwrite counts: the predicate asks the channel.
        assert!(!nick(facts_in(&guild(SEE | Permissions::CHANGE_NICKNAME, Permissions::CHANGE_NICKNAME), TEXT, true)));
        assert!(!nick(private(1)));
    }

    #[test]
    fn thread_follows_the_permission_context() {
        let thread = |facts: Facts| offered(&facts, BuiltIns::All).contains(&BuiltIn::Thread);
        let starts = SEE | Permissions::CREATE_PUBLIC_THREADS | Permissions::READ_MESSAGE_HISTORY;
        let server = guild(starts, Permissions::NONE);
        assert!(thread(facts_in(&server, TEXT, true)));
        // Inside a thread the parent decides, as the client asks it.
        assert!(thread(facts_in(&server, THREAD, true)));
        assert!(!thread(facts_in(&guild(SEE | Permissions::CREATE_PUBLIC_THREADS, Permissions::NONE), TEXT, true)));
        assert!(!thread(private(1)));
    }

    #[test]
    fn kick_ban_timeout_follow_server_wide_permissions() {
        let moderate = Permissions::KICK_MEMBERS | Permissions::BAN_MEMBERS | Permissions::MODERATE_MEMBERS;
        let listed = |facts: Facts| {
            offered(&facts, BuiltIns::All)
                .into_iter()
                .filter(|b| matches!(b, BuiltIn::Kick | BuiltIn::Ban | BuiltIn::Timeout))
                .count()
        };
        // A channel's overwrite doesn't matter.
        assert_eq!(listed(facts_in(&guild(SEE | moderate, moderate), TEXT, true)), 3);
        assert_eq!(listed(facts_in(&guild(SEE | Permissions::BAN_MEMBERS, Permissions::NONE), TEXT, true)), 1);
        assert_eq!(listed(facts_in(&guild(SEE, Permissions::NONE), TEXT, true)), 0);
        assert_eq!(listed(private(1)), 0);
    }

    #[test]
    fn leave_only_in_group_dms_and_schedule_in_dms_or_with_send_messages() {
        assert_eq!(
            names(&offered(&private(3), BuiltIns::All)),
            ["gif", "leave", "schedule", "sticker", "shrug", "tableflip", "unflip", "me", "spoiler", "msg"]
        );
        assert!(!offered(&private(1), BuiltIns::All).contains(&BuiltIn::Leave));
        assert!(offered(&private(1), BuiltIns::All).contains(&BuiltIn::Schedule));
        let schedule = |facts: Facts| offered(&facts, BuiltIns::All).contains(&BuiltIn::Schedule);
        assert!(schedule(facts_in(&guild(SEE, Permissions::NONE), TEXT, true)));
        assert!(!schedule(facts_in(&guild(SEE, Permissions::SEND_MESSAGES), TEXT, true)));
        // In a thread, Send Messages in its parent.
        assert!(!schedule(facts_in(&guild(SEE, Permissions::SEND_MESSAGES), THREAD, true)));
    }

    fn run(builtin: BuiltIn, options: Value) -> Option<Outcome> {
        outcome(builtin, options.as_array().unwrap())
    }

    fn text(name: &str, value: &str) -> Value {
        json!({"type": 3, "name": name, "value": value})
    }

    #[test]
    fn text_ones_say_what_the_client_says() {
        let said = |builtin, options: Value| match run(builtin, options) {
            Some(Outcome::Message { content, tts }) => (content, tts),
            other => panic!("{other:?}"),
        };
        assert_eq!(said(BuiltIn::Shrug, json!([text("message", "hi")])), ("hi ¯\\_(ツ)_/¯".into(), false));
        assert_eq!(said(BuiltIn::Shrug, json!([])), ("¯\\_(ツ)_/¯".into(), false), "trimmed");
        assert_eq!(said(BuiltIn::Tableflip, json!([])).0, "(╯°□°)╯︵ ┻━┻");
        assert_eq!(said(BuiltIn::Unflip, json!([text("message", "ok")])).0, "ok ┬─┬ノ( º _ ºノ)");
        assert_eq!(said(BuiltIn::Me, json!([text("message", " a ")])).0, "_ a _", "not trimmed");
        assert_eq!(said(BuiltIn::Spoiler, json!([text("message", "it")])).0, "||it||");
        assert_eq!(said(BuiltIn::Tts, json!([text("message", "hello")])), ("hello".into(), true));
        assert_eq!(run(BuiltIn::Gif, json!([text("query", "cat")])), None, "a search runs by its pick");
        assert_eq!(run(BuiltIn::Sticker, json!([text("query", "cat")])), None);
    }

    #[test]
    fn actions_read_their_options_with_the_clients_defaults() {
        let user = json!({"type": 6, "name": "user", "value": "300000000000000001"});
        let target = Snowflake(300_000_000_000_000_001);
        assert_eq!(run(BuiltIn::Nick, json!([])), Some(Outcome::Nick(String::new())), "an empty nick resets it");
        assert_eq!(
            run(BuiltIn::Kick, json!([user])),
            Some(Outcome::Kick { user: target, reason: String::new() }),
            "no reason is an empty one"
        );
        assert_eq!(
            run(
                BuiltIn::Ban,
                json!([user, {"type": 4, "name": "delete_messages", "value": 3600}, text("reason", "spam")])
            ),
            Some(Outcome::Ban { user: target, delete_message_seconds: 3600, reason: "spam".into() })
        );
        assert_eq!(
            run(BuiltIn::Timeout, json!([user, {"type": 4, "name": "duration", "value": 600}])),
            Some(Outcome::Timeout { user: target, seconds: 600, reason: String::new() })
        );
        assert_eq!(
            run(BuiltIn::Msg, json!([user, text("message", "hey")])),
            Some(Outcome::Msg { user: target, message: "hey".into() })
        );
        assert_eq!(
            run(BuiltIn::Thread, json!([text("name", "Plans"), text("message", "first")])),
            Some(Outcome::Thread { name: "Plans".into(), message: "first".into() })
        );
        assert_eq!(run(BuiltIn::Leave, json!([])), Some(Outcome::Leave { silent: false }));
        assert_eq!(
            run(BuiltIn::Leave, json!([{"type": 5, "name": "silent", "value": true}])),
            Some(Outcome::Leave { silent: true })
        );
        assert_eq!(run(BuiltIn::Schedule, json!([text("message", "later")])), Some(Outcome::Schedule("later".into())));
        assert_eq!(run(BuiltIn::Schedule, json!([text("message", "")])), None, "nothing to schedule");
    }

    #[test]
    fn requests_carry_what_the_client_sends() {
        assert_eq!(nick_body(""), json!({"nick": ""}));
        assert_eq!(kick_query(""), "reason=", "always there");
        assert_eq!(kick_query("a b!*'()"), "reason=a%20b!*'()", "encodeURIComponent");
        assert_eq!(ban_body(86400), json!({"delete_message_seconds": 86400}));
        assert_eq!(audit_reason(""), "", "sent empty");
        assert_eq!(audit_reason("é & co"), "%C3%A9%20%26%20co");
        assert_eq!(leave_query(false), "silent=false");
        assert_eq!(leave_query(true), "silent=true");
        // 2026-11-13T12:00:00.250Z, a minute on.
        assert_eq!(timeout_until(1_794_571_200_250, 60), "2026-11-13T12:01:00.250Z");
        assert_eq!(timeout_until(1_794_571_200_000, 3600), "2026-11-13T13:00:00.000Z");
        assert_eq!(timeout_body("x"), json!({"communication_disabled_until": "x"}));
    }

    #[test]
    fn clyde_lines_read_as_the_clients() {
        assert_eq!(
            Notice::Nick(Some("Neo".into())).text(),
            "Your nickname on this server has been changed to **Neo**."
        );
        assert_eq!(Notice::Nick(None).text(), "Your nickname on this server has been reset.");
        assert_eq!(Notice::NickForbidden.text(), "You do not have the **Change Nickname** permission on this server.");
        assert_eq!(Notice::NickFailed.text(), "Your nickname could not be changed on this server.");
        assert_eq!(Notice::Kicked("pat".into()).text(), "pat was kicked from the server.");
        assert_eq!(Notice::KickForbidden.text(), "You do not have the appropriate permissions to kick that user.");
        assert_eq!(Notice::KickFailed.text(), "Oops! We encountered an error when trying to kick that user.");
        assert_eq!(Notice::Banned("pat#1234".into()).text(), "pat#1234 was banned from the server.");
        assert_eq!(Notice::BanFailed.text(), "Oops! We encountered an error when trying to ban that user.");
        assert_eq!(
            Notice::TimedOut { user: "pat".into(), seconds: 604800 }.text(),
            "pat timed out for 604,800 seconds."
        );
        assert_eq!(Notice::TimedOut { user: "pat".into(), seconds: 60 }.text(), "pat timed out for 60 seconds.");
        assert_eq!(
            Notice::TimeoutForbidden.text(),
            "You do not have the appropriate permissions to timeout that user."
        );
        assert_eq!(Notice::MsgFailed.text(), "Oops! We encountered an error when trying to message that user.");

        let http = |status| Err(Error::Http { status, body: String::new() });
        assert_eq!(Notice::of_nick(&Ok(Some("Neo".into()))), Notice::Nick(Some("Neo".into())));
        assert_eq!(Notice::of_nick(&Ok(Some(String::new()))), Notice::Nick(None));
        assert_eq!(Notice::of_nick(&Ok(None)), Notice::Nick(None));
        assert_eq!(Notice::of_nick(&http(403)), Notice::NickForbidden);
        assert_eq!(Notice::of_nick(&http(500)), Notice::NickFailed);

        let message = Notice::KickFailed.message(Snowflake(9), Snowflake(42), "2026-11-13T12:00:00.000Z");
        assert_eq!((message.id, message.channel_id, message.kind), (Snowflake(42), Snowflake(9), 0));
        assert!(message.is_ephemeral());
        assert_eq!(
            (message.author.id, message.author.username.as_str(), message.author.bot),
            (Snowflake(1), "Clyde", true)
        );
        assert_eq!(message.author.avatar, None, "nothing to fetch");
        assert_eq!(message.content, Notice::KickFailed.text());
    }

    #[test]
    fn a_target_is_named_by_its_tag_else_its_id() {
        let user: User = serde_json::from_value(json!({"id": "6", "username": "pat", "discriminator": "0"})).unwrap();
        assert_eq!(target_name(Some(&user), Snowflake(6)), "pat");
        assert_eq!(target_name(None, Snowflake(6)), "6");
    }

    #[test]
    fn leave_dialog_reads_as_the_clients() {
        assert_eq!(
            leave_dialog("Pals", false),
            (
                "Leave 'Pals'".to_owned(),
                "Are you sure you want to leave **Pals**? You won't be able to rejoin this group unless you are re-invited."
                    .to_owned()
            )
        );
        assert_eq!(
            leave_dialog("Pals", true),
            (
                "Leave Pals Group".to_owned(),
                "Are you sure you want to leave? You won't be able to rejoin this group unless\n**Pals** adds you again."
                    .to_owned()
            )
        );
    }
}
