//! Threads: which ones the sidebar lists under their parent, which the
//! thread browser lists, when one is unread, and when the client acks one.
//! Who may post in a thread is in [`crate::permissions`], which thread
//! messages notify in [`crate::notifications`].
//!
//! Only threads we've joined show in the sidebar, as in the official client:
//! those that are still active (not archived), and of those only the
//! "relevant" ones. The browser lists the others too: the active ones
//! `THREAD_LIST_SYNC` brings, and the archived ones a request pages in.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::permissions::{self, Permissions};
use crate::read_state::ReadStates;
use crate::verification::Account;
use crate::{Channel, ChannelKind, Error, Guild, Snowflake, ThreadMember};

/// What the client sends as `location` when a thread is joined or left from
/// a context menu (`joinThread` and `leaveThread` in 7909, called from the
/// thread menu, 949488).
pub const MENU_LOCATION: &str = "Context Menu";

/// The edits that close or reopen a thread, then lock or unlock it, in
/// order (`archiveThread`, `unarchiveThread`, `lockThread` and
/// `unlockThread` in 7909). Locking an archived thread reopens it first
/// and archives it again with the lock; unlocking one reopens it, unlocked,
/// and archives it again.
pub fn close_edit(close: bool) -> Value {
    json!({ "archived": close })
}

pub fn lock_edits(archived: bool, lock: bool) -> Vec<Value> {
    let mut edits = Vec::new();
    if archived {
        edits.push(if lock { json!({ "archived": false }) } else { json!({ "archived": false, "locked": false }) });
    }
    edits.push(json!({ "locked": lock, "archived": archived }));
    edits
}

/// A thread's settings the client's form edits (336541): its name, how long
/// members wait between messages, when it archives itself, who may invite to
/// a private one, and whether it hides its media.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub name: String,
    /// Seconds.
    pub slowmode: u32,
    /// Minutes.
    pub archive_after: u32,
    /// Only a private thread has the choice.
    pub invitable: Option<bool>,
    pub spoiler: bool,
}

/// `IS_SPOILER_CHANNEL`, in a channel's flags.
const IS_SPOILER: u64 = 1 << 21;

/// The seconds the form offers for slowmode (`s_7` in 652215), and the
/// minutes for archiving after inactivity (`Gk` in 268761), when it is
/// 3 days if the thread says nothing (`Gl`).
pub const SLOWMODES: [u32; 14] = [0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21_600];
pub const ARCHIVE_AFTER: [u32; 4] = [60, 1440, 4320, 10_080];
const ARCHIVE_DEFAULT: u32 = 4320;

impl Settings {
    /// What the form starts from: the thread as it is.
    pub fn of(thread: &Channel) -> Self {
        let meta = thread.thread_metadata.as_ref();
        let archive = meta.map_or(0, |m| m.auto_archive_duration);
        Settings {
            name: thread.name.clone().unwrap_or_default(),
            slowmode: thread.rate_limit_per_user,
            archive_after: if archive == 0 { ARCHIVE_DEFAULT } else { archive },
            invitable: meta.and_then(|m| m.invitable).filter(|_| thread.kind == ChannelKind::PrivateThread),
            spoiler: thread.flags & IS_SPOILER != 0,
        }
    }
}

/// Why the form's settings aren't sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// The name is empty once its line breaks are spaces and it is trimmed.
    NameRequired,
}

impl Invalid {
    /// The words next to the name (`uXA573`).
    pub fn words(self) -> &'static str {
        "Thread Name is required"
    }
}

/// The edit the form saves (`saveChannel` in 435183, from `onSave` in
/// 336541). The client sends the channel record's fields through the getters
/// it has defaults in, so a thread's carries a topic of "", a bitrate of
/// 64000, a user limit of 0 and no NSFW; whatever the record lacks is left
/// out.
pub fn settings_body(thread: &Channel, settings: &Settings) -> Result<Value, Invalid> {
    let name = crate::thread_creation::single_line(&settings.name, true);
    if name.is_empty() {
        return Err(Invalid::NameRequired);
    }
    let meta = thread.thread_metadata.as_ref();
    let flags = if settings.spoiler { thread.flags | IS_SPOILER } else { thread.flags & !IS_SPOILER };
    let mut body = json!({
        "name": name,
        "type": thread.kind.raw(),
        "topic": "",
        "bitrate": 64_000,
        "user_limit": 0,
        "nsfw": false,
        "flags": flags,
        "rate_limit_per_user": settings.slowmode,
        "auto_archive_duration": settings.archive_after,
        "locked": meta.is_some_and(|m| m.locked),
    });
    if let Some(invitable) = settings.invitable {
        body["invitable"] = json!(invitable);
    }
    Ok(body)
}

/// How the form words an archiving time (`Gk` in 268761).
pub fn archive_label(minutes: u32) -> String {
    match minutes {
        60 => "1 Hour".to_owned(),
        1440 => "24 Hours".to_owned(),
        4320 => "3 Days".to_owned(),
        10_080 => "1 Week".to_owned(),
        other => duration_label(other * 60, ""),
    }
}

/// A wait or a duration as the form words it (`$` in 879631): the days if
/// there are any, else the hours, minutes or seconds, and `off` for none.
pub fn duration_label(seconds: u32, off: &str) -> String {
    let (days, hours, minutes) = (seconds / 86_400 % 31, seconds / 3600 % 24, seconds / 60 % 60);
    let plural = |n: u32, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    if days > 0 {
        plural(days, "day")
    } else if hours > 0 {
        plural(hours, "hour")
    } else if minutes > 0 {
        plural(minutes, "minute")
    } else if seconds > 0 {
        plural(seconds % 60, "second")
    } else {
        off.to_owned()
    }
}

