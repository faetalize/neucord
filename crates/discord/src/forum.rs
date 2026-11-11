//! Forum and media channels' post lists: which posts a list holds, in what
//! order, and when the official client asks Discord for more.
//!
//! A post is a thread. The active ones arrive with `THREAD_LIST_SYNC` once
//! we're subscribed to a server's threads (see [`crate::subscriptions`]), and
//! [`Listing`] keeps the order the client shows them in. The archived ones
//! come a page at a time from `threads/search` (`loadArchivedThreads` in
//! 7909), which [`Archive`] keeps account of as `ArchivedThreadsStore`
//! (45494) does. A card's first message comes with an archived page, or from
//! `post-data` for the others ([`FirstMessages`], 473503), and an unread count
//! from gateway op 28 ([`Unreads`], 474399).

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::threads::{ArchivedPage, PAGE};
use crate::{Channel, ChannelKind, Guild, Message, Snowflake};

/// How many posts one `post-data` request names at most (`getNextBatch`,
/// 473503).
const FIRST_MESSAGES_PER_REQUEST: usize = 10;

/// How many posts a card asks the first messages of at once: its own and the
/// four after it (473503).
const FIRST_MESSAGES_AHEAD: i64 = 5;

/// How many posts one op 28 names at most (435470).
const UNREADS_PER_REQUEST: usize = 180;

/// How many archived lists are kept, the oldest dropped first (45494).
const KEPT_LISTS: usize = 50;

/// A post carries at most this many tags (565846).
pub const MAX_TAGS: usize = 5;

/// How posts are ordered (`ThreadSortOrder`, 602137).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sort {
    LatestActivity,
    CreationDate,
}

/// How posts are laid out (`ForumLayout`, 371444).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    List,
    Grid,
}

/// Whether a post must carry all the tags filtered by or any of them
/// (`ForumTagSetting`, 392421).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagMatch {
    Some,
    All,
}

impl TagMatch {
    /// As the search sends it, and the channel's default holds it.
    pub fn wire(self) -> &'static str {
        match self {
            TagMatch::Some => "match_some",
            TagMatch::All => "match_all",
        }
    }
}

impl Channel {
    /// How the channel's posts are ordered until we choose
    /// (`getDefaultSortOrder`, 95701).
    pub fn default_sort(&self) -> Sort {
        match self.default_sort_order {
            Some(1) => Sort::CreationDate,
            _ => Sort::LatestActivity,
        }
    }

    /// How the channel's posts are laid out until we choose
    /// (`getDefaultLayout`, 95701): a media channel is always a grid.
    pub fn default_layout(&self) -> Layout {
        match (self.kind, self.default_forum_layout) {
            (ChannelKind::Media, _) | (_, Some(2)) => Layout::Grid,
            _ => Layout::List,
        }
    }

    /// A channel where members' reports reach moderators, whose archived
    /// posts the list leaves out.
    pub fn is_moderator_report(&self) -> bool {
        self.flags & (1 << 19) != 0
    }

    /// A forum that keeps its posts' media from being saved, copied or
    /// opened in a browser (`HIDE_MEDIA_DOWNLOAD_OPTIONS`; the client reads
    /// it off a post's parent in 207133).
    pub fn hides_media_options(&self) -> bool {
        self.flags & (1 << 15) != 0
    }

    /// How the channel's tag filter matches until we choose
    /// (`getDefaultTagSetting`, 95701).
    pub fn default_tag_match(&self) -> TagMatch {
        match self.default_tag_setting.as_deref() {
            Some("match_all") => TagMatch::All,
            _ => TagMatch::Some,
        }
    }
}

/// How a forum's posts are shown: the client's `ForumChannelStore` (422844),
/// which starts from the channel's defaults and keeps what we choose for as
/// long as the app runs.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub sort: Sort,
    pub layout: Layout,
    /// The tags filtered by, in the order they were picked: the search names
    /// them so.
    pub tags: Vec<Snowflake>,
    pub tag_match: TagMatch,
}

impl View {
    pub fn new(forum: &Channel) -> View {
        View {
            sort: forum.default_sort(),
            layout: forum.default_layout(),
            tags: Vec::new(),
            tag_match: forum.default_tag_match(),
        }
    }

    /// Filter by `tag`, or no longer (`toggleTagFilter`).
    pub fn toggle_tag(&mut self, tag: Snowflake) {
        match self.tags.iter().position(|&t| t == tag) {
            Some(at) => {
                self.tags.remove(at);
            }
            None => self.tags.push(tag),
        }
    }

    /// Reset to default: the sort, the layout and the matching, not the tags.
    pub fn reset(&mut self, forum: &Channel) {
        let fresh = View::new(forum);
        (self.sort, self.layout, self.tag_match) = (fresh.sort, fresh.layout, fresh.tag_match);
    }

    /// Whether `post` passes the tag filter (`x` in 207777, `m` in 45494).
    /// Without one, everything does.
    pub fn matches(&self, post: &Channel) -> bool {
        if self.tags.is_empty() {
            return true;
        }
        let has = |tag: &Snowflake| post.applied_tags.contains(tag);
        match self.tag_match {
            TagMatch::Some => self.tags.iter().any(has),
            TagMatch::All => self.tags.iter().all(has),
        }
    }
}

/// The key `sort` orders by in an archived list, where a post with no last
/// message has none (`p` in 45494).
fn archived_key(post: &Channel, sort: Sort) -> Option<Snowflake> {
    match sort {
        Sort::LatestActivity => post.last_message_id,
        Sort::CreationDate => Some(post.id),
    }
}

/// Pinned first and then newest first (`U` in 207777).
fn order(posts: &mut [&Channel], sort: Sort) {
    posts.sort_by_key(|p| {
        let key = match sort {
            Sort::LatestActivity => p.activity(),
            Sort::CreationDate => p.id,
        };
        (!p.is_pinned(), std::cmp::Reverse(key))
    });
}

/// The posts of `forum` that aren't archived.
fn active(guild: &Guild, forum: Snowflake) -> impl Iterator<Item = &Channel> {
    guild.threads.iter().filter(move |t| t.parent_id == Some(forum) && t.is_active_thread())
}

/// The active posts of `forum`, in the order a card looks ahead through
/// (`S` in 207777): the tag filter doesn't apply.
pub fn sorted_posts(guild: &Guild, forum: Snowflake, sort: Sort) -> Vec<Snowflake> {
    let mut posts: Vec<&Channel> = active(guild, forum).collect();
    order(&mut posts, sort);
    posts.into_iter().map(|p| p.id).collect()
}

