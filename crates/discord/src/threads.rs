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

use crate::read_state::ReadStates;
use crate::{Channel, ChannelKind, Guild, Snowflake, ThreadMember};

/// What the client sends as `location` when a thread is joined or left from
/// a context menu (`joinThread` and `leaveThread` in 7909, called from the
/// thread menu, 949488).
pub const MENU_LOCATION: &str = "Context Menu";

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
        read.on_message(g.threads[1].id, true);
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
}
