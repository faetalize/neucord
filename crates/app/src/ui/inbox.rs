//! The inbox's Mentions tab, in a popover from the header (713024): the
//! messages that mention us, newest first, each with Jump and a way to mark
//! it done, and what it includes (@everyone, @role, this server). Its list
//! stays current from the gateway (`RecentMentionsStore`, 849171); closing it
//! keeps the newest page.

use std::rc::Rc;

use adw::prelude::*;
use discord::mentions::Filter;
use discord::search::Context;
use discord::{Message, Snowflake};
use gtk::glib;

use super::App;
use crate::config;

/// How close to the end a scroll asks for the next page, in pixels.
const NEAR_END: f64 = 250.0;

/// The popover's parts.
pub(super) struct InboxView {
    popover: gtk::Popover,
    list: gtk::Box,
}

impl InboxView {
    pub(super) fn new(app: &Rc<App>) -> InboxView {
        let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(420)
            .min_content_height(120)
            .max_content_height(480)
            .propagate_natural_height(true)
            .build();
        scroller.vadjustment().connect_value_changed({
            let app = Rc::downgrade(app);
            move |adjustment| {
                let Some(app) = app.upgrade() else { return };
                if adjustment.upper() - adjustment.value() - adjustment.page_size() < NEAR_END {
                    app.more_mentions();
                }
            }
        });

        let title = gtk::Label::builder().label("Mentions").css_classes(["heading"]).hexpand(true).xalign(0.0).build();
        let filters = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Filters")
            .css_classes(["flat"])
            .popover(&filter_popover(app))
            .build();
        let top = gtk::Box::builder().margin_top(6).margin_bottom(6).margin_start(12).margin_end(6).build();
        top.append(&title);
        top.append(&filters);
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        column.append(&top);
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        column.append(&scroller);

        let popover = gtk::Popover::builder().child(&column).build();
        popover.connect_show({
            let app = Rc::downgrade(app);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.inbox_opened();
                }
            }
        });
        popover.connect_closed({
            let app = Rc::downgrade(app);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.account.state.borrow_mut().mentions.truncate();
                }
            }
        });
        InboxView { popover, list }
    }

    pub(super) fn popover(&self) -> &gtk::Popover {
        &self.popover
    }
}

/// What the mentions include.
fn filter_popover(app: &Rc<App>) -> gtk::Popover {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(9)
        .margin_bottom(9)
        .margin_start(9)
        .margin_end(9)
        .build();
    let option = |label: &str, key: &'static config::Key| {
        let check = gtk::CheckButton::builder().label(label).active(app.remembered.get(key)).build();
        check.connect_toggled({
            let app = Rc::downgrade(app);
            move |check| {
                let Some(app) = app.upgrade() else { return };
                if let Err(e) = app.remembered.set(key, check.is_active()) {
                    eprintln!("config: {e}");
                }
                app.mention_filter_changed();
            }
        });
        column.append(&check);
    };
    option("Include @everyone mentions", &config::INBOX_EVERYONE);
    option("Include @role mentions", &config::INBOX_ROLES);
    option("Only this server", &config::INBOX_THIS_SERVER);
    gtk::Popover::builder().child(&column).build()
}

impl App {
    /// The filter as the options file has it, now.
    fn current_mention_filter(&self) -> Filter {
        Filter {
            everyone: self.remembered.get(&config::INBOX_EVERYONE),
            roles: self.remembered.get(&config::INBOX_ROLES),
            this_server: self.remembered.get(&config::INBOX_THIS_SERVER),
        }
    }

    /// A box of the filter was ticked (`SET_RECENT_MENTIONS_FILTER`): a
    /// narrower one keeps what still passes, and the list is asked for again
    /// if that leaves nothing; a wider one starts over.
    fn mention_filter_changed(self: &Rc<Self>) {
        let now = self.current_mention_filter();
        {
            let mut state = self.account.state.borrow_mut();
            let before = state.mention_filter;
            let narrower = (!before.this_server && now.this_server)
                || (before.everyone && !now.everyone)
                || (before.roles && !now.roles);
            state.mention_filter = now;
            let (me, open) = (state.me(), state.guild);
            let keep: Vec<Snowflake> = state
                .mentions
                .list()
                .iter()
                .filter(|m| {
                    let found = state.find_channel(m.channel_id);
                    let guild = found.and_then(|(g, _)| g);
                    let roles = guild.map_or(&[][..], |g| g.my_roles.as_slice());
                    (!now.this_server || guild.map(|g| g.id) == open)
                        && me
                            .is_some_and(|me| discord::notifications::mentions(m, me, roles, !now.everyone, !now.roles))
                })
                .map(|m| m.id)
                .collect();
            state.mentions.refilter(narrower, |m| keep.contains(&m.id));
        }
        self.inbox_opened();
    }