/// The alert for a refused reopen (`unarchiveThread`, 7909): its heading
/// and body, for a thread or a forum post.
pub fn reopen_alert(error: &Error, post: bool) -> (&'static str, &'static str) {
    use crate::thread_creation::Refused;
    let title = if post { "Cannot reopen post" } else { "Cannot reopen thread" };
    let archived = if post { "This post is archived." } else { "This thread is archived." };
    let full = if post {
        "This server has reached the maximum number of active posts. Free up space by archiving a post."
    } else {
        "This server has reached the maximum number of active threads. Free up space by archiving a thread."
    };
    match (Refused::from(error), error) {
        (Refused::TooManyThreads, _) => (title, full),
        (Refused::TooManyAnnouncementThreads, _) => (
            "Cannot reopen thread",
            "This server has reached the maximum number of active announcement threads. Free up space by archiving a thread.",
        ),
        (Refused::RateLimited | Refused::Slowmode(_), _) => (title, "You are being rate limited."),
        (_, Error::Http { status: 403, .. }) => (title, archived),
        _ => ("Error", "An error occurred, please try again."),
    }
}

/// How long a thread we opened is remembered as opened (`M` in 123363).
const VIEWED_FOR_MS: u64 = 90 * 24 * 60 * 60 * 1000;

/// Where opening a thread starts reading (`P` in 123363, which a message to
/// jump to, if there is one, overrides).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Around the thread's first message, which has the thread's id.
    Start,
    /// Around the last message we acknowledged, to read on from there.
    Unread(Snowflake),
    /// The newest page, as for any channel.
    Newest,
}

/// The threads we opened, and when: the client's `viewedThreadIds`, kept for
/// 90 days.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Viewed(HashMap<Snowflake, u64>);

impl Viewed {
    pub fn new(entries: impl IntoIterator<Item = (Snowflake, u64)>) -> Self {
        Viewed(entries.into_iter().collect())
    }

    pub fn entries(&self) -> impl Iterator<Item = (Snowflake, u64)> + '_ {
        self.0.iter().map(|(&id, &at)| (id, at))
    }

    /// Where to open `thread`. One never opened, here or on any client
    /// (`persisted`: Discord has a read state for it), opens at its start and
    /// is remembered as opened. Otherwise one with unread messages (`unread`
    /// is the last message we acknowledged) opens there, if nothing of it is
    /// shown yet (`loaded`).
    pub fn anchor(
        &mut self,
        thread: Snowflake,
        persisted: bool,
        unread: Option<Snowflake>,
        loaded: bool,
        now_ms: u64,
    ) -> Anchor {
        if !persisted && !self.0.contains_key(&thread) {
            self.0.insert(thread, now_ms);
            self.0.retain(|_, &mut at| at >= now_ms.saturating_sub(VIEWED_FOR_MS));
            return Anchor::Start;
        }
        match unread {
            Some(acked) if !loaded => Anchor::Unread(acked),
            _ => Anchor::Newest,
        }
    }
}

/// How many archived threads a page holds (`loadArchivedThreads` in 7909).
pub const PAGE: u32 = 25;

/// The threads the sidebar lists under `parent`, as the client lists them
/// (`eT` in 791513, from `ActiveJoinedThreadsStore` in 863005): active
/// threads we've joined, newest joined first, that are relevant. Under a
/// muted parent, unless it or one of its threads is open, only those that
/// are unread. The open thread comes first, listed or not.
pub fn listed<'a>(
    guild: &'a Guild,
    parent: &Channel,
    open: Option<Snowflake>,
    parent_muted: bool,
    read: &ReadStates,
    now_ms: i64,
) -> Vec<&'a Channel> {
    let opened = open == Some(parent.id);
    let open = open.and_then(|id| guild.thread(id)).filter(|t| t.parent_id == Some(parent.id));
    let opened = opened || open.is_some();
    let mut threads: Vec<&Channel> = joined_active(guild)
        .filter(|t| t.parent_id == Some(parent.id))
        .filter(|t| {
            let state = State::of(guild, t, read, now_ms);
            if parent_muted && !opened { state.unread } else { state.relevant }
        })
        .collect();
    threads.sort_by_key(|t| std::cmp::Reverse(t.member.as_ref().and_then(|m| m.join_timestamp).unwrap_or(0)));
    if let Some(open) = open
        && !threads.iter().any(|t| t.id == open.id)
    {
        threads.insert(0, open);
    }
    threads
}

/// When a thread `listed` shows next falls out of the list on its own,
/// its auto-archive window over, in milliseconds since 1970. The client
/// sets a timer for each, to look again then.
pub fn next_expiry(guild: &Guild, now_ms: i64) -> Option<i64> {
    joined_active(guild).map(window_end).filter(|&end| end > now_ms).min()
}

/// Active threads we've joined: what the server's unread dot and mention
/// count take in, besides its channels (458294).
pub fn joined_active(guild: &Guild) -> impl Iterator<Item = &Channel> {
    guild.threads.iter().filter(|t| t.member.is_some() && t.is_active_thread())
}

/// Whether `thread` has messages newer than the last we read, by the
/// client's rule for threads (`getAckTimestamp` in 218461): without a read
/// message, the time we joined, less five seconds, or when it was last
/// archived or reopened, whichever is later; and never before we joined
/// the server, except in a forum post.
pub fn is_unread(guild: &Guild, thread: &Channel, read: &ReadStates) -> bool {
    let Some(last) = thread.last_message_id else { return false };
    let acked = match read.acked(thread.id) {
        Some(acked) => acked.unix_ms(),
        None => ack_fallback(guild, thread),
    };
    acked < last.unix_ms()
}

