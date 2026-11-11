//! The context menu entries for what we hear of and what counts as read:
//! Mark As Read, Mute with its durations, Notification Settings, and Mark
//! Unread, on servers, channels, categories, DMs and group DMs.
//!
//! Settings go out as the client sends them (see `discord::notification_edit`)
//! and apply here at once, as the client's do. Where the client greys an item
//! out (Mark As Read with nothing to read), these don't show it.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use discord::notification_edit::{self, MUTE_FOR};
use discord::{ChannelKind, Snowflake};
use serde_json::{Value, json};

use super::App;
use super::chat::MessageTarget;
use super::menus::{Choice, Choices, Entry, Item, Kind, Style};
use super::sidebar::ChannelTarget;
use super::state::now_unix;

/// What a server icon's menu acts on.
pub(super) struct GuildTarget(pub(super) Snowflake);

/// Where a channel's settings are kept: its server's entry, or the DMs'.
fn guild_of(app: &App, channel: Snowflake) -> Option<Option<Snowflake>> {
    app.state.borrow().find_channel(channel).map(|(guild, _)| guild.map(|g| g.id))
}

fn kind_of(app: &App, channel: Snowflake) -> Option<ChannelKind> {
    app.state.borrow().find_channel(channel).map(|(_, c)| c.kind)
}

impl App {
    /// Save settings for a server, or the DMs, here and on Discord.
    fn save_notification_settings(self: &Rc<Self>, guild: Option<Snowflake>, settings: Value) {
        let rest = {
            let mut state = self.state.borrow_mut();
            state.notifications.apply(guild, &settings);
            state.rest.clone()
        };
        self.refresh_badges();
        let Some(rest) = rest else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = rest.save_notification_settings(guild, settings).await {
                eprintln!("notification settings: {e}");
            }
        });
    }

    /// Change one channel's override.
    fn save_channel_settings(self: &Rc<Self>, channel: Snowflake, settings: Value) {
        if let Some(guild) = guild_of(self, channel) {
            self.save_notification_settings(guild, notification_edit::channel(channel, settings));
        }
    }

    /// Mark channels read now, one request each (`CHANNEL_ACK` with
    /// `immediate`, from the menu).
    fn mark_channels_read(self: &Rc<Self>, targets: Vec<(Snowflake, Snowflake)>) {
        let rest = self.state.borrow().rest.clone();
        for &(channel, message) in &targets {
            self.state.borrow_mut().read.ack(channel, message);
            self.withdraw_notification(channel);
        }
        self.refresh_badges();
        let Some(rest) = rest else { return };
        glib::spawn_future_local(async move {
            for (channel, message) in targets {
                if let Err(e) = rest.ack(channel, message).await {
                    eprintln!("ack {channel}: {e}");
                }
            }
        });
    }

    /// Mark a server read: every channel with news, in one bulk ack.
    fn mark_guild_read(self: &Rc<Self>, guild: Snowflake) {
        let (acks, rest) = {
            let state = self.state.borrow();
            let acks = state.guilds.iter().find(|g| g.id == guild).map(|g| state.guild_acks(g)).unwrap_or_default();
            (acks, state.rest.clone())
        };
        for ack in &acks {
            self.state.borrow_mut().read.ack(ack.channel, ack.message);
            self.withdraw_notification(ack.channel);
        }
        self.refresh_badges();
        let Some(rest) = rest.filter(|_| !acks.is_empty()) else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = rest.ack_bulk(&acks).await {
                eprintln!("ack-bulk: {e}");
            }
        });
    }
}

impl App {
    /// Whether Mark Unread shows for a message (`D` in 406704): in any channel
    /// but a thread, where the client wants it active, or archived, open and
    /// ours to post in. Neucord leaves out archived threads and ones we
    /// haven't joined, which the client reopens and joins first.
    pub(super) fn offers_mark_unread(&self, target: &MessageTarget) -> bool {
        let state = self.state.borrow();
        state
            .find_channel(target.message.channel_id)
            .is_some_and(|(_, c)| !c.kind.is_thread() || (c.is_active_thread() && c.member.is_some()))
    }

