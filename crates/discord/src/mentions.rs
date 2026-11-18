//! The inbox's Mentions tab: the messages that mention us, which
//! `RecentMentionsStore` (849171) keeps and the inbox (713024) lists, asked
//! for with `GET /users/@me/mentions` (`fetchRecentMentions`, 378939) and
//! taken off one by one with `DELETE /users/@me/mentions/{id}`.

use std::collections::HashSet;

use crate::{Message, MessageEdit, Snowflake};

/// How many mentions a page holds, and how many stay when the inbox closes.
pub const PAGE: usize = 25;

/// What the list includes (`recentMentionFilterSettings`: all servers,
/// @everyone and role mentions in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub everyone: bool,
    pub roles: bool,
    /// Only the open server's, rather than all.
    pub this_server: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { everyone: true, roles: true, this_server: false }
    }
}

/// The request's query as the inbox builds it (`sj` in 713024): `before`,
/// `limit`, `guild_id` (only for "this server"), `roles` and `everyone`; what
/// is null is left out.
pub fn query(before: Option<Snowflake>, guild: Option<Snowflake>, filter: Filter) -> String {
    let mut parts = Vec::new();
    if let Some(before) = before {
        parts.push(format!("before={before}"));
    }
    parts.push(format!("limit={PAGE}"));
    if let Some(guild) = guild {
        parts.push(format!("guild_id={guild}"));
    }
    parts.push(format!("roles={}", filter.roles));
    parts.push(format!("everyone={}", filter.everyone));
    parts.join("&")
}

#[derive(Debug)]
pub struct Mentions {
    /// Newest first.
    list: Vec<Message>,
    ids: HashSet<Snowflake>,
    loaded_ever: bool,
    loading: bool,
    more: bool,
}

impl Default for Mentions {
    fn default() -> Self {
        Mentions::new()
    }
}

impl Mentions {
    pub fn new() -> Mentions {
        Mentions { list: Vec::new(), ids: HashSet::new(), loaded_ever: false, loading: false, more: true }
    }

