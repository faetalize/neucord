//! Desktop notifications, and opening a channel from one.

use std::rc::Rc;
use std::time::Duration;

use discord::notifications::Place;
use discord::{Message, Snowflake};
use gtk::glib;
use gtk::prelude::*;

use super::state::now_ms;
use super::uploads::Outgoing;
use super::{App, friends};
use crate::{desktop, render};

/// How long a notification waits for the sender's picture before it shows without.
const ICON_WAIT: Duration = Duration::from_secs(3);

impl App {
    /// The desktop's notification server, connected on first use.
    pub(super) fn desktop(self: &Rc<Self>) -> Option<&desktop::Desktop> {
        self.desktop
            .get_or_init(|| {
                let bus = self.window.application()?.dbus_connection()?;
                let this = Rc::downgrade(self);
                Some(desktop::Desktop::new(bus, crate::APP_ID, "Neucord", move |key, event| {
                    if let Some(this) = this.upgrade() {
                        this.on_notification_event(key, event);
                    }
                }))
            })
            .as_ref()
    }

    pub(super) fn send_notification(self: &Rc<Self>, message: &Message, place: &Place) {
        if !self.notifications_allowed() {
            return;
        }
        let (title, body) = {
            let state = self.state.borrow();
            let author = &state.name_in(message.channel_id, &message.author);
            let title = match state.find_channel(message.channel_id) {
                Some((Some(guild), channel)) => {
                    format!("{author} (#{}, {})", channel.name.as_deref().unwrap_or("unnamed"), guild.name)
                }
                Some((None, dm)) if dm.kind == discord::ChannelKind::GroupDm => format!("{author} ({})", dm.title()),
                _ => author.to_owned(),
            };
            drop(state);
            let names = self.message_names(message);
            // A forward reads as what it forwards.
            let content = message.forwarded().map_or_else(|| message.content.clone(), |f| f.content);
            let mut body = render::plain_text(&discord::markup::parse(&content), &names);
            if body.is_empty() && !message.attachments.is_empty() {
                body = "Sent an attachment".into();
            } else if body.is_empty() && !message.sticker_items.is_empty() {
                body = "Sent a sticker".into();
            }
            if body.chars().count() > 200 {
                body = body.chars().take(200).chain(std::iter::once('…')).collect();
            }
            (title, body)
        };
        let icon =
            message.author.avatar.as_deref().map(|hash| discord::cdn::avatar_url(message.author.id.0, hash, 128));
        let channel = place.channel;
        let this = self.clone();
        glib::spawn_future_local(async move {
            // The sender's picture, as the client shows it; without it if it doesn't come.
            let texture = match icon {
                Some(url) => glib::future_with_timeout(ICON_WAIT, this.images.texture(&url)).await.ok().flatten(),
                None => None,
            };
            // Read, or looked at, while the picture loaded: nothing to say any more.
            if !this.notifications_allowed() || !this.has_news(channel) {
                return;
            }
            if let Some(desktop) = this.desktop() {
                // Sounds are on, as in the client by default (#86 brings the switch).
                let notice = desktop::Notice {
                    key: &notification_id(channel),
                    title: &title,
                    body: &body,
                    icon: texture.as_ref(),
                    sound: true,
                    reply: true,
                };
                desktop.show(&notice);
            }
        });
    }

    /// Whether `channel` has something unread or a mention waiting.
    fn has_news(&self, channel: Snowflake) -> bool {
        let state = self.state.borrow();
        state.find_channel(channel).is_some_and(|(_, c)| state.is_unread(c)) || state.read.mentions(channel) > 0
    }