    /// Mark everything from the message on unread: an ack to the message
    /// before it among the ones loaded, `manual`, with the mentions after it
    /// (`439147`).
    pub(super) fn mark_unread(self: &Rc<Self>, target: &MessageTarget) {
        let (channel, message) = (target.message.channel_id, target.message.id);
        let loaded: Vec<discord::Message> = self
            .timelines
            .borrow()
            .get(channel)
            .map(|t| (0..t.store.n_items()).filter_map(|n| crate::timelines::message_at(&t.store, n)).collect())
            .unwrap_or_default();
        let ids: Vec<Snowflake> = loaded.iter().map(|m| m.id).collect();
        let before = notification_edit::unread_before(&ids, message);
        let (mentions, rest) = {
            let state = self.state.borrow();
            let Some(me) = state.me() else { return };
            let place = state.place(channel);
            let group_dm = state.find_channel(channel).is_some_and(|(_, c)| c.kind == ChannelKind::GroupDm);
            let roles = loaded
                .first()
                .and_then(|m| m.guild_id)
                .and_then(|g| state.guilds.iter().find(|guild| guild.id == g))
                .map(|g| g.my_roles.clone())
                .unwrap_or_default();
            let blocked = |id| state.relationships.is_blocked(id);
            let count = loaded
                .iter()
                .filter(|m| m.id > before)
                .filter(|m| {
                    let pinged = state.notifications.mentions_me(m, me, &roles);
                    state.notifications.counts_mention(place.as_ref(), group_dm, m, pinged, blocked, now_unix())
                })
                .count() as u32;
            (count, state.rest.clone())
        };
        self.state.borrow_mut().read.mark_unread(channel, before, mentions);
        self.refresh_badges();
        let Some(rest) = rest else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = rest.ack_unread(channel, before, mentions).await {
                eprintln!("mark unread {channel}: {e}");
            }
        });
    }
}

fn durations() -> Vec<Choice> {
    MUTE_FOR
        .iter()
        .map(|(seconds, label)| Choice { value: seconds.to_string(), label: (*label).to_owned(), ticked: false })
        .collect()
}

fn mute_for(now: i64, value: &str) -> Option<Value> {
    let seconds = value.parse().ok()?;
    let at = glib::DateTime::from_unix_utc(now).ok()?;
    Some(notification_edit::mute(seconds, &at))
}

/// Mark As Read on a channel, a category, a DM: shown with something to read
/// (`511904`; the client greys it out otherwise).
pub(super) const MARK_CHANNEL_READ: Entry<ChannelTarget> = Entry::Item(Item {
    name: "mark-channel-read",
    label: "Mark As _Read",
    shows: |app, target| {
        let state = app.state.borrow();
        match state.find_channel(target.0) {
            Some((_, c)) if c.kind == ChannelKind::Category => state
                .ack_targets(target.0)
                .iter()
                .any(|&(id, _)| state.find_channel(id).is_some_and(|(_, c)| state.has_news(c))),
            Some((_, c)) => !c.kind.is_forum_like() && state.has_news(c),
            None => false,
        }
    },
    kind: Kind::Action(|app, target| {
        let targets = app.state.borrow().ack_targets(target.0);
        app.mark_channels_read(targets);
    }),
});