/// The list of active posts as `ForumActivePostStore` (207777) keeps it. It
/// holds the posts there were when the forum was opened or last sorted, and
/// shows new ones only when asked: they're counted until then.
#[derive(Debug, Default)]
pub struct Listing {
    forum: Option<Snowflake>,
    /// Every active post, in no order.
    posts: Vec<Snowflake>,
    /// What shows, in order.
    shown: Vec<Snowflake>,
    new_posts: u32,
    /// Opening the forum reads it, until it has been.
    can_ack: bool,
}

impl Listing {
    /// The forum the list is of, if any.
    pub fn forum(&self) -> Option<Snowflake> {
        self.forum
    }

    /// The posts shown, in order.
    pub fn shown(&self) -> &[Snowflake] {
        &self.shown
    }

    /// How many posts others made since the list was last made again.
    pub fn new_posts(&self) -> u32 {
        self.new_posts
    }

    /// Whether looking at the forum reads it, which only the first look
    /// does (`getCanAckThreads`).
    pub fn can_ack(&self) -> bool {
        self.can_ack
    }

    /// The forum was read.
    pub fn acked(&mut self, forum: Snowflake) {
        if self.forum == Some(forum) {
            self.can_ack = false;
        }
    }

    /// Open `forum`, or open it again, or the server's threads came or
    /// changed: start from the posts there are now.
    pub fn refresh(&mut self, guild: &Guild, forum: Snowflake, view: &View) {
        self.forum = Some(forum);
        self.posts = active(guild, forum).map(|t| t.id).collect();
        self.new_posts = 0;
        self.can_ack = true;
        self.arrange(guild, view);
    }

    /// Order and filter what the list holds again, as the client does when
    /// the sort or the tags change.
    pub fn arrange(&mut self, guild: &Guild, view: &View) {
        let Some(forum) = self.forum else { return };
        let mut posts: Vec<&Channel> = self.posts.iter().filter_map(|&id| guild.thread(id)).collect();
        order(&mut posts, view.sort);
        self.shown =
            posts.into_iter().filter(|p| p.parent_id == Some(forum) && view.matches(p)).map(|p| p.id).collect();
    }

    /// A post was made. Ours isn't counted; someone else's is, and shows
    /// when asked for.
    pub fn created(&mut self, post: &Channel, newly_created: bool, mine: bool) {
        if newly_created && post.parent_id == self.forum && !mine {
            self.new_posts += 1;
        }
    }

    /// A post changed. A pinned one joins the list at once and the posts are
    /// ordered again, which is how a pin moves a post to the top.
    pub fn updated(&mut self, guild: &Guild, post: &Channel, view: &View) {
        if post.parent_id != self.forum || !post.is_pinned() {
            return;
        }
        if !self.posts.contains(&post.id) {
            self.posts.push(post.id);
        }
        self.arrange(guild, view);
    }

    /// A post was deleted.
    pub fn deleted(&mut self, guild: &Guild, post: &Channel, view: &View) {
        if post.parent_id != self.forum {
            return;
        }
        self.posts.retain(|&p| p != post.id);
        self.arrange(guild, view);
    }
}

/// A search of a forum's archived posts, as the client names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Search {
    pub channel: Snowflake,
    /// What the list is ordered by here. The request is the same for both:
    /// the client sorts what comes back itself.
    pub sort: Sort,
    pub offset: u32,
    pub tags: Vec<Snowflake>,
    pub tag_match: TagMatch,
}

impl Search {
    /// A page of a channel's archived threads that isn't a forum's list:
    /// by last message, and filtered by no tag.
    pub fn plain(channel: Snowflake, offset: u32) -> Search {
        Search { channel, sort: Sort::LatestActivity, offset, tags: Vec::new(), tag_match: TagMatch::Some }
    }

    /// The query in the order 7909 builds it, as superagent writes it:
    /// every value is escaped, so the tags' commas are too, and a missing
    /// one is left out.
    pub fn query(&self) -> String {
        let mut query = format!("archived=true&sort_by=last_message_time&sort_order=desc&limit={PAGE}");
        if !self.tags.is_empty() {
            let tags: Vec<String> = self.tags.iter().map(ToString::to_string).collect();
            query.push_str(&format!("&tag={}", tags.join("%2C")));
        }
        query.push_str(&format!("&tag_setting={}&offset={}", self.tag_match.wire(), self.offset));
        query
    }
}

/// One list of archived posts: for a channel, in a sort, filtered by tags.
#[derive(Debug)]
struct Load {
    channel: Snowflake,
    sort: Sort,
    tags: Vec<Snowflake>,
    tag_match: TagMatch,
    loading: bool,
    initial: bool,
    has_more: bool,
    failed: bool,
    threads: Vec<Snowflake>,
    next_offset: u32,
}

/// What names a list: the tags count in any order (`I` in 45494).
type Key = (Snowflake, Sort, Vec<Snowflake>, TagMatch);

fn key(channel: Snowflake, view: &View) -> Key {
    key_of(channel, view.sort, &view.tags, view.tag_match)
}

fn key_of(channel: Snowflake, sort: Sort, tags: &[Snowflake], tag_match: TagMatch) -> Key {
    let mut tags = tags.to_vec();
    tags.sort();
    (channel, sort, tags, tag_match)
}

impl Search {
    fn key(&self) -> Key {
        key_of(self.channel, self.sort, &self.tags, self.tag_match)
    }
}

/// The archived posts loaded so far, per forum and way of listing them
/// (`ArchivedThreadsStore`, 45494).
#[derive(Debug, Default)]
pub struct Archive {
    /// Least recently used first.
    loads: Vec<(Key, Load)>,
}

impl Archive {
    fn find(&self, key: &Key) -> Option<&Load> {
        self.loads.iter().find(|(k, _)| k == key).map(|(_, l)| l)
    }

    /// Whether the first page of this list hasn't been asked for.
    pub fn is_initial(&self, forum: Snowflake, view: &View) -> bool {
        self.find(&key(forum, view)).is_none_or(|l| l.initial)
    }

    pub fn is_loading(&self, forum: Snowflake, view: &View) -> bool {
        self.find(&key(forum, view)).is_some_and(|l| l.loading)
    }

    /// Whether scrolling to the end asks for another page: there is one, and
    /// the last didn't fail or isn't coming (`getCanLoadMore`).
    pub fn can_load_more(&self, forum: Snowflake, view: &View) -> bool {
        self.find(&key(forum, view)).is_some_and(|l| l.has_more && !l.loading && !l.failed)
    }

