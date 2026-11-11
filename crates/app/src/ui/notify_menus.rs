//! The context menu entries for what we hear of and what counts as read:
//! Mark As Read, Mute with its durations, Notification Settings, and Mark
//! Unread, on servers, folders, channels, categories, threads, DMs and group
//! DMs.
//!
//! Settings go out as the client sends them (see `discord::notification_edit`)
//! and apply here at once, as the client's do; a thread's come back from
//! Discord as an event, as in the client. Items it greys out are greyed.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use discord::notification_edit::{self, MUTE_FOR, THREAD_LEVELS};
use discord::{ChannelKind, Snowflake};
use serde_json::{Value, json};

use super::App;
use super::chat::MessageTarget;
use super::menus::{Choice, Choices, Entry, Item, Kind, Style};
use super::sidebar::ChannelTarget;
use super::state::{ReadItem, now_unix};
use super::threads::ThreadTarget;

/// What a server icon's menu acts on.
pub(super) struct GuildTarget(pub(super) Snowflake);

/// What a folder's menu acts on: the servers in it.
pub(super) struct FolderTarget(pub(super) Vec<Snowflake>);

/// How long after the last fold the client saves it (`u` in 931260).
const FOLD_SAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(15);

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
        let (rest, folds) = {
            let mut state = self.state.borrow_mut();
            state.notifications.apply(guild, &settings);
            // A server's request carries the folds not yet saved, and the timer for them is done with.
            let folds = if guild.is_some() { state.unsaved_folds() } else { Vec::new() };
            if guild.is_some() {
                state.collapsed.saving();
                if let Some(timer) = self.fold_save.take() {
                    timer.remove();
                }
            }
            (state.rest.clone(), folds)
        };
        self.refresh_badges();
        let Some(rest) = rest else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = rest.save_notification_settings(guild, settings, &folds).await {
                eprintln!("notification settings: {e}");
            }
        });
    }

    /// Fold a category, or unfold it. The client saves that fifteen seconds
    /// after the last change, unless a settings request goes sooner and takes
    /// it along (931260).
    pub(super) fn fold_category(self: &Rc<Self>, category: Snowflake, folded: bool) {
        if !self.state.borrow_mut().collapsed.set(category, folded) {
            return;
        }
        self.refresh_badges();
        if let Some(timer) = self.fold_save.take() {
            timer.remove();
        }
        let this = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(FOLD_SAVE_AFTER, move || {
            let Some(this) = this.upgrade() else { return };
            this.fold_save.take();
            let (folds, rest) = {
                let mut state = this.state.borrow_mut();
                let folds = state.unsaved_folds();
                state.collapsed.saving();
                (folds, state.rest.clone())
            };
            let Some(rest) = rest.filter(|_| !folds.is_empty()) else { return };
            glib::spawn_future_local(async move {
                if let Err(e) = rest.save_folds(&folds).await {
                    eprintln!("category folds: {e}");
                }
            });
        });
        self.fold_save.replace(Some(timer));
    }

    /// Change our mute or level in a thread. The client applies nothing here:
    /// Discord answers with our thread member, which the gateway brings.
    fn save_thread_settings(self: &Rc<Self>, thread: Snowflake, settings: Value) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        glib::spawn_future_local(async move {
            if let Err(e) = rest.save_thread_settings(thread, settings).await {
                eprintln!("thread settings {thread}: {e}");
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

    /// Mark servers read, a server or a folder's: what they hold unread, in
    /// bulk acks.
    fn mark_guilds_read(self: &Rc<Self>, guilds: &[Snowflake]) {
        let (acks, rest) = {
            let state = self.state.borrow();
            let acks: Vec<_> =
                state.guilds.iter().filter(|g| guilds.contains(&g.id)).flat_map(|g| state.guild_acks(g)).collect();
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
        self.mark_unread_at(target.message.channel_id, target.message.id);
    }

    /// Mark Unread on a channel: from its newest message, as the item
    /// (511904) passes it to the same function (439147).
    fn mark_channel_unread(self: &Rc<Self>, channel: Snowflake) {
        let newest = self.state.borrow().find_channel(channel).and_then(|(_, c)| c.last_message_id);
        if let Some(message) = newest {
            self.mark_unread_at(channel, message);
        }
    }

    fn mark_unread_at(self: &Rc<Self>, channel: Snowflake, message: Snowflake) {
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

fn mute_for(value: &str) -> Option<Value> {
    let seconds = value.parse().ok()?;
    Some(notification_edit::mute(seconds, &glib::DateTime::now_utc().ok()?))
}

/// Mark As Read and Mark Unread (511904, one item for a channel, category,
/// thread and DM): Mark As Read greyed while nothing is unread, and, where
/// the experiment gives Mark Unread, that in its place on a channel with
/// nothing unread.
macro_rules! read_items {
    ($read:ident, $unread:ident, $target:ty) => {
        pub(super) const $read: Entry<$target> = Entry::Item(Item {
            name: "mark-channel-read",
            label: "Mark As _Read",
            shows: |app, target| matches!(app.state.borrow().read_item(target.0), Some(ReadItem::Read { .. })),
            kind: Kind::Gated {
                enabled: |app, target| {
                    matches!(app.state.borrow().read_item(target.0), Some(ReadItem::Read { enabled: true }))
                },
                run: |app, target| {
                    let targets = app.state.borrow().ack_targets(target.0);
                    app.mark_channels_read(targets);
                },
            },
        });
        pub(super) const $unread: Entry<$target> = Entry::Item(Item {
            name: "mark-channel-unread",
            label: "Mark _Unread",
            shows: |app, target| matches!(app.state.borrow().read_item(target.0), Some(ReadItem::Unread)),
            kind: Kind::Action(|app, target| app.mark_channel_unread(target.0)),
        });
    };
}

read_items!(MARK_CHANNEL_READ, MARK_CHANNEL_UNREAD, ChannelTarget);
read_items!(MARK_THREAD_READ, MARK_THREAD_UNREAD, ThreadTarget);

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
                    if let Some(settings) = mute_for(value) {
                        // Muting a category folds it as well (995102).
                        if kind_of(app, target.0) == Some(ChannelKind::Category) {
                            app.fold_category(target.0, true);
                        }
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

/// A joined thread's mute (57907): the items for a thread and for a forum
/// post differ in their words; a thread we haven't joined shows them greyed.
macro_rules! thread_mute_items {
    ($mute:ident, $unmute:ident, $off:ident, $name:literal, $post:expr, $mute_label:literal, $unmute_label:literal) => {
        pub(super) const $mute: Entry<ThreadTarget> = Entry::Item(Item {
            name: concat!("mute-", $name),
            label: $mute_label,
            shows: |app, target| {
                let state = app.state.borrow();
                state.is_forum_post(target.0) == $post
                    && state.thread_joined(target.0) == Some(true)
                    && !state.thread_is_muted(target.0)
            },
            kind: Kind::Submenu(&[&[Entry::Choices(Choices {
                name: concat!("mute-", $name, "-for"),
                choices: |_, _| durations(),
                style: Style::Plain,
                choose: |app, target, value, _| {
                    if let Some(settings) = mute_for(value) {
                        app.save_thread_settings(target.0, settings);
                    }
                },
            })]]),
        });
        pub(super) const $unmute: Entry<ThreadTarget> = Entry::Item(Item {
            name: concat!("unmute-", $name),
            label: $unmute_label,
            shows: |app, target| {
                let state = app.state.borrow();
                state.is_forum_post(target.0) == $post
                    && state.thread_joined(target.0) == Some(true)
                    && state.thread_is_muted(target.0)
            },
    kind: Kind::Action(|app, target| app.save_thread_settings(target.0, json!({"muted": false}))),
        });
        pub(super) const $off: Entry<ThreadTarget> = Entry::Item(Item {
            name: concat!("mute-", $name, "-unavailable"),
            label: $mute_label,
            shows: |app, target| {
                let state = app.state.borrow();
                state.is_forum_post(target.0) == $post && state.thread_joined(target.0) == Some(false)
            },
            kind: Kind::Gated { enabled: |_, _| false, run: |_, _| {} },
        });
    };
}

thread_mute_items!(MUTE_THREAD, UNMUTE_THREAD, MUTE_THREAD_OFF, "thread", false, "Mute _Thread", "Unmute _Thread");
thread_mute_items!(MUTE_POST, UNMUTE_POST, MUTE_POST_OFF, "post", true, "Mute _Post", "Unmute _Post");

/// A joined thread's Notification Settings (375500): which of its messages
/// notify, saved as the thread member's flags. A thread we haven't joined
/// shows the item greyed.
pub(super) const THREAD_NOTIFICATIONS: Entry<ThreadTarget> = Entry::Item(Item {
    name: "thread-notifications",
    label: "_Notification Settings",
    shows: |app, target| app.state.borrow().thread_joined(target.0) == Some(true),
    kind: Kind::Submenu(&[&[Entry::Choices(Choices {
        name: "thread-level",
        choices: |app, target| {
            let level = app.state.borrow().thread_notification_level(target.0);
            THREAD_LEVELS
                .map(|(flags, l, label)| Choice {
                    value: flags.to_string(),
                    label: label.into(),
                    ticked: level == Some(l),
                })
                .into()
        },
        style: Style::Radio,
        choose: |app, target, value, _| {
            if let Ok(flags) = value.parse::<u64>() {
                app.save_thread_settings(target.0, notification_edit::thread_level(flags));
            }
        },
    })]]),
});

pub(super) const THREAD_NOTIFICATIONS_OFF: Entry<ThreadTarget> = Entry::Item(Item {
    name: "thread-notifications-unavailable",
    label: "_Notification Settings",
    shows: |app, target| app.state.borrow().thread_joined(target.0) == Some(false),
    kind: Kind::Gated { enabled: |_, _| false, run: |_, _| {} },
});

/// What a channel's notification levels are called (288104): a stage's
/// "Only @mentions" is "All Live Stages", and a voice channel has none.
fn level_choices(app: &App, channel: Snowflake) -> Vec<Choice> {
    let state = app.state.borrow();
    let Some((guild, c)) = state.find_channel(channel) else { return Vec::new() };
    let own = state.notifications.channel_level_raw(guild.map(|g| g.id), channel);
    let default = if c.parent_id.is_some() { "Use Category Default" } else { "Use Server Default" };
    let levels: &[(u8, &str)] = if c.kind == ChannelKind::Stage {
        &[(3, default), (1, "All Live Stages"), (2, "Nothing")]
    } else {
        &[(3, default), (0, "All Messages"), (1, "Only @mentions"), (2, "Nothing")]
    };
    levels
        .iter()
        .map(|&(value, label)| Choice { value: value.to_string(), label: label.into(), ticked: own == value })
        .collect()
}

/// A forum or media channel's New Posts Created (288104): whether its new
/// posts notify, a flag of its override beside the level.
const NEW_POSTS: Entry<ChannelTarget> = Entry::Item(Item {
    name: "new-forum-posts",
    label: "New Posts Created",
    shows: |app, target| kind_of(app, target.0).is_some_and(ChannelKind::is_forum_like),
    kind: Kind::Check {
        checked: |app, target| {
            let state = app.state.borrow();
            state.place(target.0).is_some_and(|place| state.notifications.new_forum_posts(&place))
        },
        set: |app, target, on| {
            let flags = {
                let state = app.state.borrow();
                state
                    .place(target.0)
                    .and_then(|p| p.guild)
                    .map_or(0, |g| state.notifications.channel_flags(g, target.0))
            };
            app.save_channel_settings(target.0, notification_edit::new_posts(flags, on));
        },
    },
});

/// Notification Settings on a channel or category: its own level or the
/// default above it (`288104`), with New Posts Created on a forum. A voice
/// channel has none, and a stage a shorter list.
pub(super) const CHANNEL_NOTIFICATIONS: Entry<ChannelTarget> = Entry::Item(Item {
    name: "channel-notifications",
    label: "_Notification Settings",
    shows: |app, target| {
        guild_of(app, target.0).flatten().is_some()
            && kind_of(app, target.0).is_some_and(|k| k != ChannelKind::Voice && !k.is_private())
    },
    kind: Kind::Submenu(&[
        &[NEW_POSTS],
        &[Entry::Choices(Choices {
            name: "channel-level",
            choices: |app, target| level_choices(app, target.0),
            style: Style::Radio,
            choose: |app, target, value, _| {
                if let Ok(level) = value.parse::<u8>() {
                    app.save_channel_settings(target.0, json!({"message_notifications": level}));
                }
            },
        })],
    ]),
});

/// Mark As Read on a server icon (871638), greyed while nothing is unread.
pub(super) const MARK_GUILD_READ: Entry<GuildTarget> = Entry::Item(Item {
    name: "mark-guild-read",
    label: "Mark As _Read",
    shows: |_, _| true,
    kind: Kind::Gated {
        enabled: |app, target| {
            let state = app.state.borrow();
            state.guilds.iter().find(|g| g.id == target.0).is_some_and(|g| state.guild_has_news(g))
        },
        run: |app, target| app.mark_guilds_read(&[target.0]),
    },
});

/// Mark Folder As Read (865815): every server in the folder, greyed while
/// none has anything unread or a mention.
pub(super) const MARK_FOLDER_READ: Entry<FolderTarget> = Entry::Item(Item {
    name: "mark-folder-read",
    label: "Mark Folder As _Read",
    shows: |_, _| true,
    kind: Kind::Gated {
        enabled: |app, target| {
            let state = app.state.borrow();
            state.guilds.iter().filter(|g| target.0.contains(&g.id)).any(|g| state.guild_has_news(g))
        },
        run: |app, target| app.mark_guilds_read(&target.0),
    },
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
            if let Some(settings) = mute_for(value) {
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

/// A server's level and the switches under it (`871638`).
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
            Entry::Item(Item {
                name: "mute-events",
                label: "Mute New Events",
                shows: |_, _| true,
                kind: Kind::Check {
                    checked: |app, target| app.state.borrow().notifications.mute_scheduled_events(target.0),
                    set: |app, target, on| {
                        app.save_notification_settings(Some(target.0), json!({"mute_scheduled_events": on}))
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

/// Hide Muted Channels (153594): the sidebar leaves out the server's muted
/// channels. The client's own switch is local to the server's settings too,
/// saved with them as `hide_muted_channels`.
pub(super) const HIDE_MUTED_CHANNELS: Entry<GuildTarget> = Entry::Item(Item {
    name: "hide-muted-channels",
    label: "Hide Muted Channels",
    shows: |_, _| true,
    kind: Kind::Check {
        checked: |app, target| app.state.borrow().notifications.hide_muted_channels(target.0),
        set: |app, target, on| app.save_notification_settings(Some(target.0), json!({"hide_muted_channels": on})),
    },
});

/// Mark Unread on a message: the channel reads up to the message before it
/// (`20883`, `439147`), where the client lets us post in it.
pub(super) const MARK_UNREAD: Entry<MessageTarget> = Entry::Item(Item {
    name: "mark-unread",
    label: "Mark _Unread",
    shows: |app, target| app.offers_mark_unread(target),
    kind: Kind::Action(|app, target| app.mark_unread(target)),
});