fn ack_fallback(guild: &Guild, thread: &Channel) -> i64 {
    let joined = thread.member.as_ref().and_then(|m| m.join_timestamp).unwrap_or(0) - 5000;
    let reopened = thread.thread_metadata.as_ref().and_then(|m| m.archive_timestamp).map_or(0, |t| t - 1);
    let mut since = joined.max(reopened);
    if since <= 0 {
        since = (thread.id.unix_ms() - 1).max(0);
    }
    let forum_post = thread.kind == ChannelKind::PublicThread
        && thread.parent_id.and_then(|id| guild.channel(id)).is_some_and(|p| p.kind.is_forum_like());
    if forum_post {
        return since;
    }
    // When we joined is always known for a server we're in; the client
    // falls back to now, as if everything were read.
    let joined_guild = guild.joined_at.map_or(i64::MAX, |s| s * 1000);
    since.max(joined_guild)
}

/// How many of a forum's active posts are unread by the forum's own mark:
/// posts with something unread in them and a message after the last one we
/// acked in the forum (`ForumPostUnreadCountStore` in 435470), which is what
/// Mark As Read is greyed by. A forum we never acked has none.
pub fn forum_unread_posts(guild: &Guild, forum: &Channel, read: &ReadStates) -> usize {
    let Some(acked) = read.acked(forum.id) else { return 0 };
    guild
        .threads
        .iter()
        .filter(|t| t.parent_id == Some(forum.id) && t.is_active_thread())
        .filter(|t| t.last_message_id.is_some_and(|last| last > acked) && is_unread(guild, t, read))
        .count()
}

/// Whether the client acks what we read in `thread` (`canTrackUnreads` in
/// 218461): not in an archived thread.
pub fn is_ackable(thread: &Channel) -> bool {
    !thread.is_archived()
}

/// What the thread browser lists under a channel, as the client does (`jO`
/// and `Fr` in 424798): the active threads we've joined, the other active
/// ones, and the archived ones paged in so far; each newest activity first.
#[derive(Debug, Default)]
pub struct Browser<'a> {
    pub joined: Vec<&'a Channel>,
    pub others: Vec<&'a Channel>,
    pub archived: Vec<&'a Channel>,
}

impl Browser<'_> {
    pub fn is_empty(&self) -> bool {
        self.joined.is_empty() && self.others.is_empty() && self.archived.is_empty()
    }
}

/// The browser's lists for `parent`; `can_view` says which threads we may
/// see.
pub fn browser<'a>(
    guild: &'a Guild,
    parent: Snowflake,
    archive: &Archive,
    can_view: impl Fn(&Channel) -> bool,
) -> Browser<'a> {
    let under = |t: &&Channel| t.parent_id == Some(parent) && can_view(t);
    let newest_first = |threads: &mut Vec<&Channel>| threads.sort_by_key(|t| std::cmp::Reverse(t.activity()));
    let (mut joined, mut others): (Vec<&Channel>, Vec<&Channel>) =
        guild.threads.iter().filter(|t| t.is_active_thread()).filter(under).partition(|t| t.member.is_some());
    newest_first(&mut joined);
    newest_first(&mut others);
    let mut archived = archive.listed(guild, parent, can_view);
    newest_first(&mut archived);
    Browser { joined, others, archived }
}

/// What has been asked of one channel's archived threads, and where the next
/// page starts (`ArchivedThreadsStore`, 45494). The client asks once when
/// the browser first opens, then when it scrolls to the end of what it has,
/// and starts over only with a new session: reopening the browser shows what
/// it has.
#[derive(Debug, Default)]
pub struct Archive {
    asked: bool,
    loading: bool,
    failed: bool,
    has_more: bool,
    next_offset: u32,
    /// The oldest thread of the pages so far: what is older than it, and
    /// known from elsewhere, stays out until a page reaches it.
    oldest: Option<Snowflake>,
}

impl Archive {
    /// The offset of the first page, if the browser has yet to ask for it.
    pub fn first(&mut self) -> Option<u32> {
        if self.asked { None } else { self.begin() }
    }

    /// The offset of the next page, if there is one and the last has landed.
    pub fn more(&mut self) -> Option<u32> {
        if self.has_more && !self.loading && !self.failed { self.begin() } else { None }
    }

    fn begin(&mut self) -> Option<u32> {
        if self.loading {
            return None;
        }
        (self.asked, self.loading, self.failed) = (true, true, false);
        Some(self.next_offset)
    }

    /// The page asked for at `offset` came back: `oldest` is its last thread.
    pub fn landed(&mut self, offset: u32, has_more: bool, oldest: Option<Snowflake>) {
        self.loading = false;
        self.has_more = has_more;
        self.next_offset = offset + PAGE;
        self.oldest = oldest.or(self.oldest);
    }

    pub fn failed(&mut self) {
        (self.loading, self.failed) = (false, true);
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Archived threads under `parent` that the pages so far cover, but a
    /// media post, which the browser leaves out. Nothing until a page lands.
    fn listed<'a>(&self, guild: &'a Guild, parent: Snowflake, can_view: impl Fn(&Channel) -> bool) -> Vec<&'a Channel> {
        if self.next_offset == 0 {
            return Vec::new();
        }
        let reached = self.oldest.and_then(|id| guild.thread(id)).map(Channel::activity);
        guild
            .threads
            .iter()
            .filter(|t| t.is_archived() && t.parent_id == Some(parent) && t.kind != ChannelKind::MediaThread)
            .filter(|t| !self.has_more || reached.is_none_or(|reached| t.activity() >= reached))
            .filter(|t| can_view(t))
            .collect()
    }
}

/// Each channel's [`Archive`], for a session.
pub type Archives = HashMap<Snowflake, Archive>;