    /// The posts loaded, newest first.
    pub fn threads(&self, forum: Snowflake, view: &View) -> &[Snowflake] {
        self.find(&key(forum, view)).map_or(&[], |l| &l.threads)
    }

    /// Ask for the next page of a list (`loadArchivedThreads`): the search
    /// to send, unless the list is being loaded already.
    pub fn begin(&mut self, forum: Snowflake, view: &View) -> Option<Search> {
        let key = key(forum, view);
        if self.find(&key).is_some_and(|l| l.loading) {
            return None;
        }
        // A list that failed is forgotten once another is asked for.
        self.loads.retain(|(k, l)| *k == key || l.channel != forum || !l.failed);
        let mut load = match self.loads.iter().position(|(k, _)| *k == key) {
            Some(at) => {
                let mut load = self.loads.remove(at).1;
                load.tags = view.tags.clone();
                load.failed = false;
                load
            }
            None => Load {
                channel: forum,
                sort: view.sort,
                tags: view.tags.clone(),
                tag_match: view.tag_match,
                loading: false,
                initial: true,
                has_more: false,
                failed: false,
                threads: Vec::new(),
                next_offset: 0,
            },
        };
        load.loading = true;
        load.initial = false;
        let offset = load.next_offset;
        self.loads.push((key, load));
        // Past 50, the oldest that aren't loading go.
        let mut over = self.loads.len().saturating_sub(KEPT_LISTS);
        self.loads.retain(|(_, l)| {
            let drop = over > 0 && !l.loading;
            over -= usize::from(drop);
            !drop
        });
        Some(Search { channel: forum, sort: view.sort, offset, tags: view.tags.clone(), tag_match: view.tag_match })
    }

    /// A page arrived. Its posts are in `guild` already. The list then holds
    /// every archived post of the forum we know that the filter lets
    /// through, as far down as it has loaded.
    pub fn arrived(&mut self, guild: &Guild, search: &Search, page: &ArchivedPage) {
        let Some(load) = self.loads.iter_mut().find(|(k, _)| *k == search.key()).map(|(_, l)| l) else {
            return;
        };
        load.threads.extend(page.threads.iter().map(|t| t.id));
        Self::resort(guild, load);
        load.has_more = page.has_more;
        load.next_offset = search.offset + PAGE;
        load.loading = false;
        load.initial = false;
    }

    /// The page couldn't be loaded.
    pub fn failed(&mut self, search: &Search) {
        if let Some((_, load)) = self.loads.iter_mut().find(|(k, _)| *k == search.key()) {
            load.loading = false;
            load.failed = true;
            load.initial = false;
        }
    }

    /// `m` in 45494. Where there is more to load, the list stops at the
    /// oldest post of it, so what follows isn't skipped over. It judges that
    /// by whether there was more *before* this page, as the client does, so
    /// the first page keeps every archived post we know.
    fn resort(guild: &Guild, load: &mut Load) {
        let everything = !load.has_more;
        let cutoff = load.threads.last().and_then(|&id| guild.thread(id)).and_then(|t| archived_key(t, load.sort));
        let view = View { sort: load.sort, layout: Layout::List, tags: load.tags.clone(), tag_match: load.tag_match };
        let mut posts: Vec<(&Channel, Option<Snowflake>)> = guild
            .threads
            .iter()
            .filter(|t| t.parent_id == Some(load.channel) && t.is_archived())
            .filter(|t| view.matches(t))
            .map(|t| (t, archived_key(t, load.sort)))
            .filter(|(_, key)| match (everything, cutoff) {
                (true, _) | (_, None) => true,
                (false, Some(cutoff)) => key.is_some_and(|key| key >= cutoff),
            })
            .collect();
        posts.sort_by_key(|(_, key)| std::cmp::Reverse(*key));
        load.threads = posts.into_iter().map(|(t, _)| t.id).collect();
    }

    /// Ordering is decided again, after a post changed in a way that moves
    /// it (`RESORT_THREADS`), for one forum or every one.
    pub fn resort_all(&mut self, guild: &Guild, forum: Option<Snowflake>) {
        for (_, load) in self.loads.iter_mut().filter(|(_, l)| forum.is_none_or(|f| l.channel == f)) {
            Self::resort(guild, load);
        }
    }

    /// A pinned post moves to the active ones.
    pub fn pinned(&mut self, post: &Channel) {
        for (_, load) in self.loads.iter_mut().filter(|(_, l)| Some(l.channel) == post.parent_id) {
            load.threads.retain(|&t| t != post.id);
        }
    }

    pub fn deleted(&mut self, post: Snowflake) {
        for (_, load) in &mut self.loads {
            load.threads.retain(|&t| t != post);
        }
    }

    pub fn channel_deleted(&mut self, forum: Snowflake) {
        self.loads.retain(|(_, l)| l.channel != forum);
    }

    /// A new session starts from nothing.
    pub fn clear(&mut self) {
        self.loads.clear();
    }
}

/// A post's first message, once we know whether it has one.
#[derive(Clone, Debug)]
pub struct Slot {
    pub loaded: bool,
    pub message: Option<Message>,
}

/// The first messages the cards show, and the posts that still need asking
/// for (`ForumPostMessagesStore` in 969043 and the requests of 473503).
#[derive(Debug, Default)]
pub struct FirstMessages {
    slots: HashMap<Snowflake, Slot>,
    /// Per forum, the posts to ask `post-data` for, in the order they were
    /// wanted; the forums too.
    wanted: Vec<(Snowflake, Vec<Snowflake>)>,
}

impl FirstMessages {
    pub fn get(&self, post: Snowflake) -> Option<&Slot> {
        self.slots.get(&post)
    }

    fn set(&mut self, post: Snowflake, message: Option<Message>) {
        self.slots.insert(post, Slot { loaded: true, message });
    }

    /// A page of archived posts brought first messages, or none.
    pub fn from_page(&mut self, page: &ArchivedPage) {
        let Some(first) = &page.first_messages else { return };
        for post in &page.threads {
            self.set(post.id, None);
        }
        for message in first {
            self.set(message.channel_id, Some(message.clone()));
        }
    }

    /// What `post-data` answered: each post's first message, or none.
    pub fn from_post_data(&mut self, value: &Value) {
        let Some(posts) = value["threads"].as_object() else { return };
        for (id, post) in posts {
            let Some(id) = id.parse().ok().map(Snowflake) else { continue };
            self.set(id, Message::deserialize(&post["first_message"]).ok());
        }
    }

    /// A post's first message was sent: it has the post's id.
    pub fn created(&mut self, message: &Message) {
        if message.id.0 == message.channel_id.0 {
            self.set(message.channel_id, Some(message.clone()));
        }
    }