macro_rules! mute_items {
    ($mute:ident, $unmute:ident, $name:literal, $mute_label:literal, $unmute_label:literal, $kinds:expr) => {
        pub(super) const $mute: Entry<ChannelTarget> = Entry::Item(Item {
            name: concat!("mute-", $name),
            label: $mute_label,
            shows: |app, target| {
                let state = app.state.borrow();
                kind_of(app, target.0).is_some_and($kinds)
                    && !state.notifications.mute_state(guild_of(app, target.0).flatten(), Some(target.0), now_unix()).on
            },
            kind: Kind::Submenu(&[&[Entry::Choices(Choices {
                name: concat!("mute-", $name, "-for"),
                choices: |_, _| durations(),
                style: Style::Plain,
                choose: |app, target, value, _| {
                    if let Some(settings) = mute_for(now_unix(), value) {
                        app.save_channel_settings(target.0, settings);
                    }
                },
            })]]),
        });
        pub(super) const $unmute: Entry<ChannelTarget> = Entry::Item(Item {
            name: concat!("unmute-", $name),
            label: $unmute_label,
            shows: |app, target| {
                kind_of(app, target.0).is_some_and($kinds)
                    && app
                        .state
                        .borrow()
                        .notifications
                        .mute_state(guild_of(app, target.0).flatten(), Some(target.0), now_unix())
                        .on
            },
    kind: Kind::Action(|app, target| app.save_channel_settings(target.0, json!({"muted": false}))),
        });
    };
}

mute_items!(MUTE_CHANNEL, UNMUTE_CHANNEL, "channel", "Mute _Channel", "Unmute _Channel", |k| !matches!(
    k,
    ChannelKind::Category | ChannelKind::Dm | ChannelKind::GroupDm
));
mute_items!(MUTE_CATEGORY, UNMUTE_CATEGORY, "category", "Mute _Category", "Unmute _Category", |k| k
    == ChannelKind::Category);
mute_items!(
    MUTE_CONVERSATION,
    UNMUTE_CONVERSATION,
    "conversation",
    "Mute _Conversation",
    "Unmute _Conversation",
    |k| k.is_private()
);

/// Notification Settings on a channel or category: its own level or the
/// default above it (`288104`). Voice channels, forums and media channels
/// have other settings, which Neucord doesn't offer.
pub(super) const CHANNEL_NOTIFICATIONS: Entry<ChannelTarget> = Entry::Item(Item {
    name: "channel-notifications",
    label: "_Notification Settings",
    shows: |app, target| {
        guild_of(app, target.0).flatten().is_some()
            && kind_of(app, target.0).is_some_and(|k| !k.is_vocal() && !k.is_forum_like() && !k.is_private())
    },
    kind: Kind::Submenu(&[&[Entry::Choices(Choices {
        name: "channel-level",
        choices: |app, target| {
            let state = app.state.borrow();
            let parent = state.find_channel(target.0).is_some_and(|(_, c)| c.parent_id.is_some());
            let own = state.notifications.channel_level_raw(guild_of(app, target.0).flatten(), target.0);
            [
                (3, if parent { "Use Category Default" } else { "Use Server Default" }),
                (0, "All Messages"),
                (1, "Only @mentions"),
                (2, "Nothing"),
            ]
            .map(|(value, label)| Choice { value: value.to_string(), label: label.into(), ticked: own == value })
            .into()
        },
        style: Style::Radio,
        choose: |app, target, value, _| {
            if let Ok(level) = value.parse::<u8>() {
                app.save_channel_settings(target.0, json!({"message_notifications": level}));
            }
        },
    })]]),
});

/// Mark As Read on a server icon (`871638`).
pub(super) const MARK_GUILD_READ: Entry<GuildTarget> = Entry::Item(Item {
    name: "mark-guild-read",
    label: "Mark As _Read",
    shows: |app, target| {
        let state = app.state.borrow();
        state.guilds.iter().find(|g| g.id == target.0).is_some_and(|g| !state.guild_acks(g).is_empty())
    },
    kind: Kind::Action(|app, target| app.mark_guild_read(target.0)),
});

pub(super) const MUTE_GUILD: Entry<GuildTarget> = Entry::Item(Item {
    name: "mute-guild",
    label: "Mute _Server",
    shows: |app, target| !app.state.borrow().notifications.mute_state(Some(target.0), None, now_unix()).on,
    kind: Kind::Submenu(&[&[Entry::Choices(Choices {
        name: "mute-guild-for",
        choices: |_, _| durations(),
        style: Style::Plain,
        choose: |app, target, value, _| {
            if let Some(settings) = mute_for(now_unix(), value) {
                app.save_notification_settings(Some(target.0), settings);
            }
        },
    })]]),
});

