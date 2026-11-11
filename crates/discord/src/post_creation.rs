//! Starting a post in a forum or media channel: who the client lets, what
//! its form wants before it sends, and the request, which is the one a plain
//! thread starts with, carrying the first message. What a thread's creation
//! shares with it is in [`crate::thread_creation`]; the post list is
//! [`crate::forum`].

use serde_json::{Value, json};

use crate::model::{Channel, Guild, Tag};
use crate::permissions::{IS_MODERATOR_REPORT_CHANNEL, Permissions, in_channel};
use crate::verification::Account;
use crate::{Snowflake, scheduled, thread_creation};

/// The query the client adds to the thread request of a post
/// (`createForumPost` in 971389).
pub const QUERY: &str = "use_nested_fields=true";

/// How many tags a post may take (`iT` in 745431).
pub const TAGS_MAX: usize = 5;

/// Channel flag: a post needs a tag.
const REQUIRE_TAG: u64 = 1 << 4;

/// What AutoMod's refusal of a post says where it gave no reason.
pub const AUTOMOD_REASON: &str = "This post cannot be sent because it contains content blocked by this community. Please revise your post and try again.";

/// Whether the client shows the post form in `forum` (`canCreatePost`, `ig`
/// in 745431): Send Messages in the forum itself, which is what `g` in 406704
/// asks of a forum or media channel, and never in the channel of a server's
/// moderator reports.
pub fn can_post(guild: &Guild, forum: &Channel, me: &Account, now: i64) -> bool {
    forum.kind.is_forum_like()
        && forum.flags & IS_MODERATOR_REPORT_CHANNEL == 0
        && thread_creation::can_start(guild, forum, None, me, now)
}

/// Whether the form takes files (`ATTACH_FILES` in `ig`), where it shows.
pub fn can_attach(guild: &Guild, forum: &Channel, me: &Account, now: i64) -> bool {
    can_post(guild, forum, me, now) && in_channel(guild, forum, me, now).contains(Permissions::ATTACH_FILES)
}

/// The tags the form offers (`OT` in 691060): those that only people with
/// Manage Threads may apply are left out for anyone else.
pub fn tag_choices<'a>(guild: &Guild, forum: &'a Channel, me: &Account, now: i64) -> Vec<&'a Tag> {
    let manage = in_channel(guild, forum, me, now).contains(Permissions::MANAGE_THREADS);
    forum.available_tags.iter().filter(|tag| manage || !tag.moderated).collect()
}

/// A tag picked or put back, in the order they were picked (the client keeps
/// a set, which sends in that order). A sixth can't be added.
pub fn toggle_tag(applied: &mut Vec<Snowflake>, tag: Snowflake) {
    match applied.iter().position(|t| *t == tag) {
        Some(at) => {
            applied.remove(at);
        }
        None if applied.len() < TAGS_MAX => applied.push(tag),
        None => {}
    }
}

/// What the form holds when it is looked at.
pub struct Draft<'a> {
    pub name: &'a str,
    /// The message, trimmed.
    pub text: &'a str,
    pub files: usize,
    pub tags: usize,
}

/// What stops the form from posting (`M` and `iN` in 745431).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Problems {
    /// The title is empty (spaces count as something, as the client's check
    /// does, though its button waits for more).
    pub name: bool,
    /// No text and no file.
    pub message: bool,
    /// The forum wants a tag and none is picked: said once there is a title
    /// and something to say, which the client does without waiting for a
    /// press.
    pub tag: bool,
}

impl Problems {
    pub fn of(forum: &Channel, draft: &Draft) -> Self {
        let message = draft.text.is_empty() && draft.files == 0;
        let name = draft.name.is_empty();
        let tag = forum.flags & REQUIRE_TAG != 0 && draft.tags == 0 && !name && !message;
        Problems { name, message, tag }
    }

    /// Whether pressing Post goes through.
    pub fn none(&self) -> bool {
        !(self.name || self.message || self.tag)
    }
}

/// Whether the Post button is pressable: the title has more than spaces
/// besides what [`Problems`] asks (`iS` in 745431).
pub fn postable(forum: &Channel, draft: &Draft) -> bool {
    Problems::of(forum, draft).none() && !draft.name.trim().is_empty()
}