    /// The first message was deleted.
    pub fn deleted(&mut self, post: Snowflake, message: Snowflake) {
        if message.0 == post.0 {
            self.set(post, None);
        }
    }

    /// A post made while we're subscribed to the server's threads has no
    /// first message yet: it comes as a message of its own.
    pub fn thread_created(&mut self, post: Snowflake, subscribed: bool) {
        if subscribed && !self.slots.contains_key(&post) {
            self.set(post, None);
        }
    }

    /// Change a first message we hold.
    pub fn change(&mut self, post: Snowflake, message: Snowflake, change: impl FnOnce(&mut Message)) {
        if let Some(Slot { message: Some(first), .. }) = self.slots.get_mut(&post)
            && first.id == message
        {
            change(first);
        }
    }

    /// The people who wrote the first messages we hold.
    pub fn authors(&self) -> Vec<crate::User> {
        self.slots.values().filter_map(|s| s.message.as_ref().map(|m| m.author.clone())).collect()
    }

    /// A new session starts from nothing.
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    fn requested(&self, forum: Snowflake, post: Snowflake) -> bool {
        self.wanted.iter().any(|(f, posts)| *f == forum && posts.contains(&post))
    }

    fn want(&mut self, forum: Snowflake, posts: &[Snowflake]) -> bool {
        let mut any = false;
        for &post in posts {
            if self.slots.get(&post).is_some_and(|s| s.loaded || s.message.is_some()) {
                continue;
            }
            let at = match self.wanted.iter().position(|(f, _)| *f == forum) {
                Some(at) => at,
                None => {
                    self.wanted.push((forum, Vec::new()));
                    self.wanted.len() - 1
                }
            };
            if !self.wanted[at].1.contains(&post) {
                self.wanted[at].1.push(post);
            }
            any = true;
        }
        any
    }

    /// A card for `post` came up and needs its first message
    /// (`useForumPostFirstMessage`, 473503): the client asks for the posts
    /// from there on in the order they're listed, five at a time, rather
    /// than one request each. `sorted` is [`sorted_posts`]. Whether there is
    /// something to ask for now.
    pub fn card(&mut self, forum: Snowflake, post: Snowflake, sorted: &[Snowflake]) -> bool {
        if self.requested(forum, post) {
            return false;
        }
        // JavaScript's `slice(i, i + 5)`, where a post that isn't active
        // finds no place (-1) and the slice starts from the end.
        let at = sorted.iter().position(|&p| p == post).map_or(-1, |i| i as i64);
        let len = sorted.len() as i64;
        let from = if at < 0 { (len + at).max(0) } else { at.min(len) };
        let to = (at + FIRST_MESSAGES_AHEAD).clamp(from, len);
        let ahead: Vec<Snowflake> =
            sorted[from as usize..to as usize].iter().copied().filter(|&p| !self.requested(forum, p)).collect();
        self.want(forum, &ahead)
    }

    /// The next request to send: a forum, and up to ten of its posts.
    pub fn next_request(&self) -> Option<(Snowflake, Vec<Snowflake>)> {
        let (forum, posts) = self.wanted.first()?;
        Some((*forum, posts.iter().take(FIRST_MESSAGES_PER_REQUEST).copied().collect()))
    }

    /// The request for `posts` is over, answered or not.
    pub fn finished(&mut self, forum: Snowflake, posts: &[Snowflake]) {
        if let Some(at) = self.wanted.iter().position(|(f, _)| *f == forum) {
            self.wanted[at].1.retain(|p| !posts.contains(p));
            if self.wanted[at].1.is_empty() {
                self.wanted.remove(at);
            }
        }
    }
}

/// How many messages of each post we haven't read, as Discord counts them
/// (`ForumPostUnreadCountStore`, 474399).
#[derive(Debug, Default)]
pub struct Unreads {
    counts: HashMap<Snowflake, u32>,
    asked: HashSet<Snowflake>,
}

impl Unreads {
    pub fn count(&self, post: Snowflake) -> Option<u32> {
        self.counts.get(&post).copied()
    }

    /// The posts of `posts` we neither know the count of nor asked for
    /// (`getThreadIdsMissingCounts`).
    pub fn missing(&self, posts: impl Iterator<Item = Snowflake>) -> Vec<Snowflake> {
        posts.filter(|p| !self.counts.contains_key(p) && !self.asked.contains(p)).collect()
    }

    /// Op 28 was sent for these.
    pub fn asked(&mut self, posts: &[Snowflake]) {
        self.asked.extend(posts);
    }

    /// What op 28 answered (`FORUM_UNREADS`).
    pub fn answered(&mut self, counts: &[(Snowflake, Option<u32>)]) {
        for &(post, count) in counts {
            if let Some(count) = count {
                self.counts.insert(post, count);
            }
        }
    }

    /// A message arrived in a post we count (not one we're sending, which
    /// the client leaves out too).
    pub fn message(&mut self, post: Snowflake) {
        if let Some(count) = self.counts.get_mut(&post) {
            *count += 1;
        }
    }

    /// A post began, with nothing in it unread.
    pub fn created(&mut self, post: Snowflake) {
        self.counts.insert(post, 0);
    }

    /// A post of a forum was read.
    pub fn read(&mut self, post: Snowflake) {
        self.counts.insert(post, 0);
    }

    /// A new session starts from nothing.
    pub fn clear(&mut self) {
        self.counts.clear();
        self.asked.clear();
    }
}

/// When a post was last active, in milliseconds since 1970: when its last
/// message was sent, or when it began (`JO` in 707539).
pub fn last_activity_ms(post: &Channel) -> i64 {
    post.activity().unix_ms()
}

/// How long ago `then_ms` was, as a card says it.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let minutes = (now_ms - then_ms).max(0) / 60_000;
    match minutes {
        0 => "just now".to_owned(),
        1..=59 => format!("{minutes}m ago"),
        60..=1439 => format!("{}h ago", minutes / 60),
        1440..=43_199 => format!("{}d ago", minutes / 1440),
        _ => ">30d ago".to_owned(),
    }
}

/// How many messages a post has, as a card says it (`Mw` in 321045). Posts
/// made before 2022-06-01 stop counting at 50.
pub fn message_count_text(count: u32, post: Snowflake) -> String {
    if post.0 <= 992_549_565_104_128_000 && count >= 50 {
        "50+".to_owned()
    } else if count >= 100_000 {
        "100k+".to_owned()
    } else {
        count.to_string()
    }
}

