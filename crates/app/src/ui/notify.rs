//! Desktop notifications, and opening a channel from one.

use std::rc::Rc;

use discord::notifications::Place;
use discord::{Message, Snowflake};
use gtk::gio;
use gtk::prelude::*;

use super::App;
use crate::render;

impl App {
    pub(super) fn send_notification(&self, message: &Message, place: &Place) {
        let Some(app) = self.window.application().filter(|_| self.notifications_allowed()) else { return };
        let (title, body) = {
            let state = self.state.borrow();
            let author = message.author.display_name();
            let title = match state.find_channel(message.channel_id) {
                Some((Some(guild), channel)) => {
                    format!("{author} (#{}, {})", channel.name.as_deref().unwrap_or("unnamed"), guild.name)
                }
                Some((None, dm)) if dm.kind == discord::ChannelKind::GroupDm => format!("{author} ({})", dm.title()),
                _ => author.to_owned(),
            };
            drop(state);
            let names = self.message_names(message);
            let mut body = render::plain_text(&discord::markup::parse(&message.content), &names);
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
        let notification = gio::Notification::new(&title);
        notification.set_body(Some(&body));
        notification
            .set_default_action_and_target_value("app.open-channel", Some(&place.channel.to_string().to_variant()));
        app.send_notification(Some(&notification_id(place.channel)), &notification);
    }

    pub(super) fn notify_friend_request(&self, request: &discord::relationships::Relationship) {
        let Some(app) = self.window.application().filter(|_| self.notifications_allowed()) else { return };
        let notification = gio::Notification::new(request.user.display_name());
        notification.set_body(Some("Sent you a friend request"));
        notification.set_default_action("app.open-friends");
        app.send_notification(Some(&friend_request_id(request.user.id)), &notification);
    }

    /// The request was answered, here or elsewhere.
    /// Not while Discord requires a step: the official client stays quiet
    /// then too, and a notification would lead nowhere.
    fn notifications_allowed(&self) -> bool {
        self.state.borrow().required_action.is_none()
    }

    pub(super) fn withdraw_friend_request(&self, user: Snowflake) {
        if let Some(app) = self.window.application() {
            app.withdraw_notification(&friend_request_id(user));
        }
    }

    pub(super) fn withdraw_notification(&self, channel: Snowflake) {
        if let Some(app) = self.window.application() {
            app.withdraw_notification(&notification_id(channel));
        }
    }

    /// Jump to any channel, DM or server (from a notification click).
    pub(super) fn open_channel(self: &Rc<Self>, id: Snowflake) {
        let found = {
            let mut state = self.state.borrow_mut();
            let guild = state.find_channel(id).map(|(guild, _)| guild.map(|g| g.id));
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

/// One notification per channel: a newer message replaces the older one.
pub(super) fn notification_id(channel: Snowflake) -> String {
    format!("channel-{channel}")
}

pub(super) fn friend_request_id(user: Snowflake) -> String {
    format!("friend-request-{user}")
}
