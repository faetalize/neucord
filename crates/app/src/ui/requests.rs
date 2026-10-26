//! Message requests: DMs from people we share no friendship with, which
//! Discord holds back (or files as spam) until we accept them. Home lists
//! them on their own page; opening one shows its messages with accept and
//! ignore in place of the composer.

use std::rc::Rc;

use discord::Snowflake;
use gtk::glib;
use gtk::prelude::*;

use super::App;
use super::sidebar::dm_row;
use super::state::Page;

pub(super) struct RequestsPage {
    pub(super) root: gtk::Box,
    list: gtk::Box,
    error: gtk::Label,
}

impl RequestsPage {
    pub(super) fn new() -> Self {
        RequestsPage {
            root: gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(true)
                .css_classes(["chat"])
                .build(),
            list: gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["friends-list"]).build(),
            error: gtk::Label::builder().xalign(0.0).wrap(true).visible(false).css_classes(["composer-error"]).build(),
        }
    }
}

impl App {
    pub(super) fn build_requests(&self) -> gtk::Widget {
        let page = &self.requests;
        page.root
            .append(&gtk::Label::builder().label("Message Requests").xalign(0.0).css_classes(["chat-header"]).build());
        page.root.append(&page.error);
        page.root.append(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&page.list)
                .build(),
        );
        page.root.clone().upcast()
    }

    pub(super) fn show_requests(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            state.guild = None;
            state.channel = None;
            state.page = Some(Page::MessageRequests);
        }
        self.requests.error.set_visible(false);
        self.content.set_visible_child(&self.requests.root);
        self.rebuild_rail();
        self.rebuild_sidebar();
        self.rebuild_requests();
    }

    pub(super) fn rebuild_requests(self: &Rc<Self>) {
        let list = &self.requests.list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let state = self.state.borrow();
        let (spam, requests): (Vec<_>, Vec<_>) = state.requests().into_iter().partition(|c| c.spam);
        if requests.is_empty() && spam.is_empty() {
            list.append(
                &gtk::Label::builder()
                    .label("No message requests. When someone you don't know messages you, it shows up here.")
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["hint", "friends-empty"])
                    .build(),
            );
        }
        let sections = [
            ("REQUESTS", requests, "Their messages arrive here until you accept."),
            ("SPAM", spam, "Discord filed these as likely spam."),
        ];
        for (heading, channels, hint) in sections {
            if channels.is_empty() {
                continue;
            }
            list.append(
                &gtk::Label::builder()
                    .label(format!("{heading} — {}", channels.len()))
                    .xalign(0.0)
                    .css_classes(["category"])
                    .build(),
            );
            list.append(&gtk::Label::builder().label(hint).xalign(0.0).css_classes(["hint", "section-hint"]).build());
            for channel in channels {
                let row = gtk::Box::builder().spacing(8).css_classes(["friend"]).build();
                let open = gtk::Button::builder()
                    .child(&dm_row(channel, &self.images))
                    .hexpand(true)
                    .tooltip_text("Read the messages")
                    .css_classes(["channel", "dm"])
                    .build();
                let id = channel.id;
                open.connect_clicked({
                    let this = Rc::downgrade(self);
                    move |_| {
                        if let Some(this) = this.upgrade() {
                            this.select_channel(id);
                        }
                    }
                });
                row.append(&open);
                self.append_request_actions(&row, id);
                list.append(&row);
            }
        }
    }

    /// The bar that replaces the composer while a message request is open.
    pub(super) fn update_request_bar(self: &Rc<Self>) {
        let bar = &self.request_bar;
        while let Some(child) = bar.first_child() {
            bar.remove(&child);
        }
        let request = self.state.borrow().open_request().map(|c| (c.id, c.spam, c.title()));
        self.composer_box.set_visible(request.is_none());
        bar.set_visible(request.is_some());
        let Some((id, spam, title)) = request else { return };
        let text = if spam {
            format!("Discord filed this message request from {title} as likely spam.")
        } else {
            format!("{title} wants to message you. Accept to reply.")
        };
        bar.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).hexpand(true).build());
        self.append_request_actions(bar, id);
    }

    fn append_request_actions(self: &Rc<Self>, row: &gtk::Box, channel: Snowflake) {
        for (label, accept) in [("Accept", true), ("Ignore", false)] {
            let button =
                gtk::Button::builder().label(label).valign(gtk::Align::Center).css_classes(["row-action"]).build();
            if accept {
                button.add_css_class("primary");
            }
            let this = Rc::downgrade(self);
            button.connect_clicked(move |button| {
                if let Some(this) = this.upgrade() {
                    button.set_sensitive(false);
                    this.answer_request(channel, accept);
                }
            });
            row.append(&button);
        }
    }

    /// Accept or ignore a message request. The result is applied at once;
    /// the gateway confirms it with CHANNEL_UPDATE or CHANNEL_DELETE.
    fn answer_request(self: &Rc<Self>, channel: Snowflake, accept: bool) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = if accept {
                rest.accept_message_request(channel).await.map(|c| this.on_private_channel(c))
            } else {
                rest.reject_message_request(channel).await.map(|()| this.on_private_channel_delete(channel))
            };
            if let Err(e) = result {
                let text = format!("Could not {} the request: {e}", if accept { "accept" } else { "ignore" });
                if this.state.borrow().page == Some(Page::MessageRequests) {
                    this.requests.error.set_label(&text);
                    this.requests.error.set_visible(true);
                    this.rebuild_requests();
                } else {
                    this.show_composer_error(&text);
                    this.update_request_bar();
                }
            }
        });
    }
}