/// What starting a post sends (`createForumPost` in 971389): the title, when
/// it archives itself, the tags and the first message with its uploaded
/// `attachments`. `@silent` at the start of the message is the flag (`Ay` in
/// 677413). No `location`, no `type`, as a thread's request has them.
pub fn body(forum: &Channel, name: &str, tags: &[Snowflake], text: &str, attachments: Vec<Value>) -> Value {
    let (text, flags) = scheduled::split_silent(text, 0);
    let mut message = json!({ "content": text });
    if flags != 0 {
        message["flags"] = json!(flags);
    }
    if !attachments.is_empty() {
        message["attachments"] = Value::Array(attachments);
    }
    json!({
        "name": name,
        "auto_archive_duration": forum.default_auto_archive_duration.unwrap_or(thread_creation::AUTO_ARCHIVE_FALLBACK),
        "applied_tags": tags,
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Snowflake = Snowflake(5);

    fn me() -> Account {
        Account { id: ME, verified: true, email: true, phone: false, mfa: false }
    }

    fn forum(granted: Permissions, extra: Value) -> (Guild, Channel) {
        let everyone = (Permissions::VIEW_CHANNEL | granted).0.to_string();
        let guild = serde_json::from_value(json!({
            "id": "1", "name": "G", "owner_id": "2",
            "roles": [{"id": "1", "name": "@everyone", "permissions": everyone}],
        }))
        .unwrap();
        let mut channel = json!({"id": "9", "type": 15, "available_tags": [
            {"id": "11", "name": "open"}, {"id": "12", "name": "staff", "moderated": true}]});
        channel.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        (guild, serde_json::from_value(channel).unwrap())
    }

    #[test]
    fn posting_takes_send_messages_in_the_forum_and_files_take_attach_files() {
        let can = |granted, extra| {
            let (guild, channel) = forum(granted, extra);
            (can_post(&guild, &channel, &me(), 0), can_attach(&guild, &channel, &me(), 0))
        };
        assert_eq!(can(Permissions::SEND_MESSAGES, json!({})), (true, false));
        assert_eq!(can(Permissions::SEND_MESSAGES | Permissions::ATTACH_FILES, json!({})), (true, true));
        assert_eq!(can(Permissions::ATTACH_FILES, json!({})), (false, false), "files alone don't post");
        // Creating threads isn't what it takes, and nobody posts in a forum of reports.
        assert!(!can(Permissions::CREATE_PUBLIC_THREADS | Permissions::READ_MESSAGE_HISTORY, json!({})).0);
        assert_eq!(can(Permissions::SEND_MESSAGES, json!({"flags": 1 << 19})), (false, false));
        let (guild, media) = forum(Permissions::SEND_MESSAGES, json!({"type": 16}));
        assert!(can_post(&guild, &media, &me(), 0), "a media channel too");
        let (guild, text) = forum(Permissions::SEND_MESSAGES, json!({"type": 0}));
        assert!(!can_post(&guild, &text, &me(), 0), "not a text channel");
    }

    #[test]
    fn tags_only_moderators_apply_are_offered_to_those_who_can_manage_threads() {
        let names = |granted| {
            let (guild, channel) = forum(granted, json!({}));
            tag_choices(&guild, &channel, &me(), 0).iter().map(|t| t.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(Permissions::SEND_MESSAGES), ["open"]);
        assert_eq!(names(Permissions::MANAGE_THREADS), ["open", "staff"]);
    }

    #[test]
    fn at_most_five_tags_go_on_a_post_and_in_the_order_picked() {
        let mut tags = Vec::new();
        for id in [4, 2, 9, 7, 1, 8] {
            toggle_tag(&mut tags, Snowflake(id));
        }
        assert_eq!(tags, [4, 2, 9, 7, 1].map(Snowflake), "a sixth is not added");
        toggle_tag(&mut tags, Snowflake(9));
        toggle_tag(&mut tags, Snowflake(8));
        assert_eq!(tags, [4, 2, 7, 1, 8].map(Snowflake), "putting one back makes room");
    }

    #[test]
    fn the_form_wants_a_title_something_to_say_and_a_tag_where_the_forum_asks() {
        let (_, plain) = forum(Permissions::NONE, json!({}));
        let (_, strict) = forum(Permissions::NONE, json!({"flags": 16}));
        let draft = |name, text, files, tags| Draft { name, text, files, tags };
        let problems = |channel: &Channel, d: &Draft| Problems::of(channel, d);
        assert_eq!(problems(&plain, &draft("", "", 0, 0)), Problems { name: true, message: true, tag: false });
        assert!(problems(&plain, &draft("t", "", 1, 0)).none(), "a file is something to say");
        assert!(problems(&plain, &draft("t", "hi", 0, 0)).none());
        assert_eq!(problems(&strict, &draft("t", "hi", 0, 0)), Problems { name: false, message: false, tag: true });
        // The tag isn't asked for before there is anything to post.
        assert!(!problems(&strict, &draft("", "hi", 0, 0)).tag && !problems(&strict, &draft("t", "", 0, 0)).tag);
        assert!(problems(&strict, &draft("t", "hi", 0, 1)).none());
        // A title of spaces passes the check and not the button.
        assert!(problems(&plain, &draft(" ", "hi", 0, 0)).none() && !postable(&plain, &draft(" ", "hi", 0, 0)));
        assert!(postable(&plain, &draft("t", "hi", 0, 0)) && !postable(&strict, &draft("t", "hi", 0, 0)));
    }

    #[test]
    fn the_request_is_a_thread_with_the_first_message_in_it() {
        let (_, channel) = forum(Permissions::NONE, json!({"default_auto_archive_duration": 1440}));
        let attachment = json!({"id": "0", "filename": "a.png", "uploaded_filename": "u/a.png"});
        let sent = body(&channel, "Plans", &[Snowflake(11), Snowflake(12)], "hello", vec![attachment.clone()]);
        assert_eq!(
            sent,
            json!({
                "name": "Plans", "auto_archive_duration": 1440, "applied_tags": ["11", "12"],
                "message": {"content": "hello", "attachments": [attachment]},
            })
        );
        let bare = body(&channel, "Plans", &[], "hello", Vec::new());
        assert_eq!(bare["message"], json!({"content": "hello"}), "no flags or attachments unless there are some");
        assert_eq!(bare["applied_tags"], json!([]));
        assert_eq!(bare.get("type"), None, "a post's type is Discord's to choose");
        let (_, none) = forum(Permissions::NONE, json!({}));
        assert_eq!(body(&none, "p", &[], "x", Vec::new())["auto_archive_duration"], 4320);
    }

    #[test]
    fn a_silent_post_sends_the_flag_and_not_the_word() {
        let (_, channel) = forum(Permissions::NONE, json!({}));
        let sent = body(&channel, "p", &[], "@silent hello", Vec::new());
        assert_eq!(sent["message"], json!({"content": "hello", "flags": 4096}));
        assert_eq!(body(&channel, "p", &[], "@silently", Vec::new())["message"]["content"], "@silently");
    }
}
