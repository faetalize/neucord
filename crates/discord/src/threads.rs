//! Threads: which ones the sidebar lists under their parent, when one is
//! unread, and when the client acks one. Who may post in a thread is in
//! [`crate::permissions`], which thread messages notify in
//! [`crate::notifications`].
//!
//! Only threads we've joined show, as in the official client: those that
//! are still active (not archived), and of those only the "relevant" ones.

use crate::read_state::ReadStates;
use crate::{Channel, ChannelKind, Guild, Snowflake};

/// The threads the sidebar lists under `parent`, as the client lists them
/// (`eC` in 297469, from `ActiveJoinedThreadsStore` in 863005): active
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
/// client's rule for threads (`getAckTimestamp` in 573163): without a read
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
/// 573163): not in an archived thread.
pub fn is_ackable(thread: &Channel) -> bool {
    !thread.is_archived()
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

    #[test]
    fn archived_threads_are_never_acked() {
        let mut g = guild(json!([thread(1, 10, 0)]));
        assert!(is_ackable(&g.threads[0]));
        g.threads[0].thread_metadata.as_mut().unwrap().archived = true;
        assert!(!is_ackable(&g.threads[0]));
    }
}