    pub fn list(&self) -> &[Message] {
        &self.list
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn has_more(&self) -> bool {
        self.more
    }

    /// Whether the inbox asks for the first page when it opens: once loaded
    /// it doesn't, since live mentions are added as they arrive.
    pub fn needs_first_page(&self) -> bool {
        !self.loaded_ever
    }

    /// Whether the next page is due (`hasMore`, nothing in flight).
    pub fn wants_more(&self) -> bool {
        (self.more || !self.loaded_ever) && !self.loading
    }

    /// The `before` of the next page: the oldest we have.
    pub fn before(&self) -> Option<Snowflake> {
        self.list.last().map(|m| m.id)
    }

    pub fn started(&mut self) {
        self.loading = true;
    }

    /// A page came: after `before` it joins the list, else it replaces it.
    pub fn loaded(&mut self, page: Vec<Message>, after: bool) {
        self.more = page.len() >= PAGE;
        if !after {
            self.list.clear();
            self.ids.clear();
        }
        self.ids.extend(page.iter().map(|m| m.id));
        self.list.extend(page);
        self.loading = false;
        self.loaded_ever = true;
    }

    pub fn failed(&mut self) {
        self.loading = false;
    }

    /// The inbox closed: only the newest page stays (`TRUNCATE_MENTIONS`).
    pub fn truncate(&mut self) {
        if self.list.len() > PAGE {
            for m in self.list.drain(PAGE..) {
                self.ids.remove(&m.id);
            }
            self.more = true;
        }
    }

    /// A new session, or a filter that changed what the list can hold.
    pub fn clear(&mut self) {
        *self = Mentions::new();
    }

    /// A message was marked done, or was deleted.
    pub fn remove(&mut self, id: Snowflake) {
        if self.ids.remove(&id) {
            self.list.retain(|m| m.id != id);
        }
    }

    /// The messages that no longer count (a deleted channel's, a server we
    /// left, someone now blocked).
    pub fn remove_where(&mut self, gone: impl Fn(&Message) -> bool) {
        self.list.retain(|m| !gone(m));
        self.ids = self.list.iter().map(|m| m.id).collect();
    }

    /// A message that mentions us arrived (`MESSAGE_CREATE`): it goes first.
    pub fn arrived(&mut self, message: Message) {
        if self.ids.insert(message.id) {
            self.list.insert(0, message);
        }
    }

    /// A message in the list changed.
    pub fn edited(&mut self, edit: &MessageEdit) {
        if self.ids.contains(&edit.id)
            && let Some(message) = self.list.iter_mut().find(|m| m.id == edit.id)
        {
            edit.clone().apply(message);
        }
    }

    /// The filter changed what the list holds (`SET_RECENT_MENTIONS_FILTER`):
    /// a narrower one keeps the messages that still pass, and an empty list
    /// is asked for again; a wider one (or none) starts over.
    pub fn refilter(&mut self, narrower: bool, keep: impl Fn(&Message) -> bool) {
        if narrower {
            self.remove_where(|m| !keep(m));
            if self.list.is_empty() {
                self.loaded_ever = false;
            }
        } else {
            let more = self.more;
            self.clear();
            self.more = more;
        }
    }

    /// The open channel changed while "this server" is the filter: the list
    /// is another server's now.
    pub fn stale_for_server(&mut self) {
        self.loaded_ever = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: u64) -> Message {
        serde_json::from_value(serde_json::json!({"id": id.to_string(), "channel_id": "1",
            "author": {"id": "5", "username": "a"}, "content": "hi", "timestamp": ""}))
        .unwrap()
    }

    fn page(from: u64, count: u64) -> Vec<Message> {
        (from..from + count).rev().map(message).collect()
    }

    #[test]
    fn the_query_is_the_inboxs_without_what_is_null() {
        let all = Filter::default();
        assert_eq!(query(None, None, all), "limit=25&roles=true&everyone=true");
        let narrow = Filter { everyone: false, roles: true, this_server: true };
        assert_eq!(
            query(Some(Snowflake(9)), Some(Snowflake(3)), narrow),
            "before=9&limit=25&guild_id=3&roles=true&everyone=false"
        );
    }

    #[test]
    fn pages_join_and_a_short_one_is_the_last() {
        let mut mentions = Mentions::new();
        assert!(mentions.needs_first_page() && mentions.wants_more());
        mentions.started();
        assert!(!mentions.wants_more(), "in flight");
        mentions.loaded(page(100, 25), false);
        assert!(!mentions.needs_first_page() && mentions.has_more());
        assert_eq!(mentions.before(), Some(Snowflake(100)));
        mentions.loaded(page(80, 5), true);
        assert_eq!((mentions.list().len(), mentions.has_more()), (30, false));
        assert!(!mentions.wants_more());
    }

    #[test]
    fn closing_keeps_the_newest_page_and_the_rest_can_be_asked_for_again() {
        let mut mentions = Mentions::new();
        mentions.loaded(page(100, 25), false);
        mentions.loaded(page(70, 20), true);
        mentions.truncate();
        assert_eq!(mentions.list().len(), 25);
        assert!(mentions.has_more());
        assert_eq!(mentions.before(), Some(Snowflake(100)));
    }

    #[test]
    fn live_mentions_go_first_and_done_ones_go() {
        let mut mentions = Mentions::new();
        mentions.loaded(page(10, 3), false);
        mentions.arrived(message(50));
        mentions.arrived(message(50));
        assert_eq!(mentions.list()[0].id, Snowflake(50));
        assert_eq!(mentions.list().len(), 4, "once");
        mentions.remove(Snowflake(11));
        assert!(mentions.list().iter().all(|m| m.id != Snowflake(11)));
        mentions.remove_where(|m| m.id == Snowflake(50));
        assert_eq!(mentions.list().len(), 2);
    }

    #[test]
    fn a_narrower_filter_keeps_what_passes_and_a_wider_one_starts_over() {
        let mut mentions = Mentions::new();
        mentions.loaded(page(10, 4), false);
        mentions.refilter(true, |m| m.id.0 % 2 == 0);
        assert_eq!(mentions.list().len(), 2);
        mentions.refilter(true, |_| false);
        assert!(mentions.needs_first_page(), "nothing left: ask again");
        mentions.loaded(page(10, 4), false);
        mentions.refilter(false, |_| true);
        assert!(mentions.list().is_empty() && mentions.needs_first_page());
    }
}