    /// The popover opened, or the filter changed: ask for the first page if
    /// there is none (the client asks again only then, or for a changed
    /// filter).
    fn inbox_opened(self: &Rc<Self>) {
        let first = {
            let mut state = self.account.state.borrow_mut();
            // "This server" with no server open is all of them (`LOAD_RECENT_MENTIONS`).
            if state.mention_filter.this_server && state.guild.is_none() {
                state.mention_filter.this_server = false;
                drop(state);
                if let Err(e) = self.remembered.set(&config::INBOX_THIS_SERVER, false) {
                    eprintln!("config: {e}");
                }
                self.account.state.borrow().mentions.needs_first_page()
            } else {
                state.mentions.needs_first_page()
            }
        };
        if first {
            self.fetch_mentions(None);
        }
        self.show_mentions();
    }

    /// The next page, from the end of the list.
    pub(super) fn more_mentions(self: &Rc<Self>) {
        let (wants, before) = {
            let state = self.account.state.borrow();
            (state.mentions.wants_more() && state.mentions.has_more(), state.mentions.before())
        };
        if wants && before.is_some() {
            self.fetch_mentions(before);
        }
    }

    fn fetch_mentions(self: &Rc<Self>, before: Option<Snowflake>) {
        let (rest, guild, filter) = {
            let mut state = self.account.state.borrow_mut();
            let Some(rest) = state.rest.clone() else { return };
            if state.mentions.loading() {
                return;
            }
            state.mentions.started();
            let guild = state.mention_filter.this_server.then_some(state.guild).flatten();
            (rest, guild, state.mention_filter)
        };
        self.show_mentions();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.mentions(before, guild, filter).await;
            {
                let mut state = this.account.state.borrow_mut();
                match result {
                    Ok(page) => state.mentions.loaded(page, before.is_some()),
                    Err(e) => {
                        eprintln!("mentions: {e}");
                        state.mentions.failed();
                    }
                }
            }
            this.show_mentions();
        });
    }

    /// Fill the popover in from the list, if it is open.
    pub(super) fn show_mentions(self: &Rc<Self>) {
        let view = self.inbox_view.borrow();
        let Some(view) = view.as_ref().filter(|v| v.popover.is_visible()) else { return };
        while let Some(child) = view.list.first_child() {
            view.list.remove(&child);
        }
        let (items, loading, more) = {
            let state = self.account.state.borrow();
            // A message whose channel we don't know has no row, as in the client.
            let items: Vec<(Message, Option<Snowflake>)> = state
                .mentions
                .list()
                .iter()
                .filter_map(|m| Some((m.clone(), state.find_channel(m.channel_id)?.0.map(|g| g.id))))
                .collect();
            (items, state.mentions.loading(), state.mentions.has_more())
        };
        for (message, guild) in &items {
            view.list.append(&self.mention_row(message, *guild));
        }
        if loading {
            view.list.append(&adw::Spinner::builder().margin_top(24).margin_bottom(24).build());
        } else if items.is_empty() {
            view.list.append(
                &gtk::Label::builder()
                    .label("No mentions yet...\n\nWhen someone mentions you, you'll see it here.")
                    .justify(gtk::Justification::Center)
                    .margin_top(24)
                    .margin_bottom(24)
                    .css_classes(["dimmed"])
                    .build(),
            );
        } else if more {
            let button = gtk::Button::builder()
                .label("Load more messages")
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .build();
            let this = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.more_mentions();
                }
            });
            view.list.append(&button);
        }
    }

    /// One mention: the message, then Jump and Mark as done.
    fn mention_row(self: &Rc<Self>, message: &Message, guild: Option<Snowflake>) -> gtk::ListBoxRow {
        let actions = gtk::Box::builder().spacing(6).halign(gtk::Align::End).margin_top(4).build();
        let jump = gtk::Button::builder().label("Jump").css_classes(["flat"]).build();
        jump.connect_clicked({
            let (this, channel, id) = (Rc::downgrade(self), message.channel_id, message.id);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                if let Some(view) = this.inbox_view.borrow().as_ref() {
                    view.popover.popdown();
                }
                this.open_channel_at(channel, Some(id));
            }
        });
        let done = gtk::Button::builder()
            .icon_name("object-select-symbolic")
            .tooltip_text("Mark as read")
            .css_classes(["flat"])
            .build();
        done.connect_clicked({
            let (this, id) = (Rc::downgrade(self), message.id);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.mention_done(id);
                }
            }
        });
        actions.append(&jump);
        actions.append(&done);
        let context = guild.map_or(Context::Channel(message.channel_id), Context::Guild);
        self.message_summary_row(message, context, Some(actions.upcast_ref()))
    }

    /// Take a mention off the inbox: it goes from the list at once, and the
    /// request follows (`deleteRecentMention`).
    pub(super) fn mention_done(self: &Rc<Self>, id: Snowflake) {
        let rest = {
            let mut state = self.account.state.borrow_mut();
            state.mentions.remove(id);
            state.rest.clone()
        };
        self.show_mentions();
        if let Some(rest) = rest {
            glib::spawn_future_local(async move {
                if let Err(e) = rest.delete_mention(id).await {
                    eprintln!("mentions: {e}");
                }
            });
        }
    }
}