/// A page of archived threads: the threads, and our membership of those we
/// joined, which Discord lists apart.
#[derive(Debug)]
pub struct ArchivedPage {
    pub threads: Vec<Channel>,
    pub members: Vec<(Snowflake, ThreadMember)>,
    pub has_more: bool,
    /// The posts' first messages, for the pages of a forum's posts (see
    /// [`crate::forum::FirstMessages`]).
    pub first_messages: Option<Vec<crate::Message>>,
}

impl ArchivedPage {
    /// What the search answers with, if it has threads at all (the client
    /// treats a missing list as a failure).
    pub fn from_value(answer: &serde_json::Value) -> Option<ArchivedPage> {
        use serde::Deserialize;
        let threads = answer["threads"]
            .as_array()?
            .iter()
            .filter_map(|t| Channel::deserialize(t).ok())
            .filter(|t| t.kind.is_thread())
            .collect();
        let members = answer["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| Some((m["id"].as_str()?.parse().ok().map(Snowflake)?, ThreadMember::deserialize(m).ok()?)))
            .collect();
        let first_messages = answer["first_messages"]
            .as_array()
            .map(|messages| messages.iter().filter_map(|m| crate::Message::deserialize(m).ok()).collect());
        Some(ArchivedPage { threads, members, has_more: answer["has_more"].as_bool().unwrap_or(false), first_messages })
    }
}

/// How the client sorts a thread (`k` in 863005).
struct State {
    /// Unread messages in a thread not muted (if it's active or pinned), or
    /// a mention.
    unread: bool,
    /// Worth listing: unread, a mention, pinned, or still inside its
    /// auto-archive window.
    relevant: bool,
}

impl State {
    fn of(guild: &Guild, thread: &Channel, read: &ReadStates, now_ms: i64) -> State {
        let mentioned = read.mentions(thread.id) > 0;
        let muted = thread.member.as_ref().is_some_and(|m| m.is_muted(now_ms / 1000));
        let unread = is_unread(guild, thread, read) && !muted;
        let pinned = thread.is_pinned();
        let active = thread.is_active_thread();
        let recent = active && window_end(thread) > now_ms;
        State { unread: ((active || pinned) && unread) || mentioned, relevant: recent || pinned || unread || mentioned }
    }
}

/// When `thread` archives itself if nothing happens before (826767): its
/// auto-archive duration after the latest of its newest message, other
/// activity, and its last archiving or reopening.
fn window_end(thread: &Channel) -> i64 {
    let Some(meta) = &thread.thread_metadata else { return 0 };
    let newest = thread.last_message_id.unwrap_or(thread.id);
    let last = newest.unix_ms().max(thread.last_activity.unwrap_or(0)).max(meta.archive_timestamp.unwrap_or(0));
    last + i64::from(meta.auto_archive_duration) * 60_000
}

/// What the open chat takes from a thread, kept to compare before and after
/// an event touched the thread: our permissions there (a private thread
/// gives us nothing until we join it), whether it is locked or archived, and
/// what the box under the message that started it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    name: Option<String>,
    viewable: bool,
    permissions: Permissions,
    archived: bool,
    locked: bool,
    message_count: Option<u32>,
}

impl Look {
    pub fn of(guild: &Guild, thread: &Channel, me: &Account, now: i64) -> Look {
        Look {
            name: thread.name.clone(),
            viewable: permissions::can_view(guild, thread, me, now),
            permissions: permissions::in_channel(guild, thread, me, now),
            archived: thread.is_archived(),
            locked: thread.is_locked(),
            message_count: thread.message_count,
        }
    }
}

/// The [`Look`] of each of `threads` among `guild`'s, `None` for one it doesn't hold.
pub fn looks(guild: &Guild, threads: &[Snowflake], me: &Account, now: i64) -> Vec<Option<Look>> {
    // A big server syncs thousands at once: one pass over what it holds, not one a thread.
    let held: HashMap<Snowflake, &Channel> = guild.threads.iter().map(|t| (t.id, t)).collect();
    threads.iter().map(|id| held.get(id).map(|t| Look::of(guild, t, me, now))).collect()
}

/// How much of the open chat an event on threads makes draw again.
#[derive(Debug, PartialEq, Eq)]
pub enum Redraw {
    Nothing,
    /// Only the messages that started these threads, which carry the box
    /// under them.
    Starters(Vec<Snowflake>),
    /// What every message draws from names and permissions.
    Everything,
}