/// How many unread messages a card says it has (`k6` in 435470): 25 and up
/// are "25+", and what isn't known yet is "1+".
pub fn unread_text(unread: Option<u32>, messages: u32) -> String {
    match unread.filter(|&n| n > 0) {
        None => "1+".to_owned(),
        Some(n) => match n.min(messages) {
            25.. => "25+".to_owned(),
            n => n.to_string(),
        },
    }
}

/// Op 28's payload, in the field order the client sends it (868).
#[derive(Debug, serde::Serialize)]
pub struct UnreadRequest {
    guild_id: Snowflake,
    channel_id: Snowflake,
    threads: Vec<UnreadThread>,
}

#[derive(Debug, serde::Serialize)]
struct UnreadThread {
    thread_id: Snowflake,
    ack_message_id: Snowflake,
}

impl UnreadRequest {
    /// Ask for the counts of `posts`, each with the message we last read in
    /// it: of the first 180, none if there are none.
    pub fn new(guild: Snowflake, forum: Snowflake, posts: &[(Snowflake, Snowflake)]) -> Option<Self> {
        let threads: Vec<_> = posts
            .iter()
            .take(UNREADS_PER_REQUEST)
            .map(|&(thread_id, ack_message_id)| UnreadThread { thread_id, ack_message_id })
            .collect();
        (!threads.is_empty()).then_some(UnreadRequest { guild_id: guild, channel_id: forum, threads })
    }