    pub(super) fn notify_friend_request(self: &Rc<Self>, request: &discord::relationships::Relationship) {
        if !self.notifications_allowed() {
            return;
        }
        let user = request.user.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let icon = user.avatar.as_deref().map(|hash| discord::cdn::avatar_url(user.id.0, hash, 128));
            let texture = match icon {
                Some(url) => glib::future_with_timeout(ICON_WAIT, this.images.texture(&url)).await.ok().flatten(),
                None => None,
            };
            if let Some(desktop) = this.desktop() {
                // The client plays no sound for a friend request.
                let notice = desktop::Notice {
                    key: &friend_request_id(user.id),
                    title: user.display_name(),
                    body: "Sent you a friend request",
                    icon: texture.as_ref(),
                    sound: false,
                    reply: false,
                };
                desktop.show(&notice);
            }
        });
    }

    /// Whether anything may notify us. Not while our status is Do Not
    /// Disturb (`eA` in 534765, and `K` in 411238 for messages), which also
    /// keeps their sound. Not while Discord requires a step: the official
    /// client stays quiet then too, and a notification would lead nowhere.
    fn notifications_allowed(&self) -> bool {
        let state = self.state.borrow();
        state.required_action.is_none() && !state.do_not_disturb
    }

    pub(super) fn withdraw_friend_request(self: &Rc<Self>, user: Snowflake) {
        if let Some(desktop) = self.desktop() {
            desktop.withdraw(&friend_request_id(user));
        }
    }

    pub(super) fn withdraw_notification(self: &Rc<Self>, channel: Snowflake) {
        if let Some(desktop) = self.desktop() {
            desktop.withdraw(&notification_id(channel));
        }
    }

    fn on_notification_event(self: &Rc<Self>, key: &str, event: desktop::Event) {
        let channel = key.strip_prefix(CHANNEL_PREFIX).and_then(|id| id.parse().ok()).map(Snowflake);
        match (channel, event) {
            (Some(channel), desktop::Event::Opened) => self.open_channel(channel),
            (Some(channel), desktop::Event::Replied(text)) => self.reply_from_notification(channel, text),
            (None, desktop::Event::Opened) => self.open_friend_requests(),
            (None, desktop::Event::Replied(_)) => {}
        }
    }

    /// The friends page, at the requests we have.
    fn open_friend_requests(self: &Rc<Self>) {
        self.window.present();
        self.state.borrow_mut().friends_tab = friends::Tab::List(discord::relationships::List::Pending);
        self.show_friends();
        self.show_content();
    }

    /// What was typed into a notification, sent to its channel as the message
    /// box would, and under the same rules: where the box isn't offered (#51),
    /// or slowmode or the length refuse it, nothing goes out and the
    /// notification says so.
    fn reply_from_notification(self: &Rc<Self>, channel: Snowflake, typed: String) {
        let typed = typed.trim().to_owned();
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest.filter(|_| !typed.is_empty()) else { return };
        // `:name:`s go out as emoji, and the length is checked on what goes out.
        let text = self.emoji_library(channel).translate(&typed).text;
        let refusal = {
            let state = self.state.borrow();
            if state.box_blocked(channel) || !state.can_send(channel) {
                Some("You can't send messages in this channel.".to_owned())
            } else if let Err(too_long) = state.limits().check_length(&text) {
                Some(too_long.heading().to_owned())
            } else if state.find_channel(channel).is_some_and(|(_, c)| state.slowmode.remaining_ms(c, now_ms()) > 0) {
                Some("Slowmode is enabled in this channel.".to_owned())
            } else {
                None
            }
        };
        if let Some(refusal) = refusal {
            return self.reply_failed(channel, &refusal);
        }
        self.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
        self.typing_message_sent(channel);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let message = Outgoing { text: &text, reply: None, sticker: None, at: None };
            match this.send_with_uploads(&rest, channel, &message, Vec::new()).await {
                Ok(()) => {
                    this.message_sent(channel);
                    this.withdraw_notification(channel);
                }
                Err(stopped) => {
                    if let Some(why) = this.record_stopped(channel, &stopped) {
                        this.reply_failed(channel, &why);
                    }
                }
            }
        });
    }

    /// Say a reply didn't go out, in the channel's notification, which still takes one.
    fn reply_failed(self: &Rc<Self>, channel: Snowflake, why: &str) {
        let title = {
            let state = self.state.borrow();
            let name = state.find_channel(channel).map(|(_, c)| c.title());
            format!("Reply not sent{}", name.map(|n| format!(" to {n}")).unwrap_or_default())
        };
        if let Some(desktop) = self.desktop() {
            let notice = desktop::Notice {
                key: &notification_id(channel),
                title: &title,
                body: why,
                icon: None,
                sound: false,
                reply: true,
            };
            desktop.show(&notice);
        }
    }

    /// Jump to any channel we can view, DM or server (from a notification
    /// click, or a forward's footer).
    pub(super) fn open_channel(self: &Rc<Self>, id: Snowflake) {
        let found = {
            let mut state = self.state.borrow_mut();
            let guild = state.find_channel(id).filter(|_| state.can_view(id)).map(|(guild, _)| guild.map(|g| g.id));
            if let Some(guild) = guild {
                state.guild = guild;
            }
            guild.is_some()
        };
        self.window.present();
        if found {
            self.rebuild_rail();
            self.select_channel(id);
            self.show_content();
        }
    }
}

const CHANNEL_PREFIX: &str = "channel-";

/// One notification per channel: a newer message replaces the older one.
pub(super) fn notification_id(channel: Snowflake) -> String {
    format!("{CHANNEL_PREFIX}{channel}")
}

pub(super) fn friend_request_id(user: Snowflake) -> String {
    format!("friend-request-{user}")
}