/// What to draw again, from each touched thread's [`Look`] before and after
/// (`None`: not held), with `open` the channel on screen. Its own thread
/// changing, a thread that wasn't held, or one that is called something else
/// or can't be seen any more (a `<#thread>` in any message says its name, or
/// doesn't) is everything; any other change shows only in the thread's
/// starter. A sync that brings what we hold already, as most do, is nothing.
pub fn redraw(
    before: &[Option<Look>],
    after: &[Option<Look>],
    touched: &[Snowflake],
    open: Option<Snowflake>,
) -> Redraw {
    let mut starters = Vec::new();
    for ((before, after), &thread) in before.iter().zip(after).zip(touched) {
        if before == after {
            continue;
        }
        let (Some(before), Some(after)) = (before, after) else { return Redraw::Everything };
        if open == Some(thread) || before.name != after.name || before.viewable != after.viewable {
            return Redraw::Everything;
        }
        starters.push(thread);
    }
    if starters.is_empty() { Redraw::Nothing } else { Redraw::Starters(starters) }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn refused(status: u32, body: &str) -> Error {
        Error::Http { status, body: body.into() }
    }

    fn channel(extra: serde_json::Value) -> Channel {
        let mut value = json!({"id": "9", "type": 11, "name": "plans", "parent_id": "2", "flags": 0});
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_form_starts_from_the_thread_and_saves_what_the_client_sends() {
        let thread = channel(json!({
            "rate_limit_per_user": 30, "flags": 1 << 1,
            "thread_metadata": {"archived": false, "locked": true, "auto_archive_duration": 1440},
        }));
        let mut settings = Settings::of(&thread);
        assert_eq!(
            settings,
            Settings { name: "plans".into(), slowmode: 30, archive_after: 1440, invitable: None, spoiler: false }
        );
        settings.name = "  new\nname ".into();
        settings.spoiler = true;
        assert_eq!(
            settings_body(&thread, &settings),
            Ok(json!({
                "name": "new name", "type": 11, "topic": "", "bitrate": 64000, "user_limit": 0, "nsfw": false,
                "flags": (1 << 1) | (1 << 21), "rate_limit_per_user": 30, "auto_archive_duration": 1440,
                "locked": true,
            })),
            "line breaks are spaces, and it is trimmed; the lock stays; a public thread has no invitable"
        );
        settings.spoiler = false;
        assert_eq!(settings_body(&thread, &settings).unwrap()["flags"], 1 << 1);
    }

    #[test]
    fn a_private_thread_says_who_may_invite_and_a_thread_without_a_duration_gets_three_days() {
        let thread = channel(json!({"type": 12, "thread_metadata": {"invitable": false}}));
        let mut settings = Settings::of(&thread);
        assert_eq!((settings.invitable, settings.archive_after), (Some(false), 4320));
        settings.invitable = Some(true);
        let body = settings_body(&thread, &settings).unwrap();
        assert_eq!((&body["type"], &body["invitable"], &body["locked"]), (&json!(12), &json!(true), &json!(false)));
        let public = channel(json!({"thread_metadata": {"invitable": true}}));
        assert_eq!(Settings::of(&public).invitable, None, "only a private thread has the choice");
    }

    #[test]
    fn a_name_of_nothing_is_refused() {
        let thread = channel(json!({}));
        let mut settings = Settings::of(&thread);
        settings.name = " \r\n ".into();
        assert_eq!(settings_body(&thread, &settings), Err(Invalid::NameRequired));
        assert_eq!(Invalid::NameRequired.words(), "Thread Name is required");
    }

    #[test]
    fn durations_are_worded_by_their_largest_part() {
        let words: Vec<_> = SLOWMODES.iter().map(|&s| duration_label(s, "Off")).collect();
        assert_eq!(words[0], "Off");
        assert_eq!(&words[1..6], ["5 seconds", "10 seconds", "15 seconds", "30 seconds", "1 minute"]);
        assert_eq!(&words[11..], ["1 hour", "2 hours", "6 hours"]);
        let archive: Vec<_> = ARCHIVE_AFTER.iter().map(|&m| archive_label(m)).collect();
        assert_eq!(archive, ["1 Hour", "24 Hours", "3 Days", "1 Week"], "the form's own words for these");
        assert_eq!(archive_label(2880), "2 days", "any other is worded as a duration");
    }

    #[test]
    fn closing_locking_and_reopening_send_the_clients_edits() {
        assert_eq!(close_edit(true), json!({"archived": true}));
        assert_eq!(close_edit(false), json!({"archived": false}));
        assert_eq!(lock_edits(false, true), [json!({"locked": true, "archived": false})]);
        assert_eq!(lock_edits(false, false), [json!({"locked": false, "archived": false})]);
        assert_eq!(
            lock_edits(true, true),
            [json!({"archived": false}), json!({"locked": true, "archived": true})],
            "an archived thread is reopened to be locked, and archived again"
        );
        assert_eq!(
            lock_edits(true, false),
            [json!({"archived": false, "locked": false}), json!({"locked": false, "archived": true})]
        );
    }

    #[test]
    fn a_refused_reopen_says_why() {
        assert_eq!(
            reopen_alert(&refused(400, r#"{"code": 160006}"#), true),
            (
                "Cannot reopen post",
                "This server has reached the maximum number of active posts. Free up space by archiving a post."
            )
        );
        assert_eq!(reopen_alert(&refused(400, r#"{"code": 160006}"#), false).0, "Cannot reopen thread");
        assert_eq!(reopen_alert(&refused(400, r#"{"code": 160007}"#), true).0, "Cannot reopen thread");
        assert_eq!(reopen_alert(&refused(429, "{}"), false), ("Cannot reopen thread", "You are being rate limited."));
        assert_eq!(reopen_alert(&refused(403, "{}"), true), ("Cannot reopen post", "This post is archived."));
        assert_eq!(reopen_alert(&refused(500, "{}"), true), ("Error", "An error occurred, please try again."));
    }

    #[test]
    fn a_thread_never_opened_starts_at_its_start_once() {
        const T: Snowflake = Snowflake(7);
        let mut viewed = Viewed::default();
        assert_eq!(viewed.anchor(T, false, None, false, 1_000), Anchor::Start);
        assert_eq!(viewed.anchor(T, false, None, false, 2_000), Anchor::Newest, "opened since");
        assert_eq!(Viewed::default().anchor(T, true, None, false, 1_000), Anchor::Newest, "opened on another client");
    }

    #[test]
    fn a_thread_with_unread_messages_opens_where_we_left_off() {
        const T: Snowflake = Snowflake(7);
        let mut viewed = Viewed::new([(T, 1)]);
        assert_eq!(viewed.anchor(T, true, Some(Snowflake(40)), false, 5), Anchor::Unread(Snowflake(40)));
        assert_eq!(viewed.anchor(T, true, Some(Snowflake(40)), true, 5), Anchor::Newest, "already showing messages");
        assert_eq!(viewed.anchor(T, true, None, false, 5), Anchor::Newest, "nothing unread");
    }

    #[test]
    fn what_was_opened_long_ago_is_forgotten() {
        let mut viewed = Viewed::new([(Snowflake(1), 0), (Snowflake(2), 100 * 24 * 60 * 60 * 1000)]);
        viewed.anchor(Snowflake(9), false, None, false, 100 * 24 * 60 * 60 * 1000);
        let mut kept: Vec<_> = viewed.entries().map(|(id, _)| id.0).collect();
        kept.sort();
        assert_eq!(kept, [2, 9], "the one from 100 days before went");
    }

    const HOUR: i64 = 3_600_000;
    /// 2026-10-28T00:00:00Z.
    const NOW: i64 = 1_793_145_600_000;

    /// A snowflake made at `ms`.
    fn at(ms: i64) -> Snowflake {
        Snowflake(((ms as u64) - 1_420_070_400_000) << 22)
    }

    fn iso(ms: i64) -> String {
        glib::DateTime::from_unix_utc(ms / 1000).unwrap().format_iso8601().unwrap().to_string()
    }

    fn guild(threads: serde_json::Value) -> Guild {
        let mut guild: Guild = serde_json::from_value(json!({
            "id": "1", "name": "G", "joined_at": iso(NOW - 1000 * HOUR),
            "channels": [{"id": "10", "type": 0}, {"id": "11", "type": 0}, {"id": "15", "type": 15}],
            "threads": threads,
        }))
        .unwrap();
        guild.joined_at = Some((NOW - 1000 * HOUR) / 1000);
        guild
    }

    /// A thread under `parent`, made a day ago and joined then, whose newest
    /// message came `quiet` ago and which archives itself after an hour.
    fn thread(id: u64, parent: u64, quiet: i64) -> serde_json::Value {
        json!({
            "id": at(NOW - 24 * HOUR + id as i64).to_string(), "type": 11, "parent_id": parent.to_string(),
            "last_message_id": at(NOW - quiet).to_string(),
            "thread_metadata": {"archived": false, "locked": false, "auto_archive_duration": 60,
                                "archive_timestamp": iso(NOW - 24 * HOUR)},
            "member": {"flags": 0, "muted": false, "mute_config": null, "join_timestamp": iso(NOW - 24 * HOUR + id as i64 * 1000)},
        })
    }

    fn ids(threads: &[&Channel]) -> Vec<Snowflake> {
        threads.iter().map(|t| t.id).collect()
    }

    /// Each thread read up to its newest message, or as `upto` says.
    fn read_upto(guild: &Guild, upto: &[(usize, i64)]) -> ReadStates {
        let mut read = ReadStates::default();
        for (i, t) in guild.threads.iter().enumerate() {
            let acked = upto.iter().find(|(j, _)| *j == i).map_or(t.last_message_id.unwrap(), |&(_, ms)| at(ms));
            read.ack(t.id, acked);
        }
        read
    }

    fn read_all(guild: &Guild) -> ReadStates {
        read_upto(guild, &[])
    }

    #[test]
    fn a_forums_unread_posts_are_those_newer_than_its_own_mark() {
        let g =
            guild(json!([thread(1, 15, HOUR), thread(2, 15, 2 * HOUR), thread(3, 15, 3 * HOUR), thread(4, 10, HOUR)]));
        let forum = g.channel(Snowflake(15)).unwrap();
        // The first two posts are unread; the third and the thread in another channel are read.
        let mut read = read_upto(&g, &[(0, NOW - 4 * HOUR), (1, NOW - 4 * HOUR)]);
        assert_eq!(forum_unread_posts(&g, forum, &read), 0, "never acked");
        read.ack(forum.id, at(NOW - 90 * 60_000));
        assert_eq!(forum_unread_posts(&g, forum, &read), 1, "only the first has a message after the forum's mark");
        let first = &g.threads[0];
        read.ack(first.id, first.last_message_id.unwrap());
        assert_eq!(forum_unread_posts(&g, forum, &read), 0, "a post we read ourselves");
        read.ack(first.id, at(NOW - 4 * HOUR));
        read.mark_unread(first.id, at(NOW - 4 * HOUR), 0);
        read.ack(forum.id, at(NOW));
        assert_eq!(forum_unread_posts(&g, forum, &read), 0, "marked read");
    }

    #[test]
    fn lists_recent_joined_threads_newest_joined_first() {
        let mut stale = thread(4, 10, 2 * HOUR);
        stale["member"] = serde_json::Value::Null;
        let g = guild(json!([
            thread(1, 10, HOUR / 2),
            thread(2, 10, HOUR / 4),
            thread(3, 10, 2 * HOUR),
            stale,
            thread(5, 11, 0)
        ]));
        let read = read_all(&g);
        let general = g.channel(Snowflake(10)).unwrap();
        let listed = listed(&g, general, None, false, &read, NOW);
        assert_eq!(ids(&listed), [g.threads[1].id, g.threads[0].id], "quiet for two hours: out; not joined: never");
        assert_eq!(next_expiry(&g, NOW), Some(NOW + HOUR / 2), "the first to go quiet for an hour");
    }

    #[test]
    fn unread_mentioned_and_pinned_threads_stay() {
        let mut pinned = thread(3, 10, 5 * HOUR);
        pinned["flags"] = json!(2);
        let g = guild(json!([thread(1, 10, 2 * HOUR), thread(2, 10, 3 * HOUR), pinned, thread(4, 10, 6 * HOUR)]));
        // The first has a message we haven't read, the second mentions us.
        let mut read = read_upto(&g, &[(0, NOW - 3 * HOUR)]);
        read.on_message(g.threads[1].id, crate::read_state::Mention::High);
        let listed = listed(&g, g.channel(Snowflake(10)).unwrap(), None, false, &read, NOW);
        assert_eq!(ids(&listed), [g.threads[2].id, g.threads[1].id, g.threads[0].id]);
    }

    #[test]
    fn muted_parents_list_only_unread_threads() {
        let g = guild(json!([thread(1, 10, 0), thread(2, 10, HOUR / 2)]));
        let read = read_upto(&g, &[(1, NOW - HOUR)]);
        let general = g.channel(Snowflake(10)).unwrap();
        assert_eq!(ids(&listed(&g, general, None, true, &read, NOW)), [g.threads[1].id]);
        let open = Some(g.threads[0].id);
        assert_eq!(listed(&g, general, open, true, &read, NOW).len(), 2, "one is open: all relevant ones");
    }

    #[test]
    fn the_open_thread_comes_first() {
        let mut archived = thread(2, 10, 0);
        archived["thread_metadata"]["archived"] = json!(true);
        let g = guild(json!([thread(1, 10, 0), archived]));
        let read = read_all(&g);
        let general = g.channel(Snowflake(10)).unwrap();
        assert_eq!(ids(&listed(&g, general, None, false, &read, NOW)), [g.threads[0].id], "archived: not listed");
        let open = Some(g.threads[1].id);
        assert_eq!(ids(&listed(&g, general, open, false, &read, NOW)), [g.threads[1].id, g.threads[0].id]);
        let other = g.channel(Snowflake(11)).unwrap();
        assert!(listed(&g, other, open, false, &read, NOW).is_empty(), "not under another channel");
    }

    #[test]
    fn unread_falls_back_to_when_we_joined() {
        let g = guild(json!([thread(1, 10, 0)]));
        let read = ReadStates::default();
        let t = &g.threads[0];
        assert!(is_unread(&g, t, &read), "a message since we joined, and nothing read");
        // Never archived: from when we joined, less five seconds.
        let mut early = g.clone();
        early.threads[0].thread_metadata.as_mut().unwrap().archive_timestamp = None;
        let joined = early.threads[0].member.as_ref().unwrap().join_timestamp.unwrap();
        early.threads[0].last_message_id = Some(at(joined - 2000));
        assert!(is_unread(&early, &early.threads[0], &read), "five seconds' grace");
        early.threads[0].last_message_id = Some(at(joined - 6000));
        assert!(!is_unread(&early, &early.threads[0], &read));
        let mut joined_late = g.clone();
        joined_late.joined_at = Some(NOW / 1000 + 1);
        assert!(!is_unread(&joined_late, &joined_late.threads[0], &read), "never before we joined the server");
        let mut post = joined_late.clone();
        post.threads[0].parent_id = Some(Snowflake(15));
        assert!(is_unread(&post, &post.threads[0], &read), "a forum post doesn't go by when we joined the server");
    }

    /// An archived thread under `parent`, last active `quiet` ago.
    fn archived(id: u64, parent: u64, quiet: i64) -> serde_json::Value {
        let mut t = thread(id, parent, quiet);
        t["thread_metadata"]["archived"] = json!(true);
        t["member"] = serde_json::Value::Null;
        t
    }

    #[test]
    fn the_browser_splits_active_threads_by_membership() {
        let mut stranger = thread(2, 10, HOUR / 4);
        stranger["member"] = serde_json::Value::Null;
        let mut old_stranger = thread(3, 10, 5 * HOUR);
        old_stranger["member"] = serde_json::Value::Null;
        let g = guild(json!([thread(1, 10, HOUR / 2), stranger, old_stranger, thread(4, 11, 0), archived(5, 10, 0)]));
        let browser = browser(&g, Snowflake(10), &Archive::default(), |_| true);
        assert_eq!(ids(&browser.joined), [g.threads[0].id]);
        assert_eq!(ids(&browser.others), [g.threads[1].id, g.threads[2].id], "newest activity first, however old");
        assert!(browser.archived.is_empty(), "until a page lands");
        let hidden = super::browser(&g, Snowflake(10), &Archive::default(), |t| t.id != g.threads[1].id);
        assert_eq!(ids(&hidden.others), [g.threads[2].id], "only what we may view");
    }

    #[test]
    fn archived_threads_page_in_by_offset() {
        let mut archive = Archive::default();
        assert_eq!(archive.first(), Some(0));
        assert_eq!((archive.first(), archive.more()), (None, None), "it is on its way");
        archive.landed(0, true, None);
        assert_eq!(archive.first(), None, "the first page is asked for once, even when the browser reopens");
        assert_eq!(archive.more(), Some(PAGE));
        assert_eq!(archive.more(), None, "one at a time");
        archive.failed();
        assert_eq!(archive.more(), None, "a failure stops the paging");
        archive.landed(0, false, None);
        assert_eq!(archive.more(), None, "nothing left");
    }

    #[test]
    fn archived_threads_end_where_the_pages_do() {
        let g = guild(json!([
            archived(1, 10, 3 * HOUR),
            archived(2, 10, 2 * HOUR),
            archived(3, 10, HOUR),
            archived(4, 11, 0)
        ]));
        let mut archive = Archive::default();
        archive.first();
        // Two pages' worth are known from elsewhere, one page is in.
        archive.landed(0, true, Some(g.threads[1].id));
        let browser = browser(&g, Snowflake(10), &archive, |_| true);
        assert_eq!(ids(&browser.archived), [g.threads[2].id, g.threads[1].id], "not the older one the next page holds");
        archive.more();
        archive.landed(PAGE, false, Some(g.threads[0].id));
        let browser = super::browser(&g, Snowflake(10), &archive, |_| true);
        assert_eq!(browser.archived.len(), 3, "the last page: all of them");
        let mut media = g.clone();
        media.threads[2].kind = ChannelKind::MediaThread;
        let browser = super::browser(&media, Snowflake(10), &archive, |_| true);
        assert_eq!(browser.archived.len(), 2, "a media post is left out");
    }

    #[test]
    fn an_archived_page_has_threads_and_our_membership_of_some() {
        let page = ArchivedPage::from_value(&json!({
            "threads": [archived(1, 10, HOUR), {"id": "9", "type": 0}],
            "members": [{"id": "5", "user_id": "3", "flags": 2, "join_timestamp": iso(NOW)}],
            "has_more": true,
            "first_messages": [],
        }))
        .unwrap();
        assert_eq!(page.threads.len(), 1, "a channel that isn't a thread is left out");
        assert_eq!(page.members.iter().map(|(id, m)| (id.0, m.flags)).collect::<Vec<_>>(), [(5, 2)]);
        assert!(page.has_more);
        assert_eq!(page.first_messages.as_ref().map(Vec::len), Some(0));
        let last = ArchivedPage::from_value(&json!({"threads": []})).unwrap();
        assert!(!last.has_more && last.members.is_empty());
        assert!(ArchivedPage::from_value(&json!({"threads": null})).is_none(), "the client treats it as a failure");
    }

    #[test]
    fn archived_threads_are_never_acked() {
        let mut g = guild(json!([thread(1, 10, 0)]));
        assert!(is_ackable(&g.threads[0]));
        g.threads[0].thread_metadata.as_mut().unwrap().archived = true;
        assert!(!is_ackable(&g.threads[0]));
    }
    mod redraw {
        use super::super::*;

        const OPEN: Snowflake = Snowflake(20);
        const PUBLIC: Snowflake = Snowflake(21);
        const PRIVATE: Snowflake = Snowflake(22);
        const ALL: [Snowflake; 3] = [OPEN, PUBLIC, PRIVATE];

        fn me() -> Account {
            Account { id: Snowflake(5), ..Account::default() }
        }

        /// A server whose `@everyone` views, sends and reads; one text channel and three threads under it.
        fn guild() -> Guild {
            let everyone = (Permissions::VIEW_CHANNEL
                | Permissions::SEND_MESSAGES
                | Permissions::READ_MESSAGE_HISTORY
                | Permissions::SEND_MESSAGES_IN_THREADS)
                .0
                .to_string();
            let thread = |id: u64, kind: u8| {
                json!({"id": id.to_string(), "type": kind, "name": "plans", "parent_id": "2", "flags": 0,
                       "message_count": 3, "thread_metadata": {"archived": false, "locked": false}})
            };
            serde_json::from_value(json!({
                "id": "1", "name": "G", "owner_id": "9",
                "roles": [{"id": "1", "name": "@everyone", "permissions": everyone}],
                "channels": [{"id": "2", "type": 0}],
                "threads": [thread(20, 11), thread(21, 11), thread(22, 12)],
            }))
            .unwrap()
        }

        fn looks_of(guild: &Guild) -> Vec<Option<Look>> {
            looks(guild, &ALL, &me(), 0)
        }

        fn change(guild: &mut Guild, id: Snowflake, edit: impl FnOnce(&mut Channel)) {
            edit(guild.threads.iter_mut().find(|t| t.id == id).unwrap());
        }

        fn after(change: impl FnOnce(&mut Guild)) -> Redraw {
            let mut guild = guild();
            let before = looks_of(&guild);
            change(&mut guild);
            redraw(&before, &looks_of(&guild), &ALL, Some(OPEN))
        }

        fn member() -> Option<ThreadMember> {
            let member = json!({"id": "21", "user_id": "5", "join_timestamp": "2026-01-01T00:00:00+00:00", "flags": 0});
            Some(serde_json::from_value(member).expect("a thread member"))
        }

        #[test]
        fn a_sync_of_what_we_hold_draws_nothing() {
            assert_eq!(after(|_| {}), Redraw::Nothing);
        }

        #[test]
        fn joining_a_public_thread_draws_nothing() {
            let joined = after(|guild| guild.set_thread_member(PUBLIC, member()));
            assert_eq!(joined, Redraw::Nothing, "it was ours to see before");
        }

        #[test]
        fn joining_a_private_thread_changes_what_we_may_see() {
            let joined = after(|guild| guild.set_thread_member(PRIVATE, member()));
            assert_eq!(joined, Redraw::Everything, "it appears in the names a message can show");
        }

        #[test]
        fn a_thread_that_only_counts_differently_draws_its_starter() {
            let counted = after(|guild| change(guild, PUBLIC, |t| t.message_count = Some(4)));
            assert_eq!(counted, Redraw::Starters(vec![PUBLIC]));
            let archived =
                after(|guild| change(guild, PUBLIC, |t| t.thread_metadata.as_mut().unwrap().archived = true));
            assert_eq!(archived, Redraw::Starters(vec![PUBLIC]));
        }

        #[test]
        fn the_open_thread_changing_draws_everything() {
            let locked = after(|guild| change(guild, OPEN, |t| t.thread_metadata.as_mut().unwrap().locked = true));
            assert_eq!(locked, Redraw::Everything);
        }

        #[test]
        fn a_new_name_or_a_new_thread_draws_everything() {
            let renamed = after(|guild| change(guild, PUBLIC, |t| t.name = Some("lunch".into())));
            assert_eq!(renamed, Redraw::Everything);
            let mut guild = guild();
            let before = looks_of(&guild);
            guild.threads.retain(|t| t.id != PUBLIC);
            assert_eq!(redraw(&before, &looks_of(&guild), &ALL, Some(OPEN)), Redraw::Everything);
        }
    }
}