    /// The posts the request names.
    pub fn posts(&self) -> Vec<Snowflake> {
        self.threads.iter().map(|t| t.thread_id).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const FORUM: Snowflake = Snowflake(15);

    fn post(id: u64, last: Option<u64>, archived: bool, tags: &[u64], pinned: bool) -> Value {
        json!({"id": id.to_string(), "type": 11, "parent_id": "15",
               "last_message_id": last.map(|l| l.to_string()),
               "flags": if pinned { 2 } else { 0 },
               "applied_tags": tags.iter().map(ToString::to_string).collect::<Vec<_>>(),
               "thread_metadata": {"archived": archived, "locked": false, "auto_archive_duration": 1440}})
    }

    fn guild(posts: Vec<Value>) -> Guild {
        serde_json::from_value(
            json!({"id": "1", "name": "G", "channels": [{"id": "15", "type": 15}], "threads": posts}),
        )
        .unwrap()
    }

    fn channel(value: Value) -> Channel {
        serde_json::from_value(value).unwrap()
    }

    fn ids(posts: &[Snowflake]) -> Vec<u64> {
        posts.iter().map(|p| p.0).collect()
    }

    fn view(forum: Value) -> View {
        View::new(&channel(forum))
    }

    #[test]
    fn a_channel_says_how_its_posts_start_out() {
        let forum = |extra: Value| {
            let mut c = json!({"id": "15", "type": 15});
            c.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            channel(c)
        };
        let plain = forum(json!({}));
        assert_eq!(
            (plain.default_sort(), plain.default_layout(), plain.default_tag_match()),
            (Sort::LatestActivity, Layout::List, TagMatch::Some)
        );
        let chosen =
            forum(json!({"default_sort_order": 1, "default_forum_layout": 2, "default_tag_setting": "match_all"}));
        assert_eq!(
            (chosen.default_sort(), chosen.default_layout(), chosen.default_tag_match()),
            (Sort::CreationDate, Layout::Grid, TagMatch::All)
        );
        // 0 is "not set" for the layout.
        assert_eq!(forum(json!({"default_forum_layout": 0})).default_layout(), Layout::List);
        let media = channel(json!({"id": "16", "type": 16, "default_forum_layout": 1}));
        assert_eq!(media.default_layout(), Layout::Grid, "a media channel is always a grid");
    }

    #[test]
    fn tags_with_no_emoji_id_are_read_whatever_discord_sends() {
        let forum =
            channel(json!({"id": "15", "type": 15, "default_reaction_emoji": {"emoji_id": null, "emoji_name": "x"},
            "available_tags": [{"id": "1", "name": "a", "emoji_id": null, "emoji_name": null, "moderated": null},
                               {"id": "2", "name": "b", "emoji_id": "0", "emoji_name": "y", "moderated": true},
                               {"id": "3", "name": "c", "emoji_id": "77", "emoji_name": "z"}]}));
        let tags: Vec<_> = forum.available_tags.iter().map(|t| (t.emoji_id, t.moderated)).collect();
        assert_eq!(tags, vec![(None, false), (None, true), (Some(Snowflake(77)), false)]);
        assert_eq!(forum.default_reaction_emoji.unwrap().emoji_id, None);
    }

    #[test]
    fn a_tag_filter_keeps_posts_with_some_or_all_of_the_tags() {
        let mut view = view(json!({"id": "15", "type": 15}));
        let (a, b, none) = (
            channel(post(1, None, false, &[7], false)),
            channel(post(2, None, false, &[7, 8], false)),
            channel(post(3, None, false, &[], false)),
        );
        assert!([&a, &b, &none].iter().all(|p| view.matches(p)), "no filter, no filtering");
        view.toggle_tag(Snowflake(7));
        view.toggle_tag(Snowflake(8));
        assert_eq!([&a, &b, &none].map(|p| view.matches(p)), [true, true, false], "some");
        view.tag_match = TagMatch::All;
        assert_eq!([&a, &b, &none].map(|p| view.matches(p)), [false, true, false], "all");
        view.toggle_tag(Snowflake(7));
        assert_eq!(view.tags, vec![Snowflake(8)], "picked again, no longer filtered by");
    }

    #[test]
    fn a_view_resets_to_the_channels_defaults_and_keeps_its_tags() {
        let forum = channel(json!({"id": "15", "type": 15, "default_sort_order": 1}));
        let mut view = View::new(&forum);
        view.sort = Sort::LatestActivity;
        view.layout = Layout::Grid;
        view.tag_match = TagMatch::All;
        view.toggle_tag(Snowflake(7));
        view.reset(&forum);
        assert_eq!((view.sort, view.layout, view.tag_match), (Sort::CreationDate, Layout::List, TagMatch::Some));
        assert_eq!(view.tags, vec![Snowflake(7)]);
    }

    #[test]
    fn active_posts_are_pinned_first_and_then_newest_by_activity_or_creation() {
        let g = guild(vec![
            post(10, Some(50), false, &[], false),
            post(20, Some(30), false, &[], false),
            post(30, None, false, &[], false),
            post(5, Some(1), false, &[], true),
            post(40, Some(99), true, &[], false),
        ]);
        assert_eq!(
            ids(&sorted_posts(&g, FORUM, Sort::LatestActivity)),
            [5, 10, 20, 30],
            "a post nothing was said in counts from its start"
        );
        assert_eq!(ids(&sorted_posts(&g, FORUM, Sort::CreationDate)), [5, 30, 20, 10]);
    }

    #[test]
    fn the_list_changes_only_when_the_client_changes_it() {
        let mut g = guild(vec![post(10, Some(50), false, &[], false), post(20, Some(60), false, &[], false)]);
        let mut view = view(json!({"id": "15", "type": 15}));
        let mut list = Listing::default();
        list.refresh(&g, FORUM, &view);
        assert_eq!(ids(list.shown()), [20, 10]);
        assert!(list.can_ack());

        // Someone else's new post is counted, ours isn't, and neither shows.
        g.set_thread(channel(post(30, Some(70), false, &[], false)));
        let (theirs, ours) = (channel(post(30, None, false, &[], false)), channel(post(31, None, false, &[], false)));
        list.created(&theirs, true, false);
        list.created(&ours, true, true);
        list.created(&channel(post(32, None, false, &[], false)), false, false);
        let elsewhere = channel(json!({"id": "33", "type": 11, "parent_id": "16"}));
        list.created(&elsewhere, true, false);
        assert_eq!((list.new_posts(), ids(list.shown())), (1, vec![20, 10]));

        // A message in a post doesn't move it.
        g.set_thread(channel(post(10, Some(90), false, &[], false)));
        list.arrange(&g, &view);
        assert_eq!(ids(list.shown()), [10, 20], "ordering again does");
        g.set_thread(channel(post(20, Some(95), false, &[], false)));
        assert_eq!(ids(list.shown()), [10, 20], "but nothing asked it to");

        list.refresh(&g, FORUM, &view);
        assert_eq!((list.new_posts(), ids(list.shown())), (0, vec![20, 10, 30]));

        // A pinned post's update puts it first, and a delete takes it out.
        let pinned = post(10, Some(90), false, &[], true);
        g.set_thread(channel(pinned.clone()));
        list.updated(&g, &channel(pinned), &view);
        assert_eq!(ids(list.shown()), [10, 20, 30]);
        let unpinned = channel(post(30, Some(70), false, &[], false));
        list.updated(&g, &unpinned, &view);
        assert_eq!(ids(list.shown()), [10, 20, 30], "an update of another post does nothing");
        list.deleted(&g, &unpinned, &view);
        assert_eq!(ids(list.shown()), [10, 20]);

        view.sort = Sort::CreationDate;
        list.arrange(&g, &view);
        assert_eq!(ids(list.shown()), [10, 20]);
        list.acked(FORUM);
        assert!(!list.can_ack());
    }

    #[test]
    fn the_list_filters_by_tag() {
        let g = guild(vec![post(10, None, false, &[7], false), post(20, None, false, &[], false)]);
        let mut view = view(json!({"id": "15", "type": 15}));
        view.toggle_tag(Snowflake(7));
        let mut list = Listing::default();
        list.refresh(&g, FORUM, &view);
        assert_eq!(ids(list.shown()), [10]);
    }

    #[test]
    fn a_search_names_its_query_as_the_client_does() {
        let mut view = view(json!({"id": "15", "type": 15}));
        let mut archive = Archive::default();
        let first = archive.begin(FORUM, &view).unwrap();
        assert_eq!(
            first.query(),
            "archived=true&sort_by=last_message_time&sort_order=desc&limit=25&tag_setting=match_some&offset=0"
        );
        view.sort = Sort::CreationDate;
        view.tag_match = TagMatch::All;
        view.toggle_tag(Snowflake(30));
        view.toggle_tag(Snowflake(4));
        let tagged = archive.begin(FORUM, &view).unwrap();
        assert_eq!(
            tagged.query(),
            "archived=true&sort_by=last_message_time&sort_order=desc&limit=25&tag=30%2C4&tag_setting=match_all&offset=0",
            "still by last message, whatever we sort by; the tags in the order picked"
        );
    }

    fn page(posts: &[(u64, Option<u64>)], has_more: bool) -> ArchivedPage {
        ArchivedPage {
            threads: posts.iter().map(|&(id, last)| channel(post(id, last, true, &[], false))).collect(),
            members: Vec::new(),
            has_more,
            first_messages: None,
        }
    }

    fn take(g: &mut Guild, page: &ArchivedPage) {
        for t in &page.threads {
            g.set_thread(t.clone());
        }
    }

    #[test]
    fn archived_pages_are_asked_for_in_turn_and_kept_in_order() {
        let view = view(json!({"id": "15", "type": 15}));
        let mut g = guild(vec![post(1, Some(5), true, &[], false)]);
        let mut archive = Archive::default();
        assert!(archive.is_initial(FORUM, &view) && !archive.can_load_more(FORUM, &view));

        let search = archive.begin(FORUM, &view).unwrap();
        assert_eq!(search.offset, 0);
        assert!(archive.is_loading(FORUM, &view) && !archive.is_initial(FORUM, &view));
        assert!(archive.begin(FORUM, &view).is_none(), "one at a time");

        let first = page(&[(2, Some(90)), (3, Some(80))], true);
        take(&mut g, &first);
        archive.arrived(&g, &search, &first);
        // The first page keeps every archived post we know, the older too.
        assert_eq!(ids(archive.threads(FORUM, &view)), [2, 3, 1]);
        assert!(archive.can_load_more(FORUM, &view) && !archive.is_loading(FORUM, &view));

        let search = archive.begin(FORUM, &view).unwrap();
        assert_eq!(search.offset, 25);
        let second = page(&[(4, Some(40))], false);
        take(&mut g, &second);
        archive.arrived(&g, &search, &second);
        // Before this page there was more, so the list stops at its oldest.
        assert_eq!(ids(archive.threads(FORUM, &view)), [2, 3, 4]);
        assert!(!archive.can_load_more(FORUM, &view), "that was the last");
    }

    #[test]
    fn archived_posts_are_listed_by_creation_when_we_sort_that_way() {
        let mut view = view(json!({"id": "15", "type": 15}));
        view.sort = Sort::CreationDate;
        let mut g = guild(vec![]);
        let mut archive = Archive::default();
        let search = archive.begin(FORUM, &view).unwrap();
        let loaded = page(&[(7, Some(100)), (9, None), (8, Some(1))], false);
        take(&mut g, &loaded);
        archive.arrived(&g, &search, &loaded);
        assert_eq!(ids(archive.threads(FORUM, &view)), [9, 8, 7]);
    }

    #[test]
    fn a_failed_page_is_forgotten_when_another_list_is_asked_for() {
        let mut view = view(json!({"id": "15", "type": 15}));
        let mut archive = Archive::default();
        let search = archive.begin(FORUM, &view).unwrap();
        archive.failed(&search);
        assert!(!archive.is_loading(FORUM, &view) && !archive.can_load_more(FORUM, &view));
        assert!(!archive.is_initial(FORUM, &view), "it was asked for");
        view.toggle_tag(Snowflake(1));
        archive.begin(FORUM, &view).unwrap();
        view.tags.clear();
        assert!(archive.is_initial(FORUM, &view), "the failed list is gone");
    }

    #[test]
    fn a_pinned_post_leaves_the_archived_lists_and_a_deleted_one_every_list() {
        let view = view(json!({"id": "15", "type": 15}));
        let mut g = guild(vec![]);
        let mut archive = Archive::default();
        let search = archive.begin(FORUM, &view).unwrap();
        let loaded = page(&[(2, Some(90)), (3, Some(80))], false);
        take(&mut g, &loaded);
        archive.arrived(&g, &search, &loaded);
        archive.pinned(&channel(post(2, Some(90), false, &[], true)));
        assert_eq!(ids(archive.threads(FORUM, &view)), [3]);
        archive.deleted(Snowflake(3));
        assert!(archive.threads(FORUM, &view).is_empty());
        archive.channel_deleted(FORUM);
        assert!(archive.is_initial(FORUM, &view));
    }

    #[test]
    fn only_fifty_lists_are_kept() {
        let mut view = view(json!({"id": "15", "type": 15}));
        let mut archive = Archive::default();
        for tag in 1..=60 {
            view.tags = vec![Snowflake(tag)];
            let search = archive.begin(FORUM, &view).unwrap();
            archive.failed(&search);
            // A list that failed goes when the next is asked for.
        }
        assert_eq!(archive.loads.len(), 1);
        let mut kept = Archive::default();
        for tag in 1..=60 {
            view.tags = vec![Snowflake(tag)];
            let search = kept.begin(FORUM, &view).unwrap();
            let g = guild(vec![]);
            kept.arrived(&g, &search, &page(&[], false));
        }
        assert_eq!(kept.loads.len(), 50);
        view.tags = vec![Snowflake(1)];
        assert!(kept.is_initial(FORUM, &view), "the oldest went");
        view.tags = vec![Snowflake(60)];
        assert!(!kept.is_initial(FORUM, &view));
    }

    #[test]
    fn a_page_without_threads_is_a_failure() {
        assert!(ArchivedPage::from_value(&json!({"threads": null})).is_none());
        assert!(ArchivedPage::from_value(&json!({})).is_none());
        let page = ArchivedPage::from_value(&json!({
            "threads": [post(2, Some(3), true, &[], false), {"id": "9", "type": 0}],
            "members": [{"id": "2", "user_id": "5", "flags": 0, "join_timestamp": "2026-10-28T00:00:00Z"}],
            "first_messages": [{"id": "2", "channel_id": "2", "content": "hi", "timestamp": "2026-10-28T00:00:00Z",
                                "author": {"id": "5", "username": "a"}}],
            "has_more": true,
        }))
        .unwrap();
        assert_eq!((page.threads.len(), page.members.len(), page.has_more), (1, 1, true), "only threads");
        let mut first = FirstMessages::default();
        first.from_page(&page);
        assert_eq!(first.get(Snowflake(2)).and_then(|s| s.message.as_ref()).map(|m| m.content.as_str()), Some("hi"));
    }

    #[test]
    fn a_page_with_no_first_messages_leaves_them_unknown() {
        let mut first = FirstMessages::default();
        first.from_page(&page(&[(2, None)], false));
        assert!(first.get(Snowflake(2)).is_none());
        let with = ArchivedPage { first_messages: Some(Vec::new()), ..page(&[(2, None)], false) };
        first.from_page(&with);
        assert!(first.get(Snowflake(2)).is_some_and(|s| s.loaded && s.message.is_none()), "a post with none");
    }

    fn sorted(ids: &[u64]) -> Vec<Snowflake> {
        ids.iter().map(|&i| Snowflake(i)).collect()
    }

    #[test]
    fn a_card_asks_for_its_own_first_message_and_the_four_after_it() {
        let mut first = FirstMessages::default();
        let list = sorted(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(first.card(FORUM, Snowflake(3), &list));
        assert_eq!(first.next_request(), Some((FORUM, sorted(&[3, 4, 5, 6, 7]))));
        // Asking again for a post already wanted changes nothing, and one
        // of the five is covered.
        assert!(!first.card(FORUM, Snowflake(4), &list));
        assert!(first.card(FORUM, Snowflake(1), &list));
        assert_eq!(first.next_request(), Some((FORUM, sorted(&[3, 4, 5, 6, 7, 1, 2]))));
        first.finished(FORUM, &sorted(&[3, 4, 5, 6, 7, 1, 2]));
        assert_eq!(first.next_request(), None);
        // Asked for once; the client asks again only when answered with none.
        assert!(first.card(FORUM, Snowflake(8), &list));
    }

    #[test]
    fn a_request_names_at_most_ten_posts_and_goes_forum_by_forum() {
        let mut first = FirstMessages::default();
        let many: Vec<u64> = (1..=14).collect();
        let list = sorted(&many);
        for post in [1, 6, 11] {
            first.card(FORUM, Snowflake(post), &list);
        }
        first.card(Snowflake(16), Snowflake(99), &sorted(&[99]));
        let (forum, posts) = first.next_request().unwrap();
        assert_eq!((forum, posts.len()), (FORUM, 10));
        first.finished(forum, &posts);
        let (forum, posts) = first.next_request().unwrap();
        assert_eq!((forum, ids(&posts)), (FORUM, vec![11, 12, 13, 14]), "the rest of the forum first");
        first.finished(forum, &posts);
        assert_eq!(first.next_request(), Some((Snowflake(16), sorted(&[99]))));
    }

    #[test]
    fn a_post_in_no_list_reads_from_the_end() {
        let mut first = FirstMessages::default();
        let list = sorted(&[1, 2, 3]);
        // JavaScript's slice(-1, 4) of a short list.
        assert!(first.card(FORUM, Snowflake(60), &list));
        assert_eq!(first.next_request(), Some((FORUM, sorted(&[3]))));
        let long = sorted(&[1, 2, 3, 4, 5, 6]);
        let mut other = FirstMessages::default();
        assert!(!other.card(FORUM, Snowflake(60), &long), "slice(-1, 4) of a long one is empty");
    }

    #[test]
    fn a_known_first_message_needs_no_request() {
        let mut first = FirstMessages::default();
        first.from_post_data(&json!({"threads": {"3": {"first_message": null}}}));
        assert!(first.get(Snowflake(3)).is_some_and(|s| s.loaded && s.message.is_none()));
        assert!(!first.card(FORUM, Snowflake(3), &sorted(&[3])));
        first.from_post_data(&json!({"threads": {"4": {"first_message": {
            "id": "4", "channel_id": "4", "content": "x", "timestamp": "2026-10-28T00:00:00Z",
            "author": {"id": "5", "username": "a"}}}}}));
        assert!(first.get(Snowflake(4)).is_some_and(|s| s.message.is_some()));
    }

    #[test]
    fn a_first_message_follows_what_happens_to_it() {
        let message = |id: u64, channel: u64, content: &str| -> Message {
            serde_json::from_value(json!({"id": id.to_string(), "channel_id": channel.to_string(), "content": content,
                "timestamp": "2026-10-28T00:00:00Z", "author": {"id": "5", "username": "a"}}))
            .unwrap()
        };
        let mut first = FirstMessages::default();
        first.created(&message(8, 4, "a reply"));
        assert!(first.get(Snowflake(4)).is_none(), "not the first message");
        first.created(&message(4, 4, "start"));
        first.change(Snowflake(4), Snowflake(4), |m| m.content = "edited".into());
        assert_eq!(first.get(Snowflake(4)).unwrap().message.as_ref().unwrap().content, "edited");
        first.change(Snowflake(4), Snowflake(9), |m| m.content = "other".into());
        assert_eq!(first.get(Snowflake(4)).unwrap().message.as_ref().unwrap().content, "edited");
        first.deleted(Snowflake(4), Snowflake(8));
        assert!(first.get(Snowflake(4)).unwrap().message.is_some());
        first.deleted(Snowflake(4), Snowflake(4));
        assert!(first.get(Snowflake(4)).is_some_and(|s| s.loaded && s.message.is_none()));
        first.thread_created(Snowflake(5), false);
        assert!(first.get(Snowflake(5)).is_none(), "not subscribed");
        first.thread_created(Snowflake(5), true);
        assert!(first.get(Snowflake(5)).is_some_and(|s| s.loaded));
        first.clear();
        assert!(first.get(Snowflake(5)).is_none());
    }

    #[test]
    fn unread_counts_are_asked_for_once_and_kept_up_to_date() {
        let mut unreads = Unreads::default();
        let posts = sorted(&[1, 2, 3]);
        assert_eq!(ids(&unreads.missing(posts.iter().copied())), [1, 2, 3]);
        unreads.asked(&sorted(&[1, 2]));
        unreads.answered(&[(Snowflake(1), Some(4)), (Snowflake(2), None)]);
        assert_eq!(ids(&unreads.missing(posts.iter().copied())), [3]);
        assert_eq!((unreads.count(Snowflake(1)), unreads.count(Snowflake(2))), (Some(4), None));
        unreads.message(Snowflake(1));
        unreads.message(Snowflake(3));
        assert_eq!((unreads.count(Snowflake(1)), unreads.count(Snowflake(3))), (Some(5), None), "only those we count");
        unreads.read(Snowflake(1));
        unreads.created(Snowflake(9));
        assert_eq!((unreads.count(Snowflake(1)), unreads.count(Snowflake(9))), (Some(0), Some(0)));
        unreads.clear();
        assert_eq!(ids(&unreads.missing(posts.iter().copied())), [1, 2, 3]);
    }

    #[test]
    fn op_28_names_at_most_180_posts_and_none_when_there_are_none() {
        let posts: Vec<_> = (1..=300).map(|p| (Snowflake(p), Snowflake(1))).collect();
        assert_eq!(UnreadRequest::new(Snowflake(1), FORUM, &posts).unwrap().posts().len(), 180);
        assert!(UnreadRequest::new(Snowflake(1), FORUM, &[]).is_none());
    }

    #[test]
    fn op_28_names_the_posts_as_the_client_does() {
        let body = UnreadRequest::new(Snowflake(1), FORUM, &[(Snowflake(20), Snowflake(30))]).unwrap();
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"guild_id":"1","channel_id":"15","threads":[{"thread_id":"20","ack_message_id":"30"}]}"#
        );
    }