pub(super) const UNMUTE_GUILD: Entry<GuildTarget> = Entry::Item(Item {
    name: "unmute-guild",
    label: "Unmute _Server",
    shows: |app, target| app.state.borrow().notifications.mute_state(Some(target.0), None, now_unix()).on,
    kind: Kind::Action(|app, target| app.save_notification_settings(Some(target.0), json!({"muted": false}))),
});

/// A server's level and the switches under it (`871638`). Mute New Events and
/// the client's other items there need what Neucord doesn't have.
pub(super) const GUILD_NOTIFICATIONS: Entry<GuildTarget> = Entry::Item(Item {
    name: "guild-notifications",
    label: "_Notification Settings",
    shows: |_, _| true,
    kind: Kind::Submenu(&[
        &[Entry::Choices(Choices {
            name: "guild-level",
            choices: |app, target| {
                let state = app.state.borrow();
                let default =
                    state.guilds.iter().find(|g| g.id == target.0).map_or(0, |g| g.default_message_notifications);
                let raw = state.notifications.guild_level_raw(Some(target.0));
                let level = if raw == 3 { default } else { raw };
                [(0, "All Messages"), (1, "Only @mentions"), (2, "Nothing")]
                    .map(|(value, label)| Choice {
                        value: value.to_string(),
                        label: label.into(),
                        ticked: level == value,
                    })
                    .into()
            },
            style: Style::Radio,
            choose: |app, target, value, _| {
                if let Ok(level) = value.parse::<u8>() {
                    app.save_notification_settings(Some(target.0), json!({"message_notifications": level}));
                }
            },
        })],
        &[
            Entry::Item(Item {
                name: "suppress-everyone",
                label: "Suppress @everyone and @here",
                shows: |_, _| true,
                kind: Kind::Check {
                    checked: |app, target| app.state.borrow().notifications.suppress_everyone(target.0),
                    set: |app, target, on| {
                        app.save_notification_settings(Some(target.0), json!({"suppress_everyone": on}))
                    },
                },
            }),
            Entry::Item(Item {
                name: "suppress-roles",
                label: "Suppress All Role @mentions",
                shows: |_, _| true,
                kind: Kind::Check {
                    checked: |app, target| app.state.borrow().notifications.suppress_roles(target.0),
                    set: |app, target, on| {
                        app.save_notification_settings(Some(target.0), json!({"suppress_roles": on}))
                    },
                },
            }),
            Entry::Item(Item {
                name: "suppress-highlights",
                label: "Suppress Highlights",
                shows: |_, _| true,
                kind: Kind::Check {
                    checked: |app, target| app.state.borrow().notifications.suppress_highlights(target.0),
                    // Highlights go by `notify_highlights`: 1 disabled, 2 enabled (the client's `guM`).
                    set: |app, target, on| {
                        app.save_notification_settings(
                            Some(target.0),
                            json!({"notify_highlights": if on { 1 } else { 2 }}),
                        )
                    },
                },
            }),
        ],
        &[Entry::Item(Item {
            name: "mobile-push",
            label: "Mobile Push Notifications",
            shows: |_, _| true,
            kind: Kind::Check {
                checked: |app, target| app.state.borrow().notifications.mobile_push(target.0),
                set: |app, target, on| app.save_notification_settings(Some(target.0), json!({"mobile_push": on})),
            },
        })],
    ]),
});

/// Mark Unread on a message: the channel reads up to the message before it
/// (`20883`, `439147`), where the client lets us post in it.
pub(super) const MARK_UNREAD: Entry<MessageTarget> = Entry::Item(Item {
    name: "mark-unread",
    label: "Mark _Unread",
    shows: |app, target| app.offers_mark_unread(target),
    kind: Kind::Action(|app, target| app.mark_unread(target)),
});