    #[test]
    fn a_forum_can_keep_its_medias_options_hidden() {
        assert!(channel(json!({"id": "15", "type": 15, "flags": 1 << 15})).hides_media_options());
        assert!(!channel(json!({"id": "15", "type": 15, "flags": 1 << 1})).hides_media_options());
    }

    #[test]
    fn a_post_says_when_it_was_last_active() {
        // Snowflakes: 1_000 ms and 61 minutes after Discord's epoch.
        let at = |ms: u64| Snowflake(ms << 22);
        let mut post = channel(post(0, None, false, &[], false));
        post.id = at(1_000);
        assert_eq!(last_activity_ms(&post), 1_420_070_401_000, "when it began");
        post.last_message_id = Some(at(3_660_000));
        assert_eq!(last_activity_ms(&post), 1_420_074_060_000);
        let now = 1_420_074_060_000;
        assert_eq!(ago(now, now), "just now");
        assert_eq!((ago(now + 60_000, now), ago(now + 59 * 60_000, now)), ("1m ago".to_owned(), "59m ago".to_owned()));
        assert_eq!(
            (ago(now + 3_600_000, now), ago(now + 86_400_000 * 3, now)),
            ("1h ago".to_owned(), "3d ago".to_owned())
        );
        assert_eq!(ago(now + 86_400_000 * 31, now), ">30d ago");
        assert_eq!(ago(now - 5, now), "just now", "a clock that's behind");
    }

    #[test]
    fn counts_read_as_a_card_says_them() {
        let old = Snowflake(900_000_000_000_000_000);
        let new = Snowflake(1_400_000_000_000_000_000);
        assert_eq!(message_count_text(49, old), "49");
        assert_eq!((message_count_text(50, old), message_count_text(50, new)), ("50+".into(), "50".into()));
        assert_eq!(
            (message_count_text(99_999, new), message_count_text(100_000, new)),
            ("99999".into(), "100k+".into())
        );
        assert_eq!(unread_text(None, 40), "1+");
        assert_eq!(unread_text(Some(0), 40), "1+");
        assert_eq!(
            (unread_text(Some(7), 40), unread_text(Some(30), 40), unread_text(Some(9), 3)),
            ("7".into(), "25+".into(), "3".into())
        );
    }
}
