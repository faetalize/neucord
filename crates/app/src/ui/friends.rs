//! Home's friends page: friends and their presence, pending requests,
//! blocked users, and adding someone by username.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use discord::relationships::{Kind, List, Relationship, Status};
use discord::{ChannelKind, Rest, Snowflake};
use gtk::prelude::*;
use gtk::{glib, pango};

use super::App;
use super::state::Page;
use super::widgets::{avatar_widget, badge, palette_index};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    List(List),
    Add,
}

impl Default for Tab {
    fn default() -> Self {
        Tab::List(List::Online)
    }
}

/// What a row's button does with the row's user.
type Action = fn(&Rc<App>, Snowflake);

const TABS: [(Tab, &str); 5] = [
    (Tab::List(List::Online), "Online"),
    (Tab::List(List::All), "All"),
    (Tab::List(List::Pending), "Pending"),
    (Tab::List(List::Blocked), "Blocked"),
    (Tab::Add, "Add Friend"),
];

/// The page's widgets. Its rows are rebuilt from state whenever it is shown
/// and something on it changed.
pub(super) struct FriendsPage {
    pub(super) root: gtk::Box,
    tabs: Vec<(Tab, gtk::ToggleButton)>,
    /// "list" or "add".
    stack: gtk::Stack,
    heading: gtk::Label,
    list: gtk::Box,
    error: gtk::Label,
    entry: gtk::Entry,
    send: gtk::Button,
    add_status: gtk::Label,
    /// A rebuild is already scheduled.
    queued: Cell<bool>,
}

impl FriendsPage {
    pub(super) fn new() -> Self {
        let tabs = TABS
            .iter()
            .map(|&(tab, label)| {
                let button = gtk::ToggleButton::builder().label(label).css_classes(["friends-tab"]).build();
                if tab == Tab::Add {
                    button.add_css_class("add");
                }
                (tab, button)
            })
            .collect::<Vec<_>>();
        for (_, button) in &tabs[1..] {
            button.set_group(Some(&tabs[0].1));
        }
        FriendsPage {
            root: gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(true)
                .css_classes(["chat"])
                .build(),
            tabs,
            stack: gtk::Stack::new(),
            heading: gtk::Label::builder().xalign(0.0).css_classes(["category"]).build(),
            list: gtk::Box::new(gtk::Orientation::Vertical, 0),
            error: label_builder().visible(false).css_classes(["composer-error"]).build(),
            entry: gtk::Entry::builder().placeholder_text("Username").hexpand(true).build(),
            send: gtk::Button::builder()
                .label("Send Friend Request")
                .sensitive(false)
                .css_classes(["login-button"])
                .build(),
            add_status: label_builder().visible(false).build(),
            queued: Cell::new(false),
        }
    }
}

fn label_builder() -> gtk::builders::LabelBuilder {
    gtk::Label::builder().xalign(0.0).wrap(true)
}

impl App {
    pub(super) fn build_friends(self: &Rc<Self>) -> gtk::Widget {
        let page = &self.friends;
        let header = gtk::Box::builder().spacing(8).css_classes(["chat-header", "friends-header"]).build();
        header.append(&gtk::Label::builder().label("Friends").css_classes(["friends-title"]).build());
        for &(tab, ref button) in &page.tabs {
            header.append(button);
            let this = Rc::downgrade(self);
            button.connect_toggled(move |button| {
                if let Some(this) = this.upgrade()
                    && button.is_active()
                {
                    this.select_friends_tab(tab);
                }
            });
        }
        page.root.append(&header);

        let list_page =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["friends-list"]).build();
        list_page.append(&page.heading);
        list_page.append(&page.list);
        page.stack.add_named(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&list_page)
                .build(),
            Some("list"),
        );
        page.stack.add_named(&self.build_add_friend(), Some("add"));
        page.root.append(&page.error);
        page.root.append(&page.stack);
        page.root.clone().upcast()
    }

    fn build_add_friend(self: &Rc<Self>) -> gtk::Widget {
        let page = &self.friends;
        let column =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).css_classes(["add-friend"]).build();
        column.append(&label_builder().label("Add Friend").css_classes(["card-title"]).build());
        column.append(
            &label_builder().label("You can add friends with their Discord username.").css_classes(["hint"]).build(),
        );
        let row = gtk::Box::builder().spacing(8).css_classes(["add-friend-row"]).build();
        row.append(&page.entry);
        row.append(&page.send);
        column.append(&row);
        column.append(&page.add_status);

        page.entry.connect_changed({
            let send = page.send.clone();
            move |entry| send.set_sensitive(!entry.text().trim().is_empty())
        });
        let send = {
            let this = Rc::downgrade(self);
            move || {
                if let Some(this) = this.upgrade() {
                    this.send_friend_request();
                }
            }
        };
        page.entry.connect_activate({
            let send = send.clone();
            move |_| send()
        });
        page.send.connect_clicked(move |_| send());
        column.upcast()
    }

    /// Open Home's friends page.
    pub(super) fn show_friends(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            state.guild = None;
            state.channel = None;
            state.page = Some(Page::Friends);
        }
        self.friends.error.set_visible(false);
        self.content.set_visible_child(&self.friends.root);
        self.rebuild_rail();
        self.rebuild_sidebar();
        self.rebuild_friends();
    }

    fn select_friends_tab(self: &Rc<Self>, tab: Tab) {
        let changed = std::mem::replace(&mut self.state.borrow_mut().friends_tab, tab) != tab;
        if changed {
            self.friends.error.set_visible(false);
            self.rebuild_friends();
            if tab == Tab::Add {
                self.friends.entry.grab_focus();
            }
        }
    }

    /// Rebuild the page once the current burst of events (presence updates
    /// come in floods) is handled, if it is showing.
    pub(super) fn queue_friends_refresh(self: &Rc<Self>) {
        if self.state.borrow().page != Some(Page::Friends) || self.friends.queued.replace(true) {
            return;
        }
        let this = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(this) = this.upgrade() {
                this.friends.queued.set(false);
                if this.state.borrow().page == Some(Page::Friends) {
                    this.rebuild_friends();
                }
            }
        });
    }

    fn rebuild_friends(self: &Rc<Self>) {
        let page = &self.friends;
        let (tab, incoming) = {
            let state = self.state.borrow();
            (state.friends_tab, state.relationships.incoming())
        };
        for (t, button) in &page.tabs {
            // Activating a tab re-enters `select_friends_tab`, which finds no
            // change; so state must not be borrowed here.
            button.set_active(*t == tab);
            if *t == Tab::List(List::Pending) {
                let child = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                child.append(&gtk::Label::new(Some("Pending")));
                if let Some(badge) = badge(incoming) {
                    child.append(&badge);
                }
                button.set_child(Some(&child));
            }
        }
        let state = self.state.borrow();
        let list = match tab {
            Tab::Add => {
                page.stack.set_visible_child_name("add");
                return;
            }
            Tab::List(list) => list,
        };
        page.stack.set_visible_child_name("list");
        while let Some(child) = page.list.first_child() {
            page.list.remove(&child);
        }
        let people = state.relationships.list(list);
        let (heading, empty) = match list {
            List::Online => ("ONLINE", "None of your friends are online."),
            List::All => ("ALL FRIENDS", "No friends yet. Add someone with their username."),
            List::Pending => ("PENDING", "No pending friend requests."),
            List::Blocked => ("BLOCKED", "You haven't blocked anyone."),
        };
        page.heading.set_label(&format!("{heading} — {}", people.len()));
        if people.is_empty() {
            page.list.append(&label_builder().label(empty).css_classes(["hint", "friends-empty"]).build());
        }
        for relationship in people {
            page.list.append(&self.friend_row(relationship, state.relationships.presence(relationship.user.id).status));
        }
    }

    fn friend_row(self: &Rc<Self>, relationship: &Relationship, status: Status) -> gtk::Widget {
        let user = &relationship.user;
        let row = gtk::Box::builder().spacing(12).css_classes(["friend"]).build();

        let avatar = gtk::Overlay::builder()
            .child(&avatar_widget(user, palette_index(user), 32, Some(&self.images)))
            .valign(gtk::Align::Center)
            .build();
        if relationship.kind == Kind::Friend {
            avatar.add_overlay(&presence_dot(status));
        }
        row.append(&avatar);

        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        let name = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        name.append(&gtk::Label::builder().label(relationship.name()).css_classes(["author"]).build());
        name.append(
            &gtk::Label::builder()
                .label(&user.username)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["time"])
                .build(),
        );
        text.append(&name);
        let detail = match (relationship.kind, relationship.spam) {
            (Kind::Friend, _) => {
                let presence = self.state.borrow().relationships.presence(user.id);
                presence.text.unwrap_or_else(|| status.label().to_owned())
            }
            (Kind::Incoming, false) => "Incoming friend request".to_owned(),
            (Kind::Incoming, true) => "Incoming friend request, likely spam".to_owned(),
            (Kind::Outgoing, _) => "Outgoing friend request".to_owned(),
            (Kind::Blocked, _) => "Blocked".to_owned(),
            (Kind::Other(_), _) => String::new(),
        };
        text.append(
            &gtk::Label::builder()
                .label(detail)
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["hint"])
                .build(),
        );
        row.append(&text);

        let id = user.id;
        let actions: &[(&str, Action)] = match relationship.kind {
            Kind::Friend => &[("Message", App::message_user), ("Remove", App::confirm_remove_friend)],
            Kind::Incoming => &[("Accept", App::accept_friend), ("Ignore", App::remove_relationship)],
            Kind::Outgoing => &[("Cancel", App::remove_relationship)],
            Kind::Blocked => &[("Unblock", App::remove_relationship)],
            Kind::Other(_) => &[],
        };
        for &(label, action) in actions {
            let button =
                gtk::Button::builder().label(label).valign(gtk::Align::Center).css_classes(["row-action"]).build();
            if matches!(label, "Accept" | "Message") {
                button.add_css_class("primary");
            }
            let this = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    action(&this, id);
                }
            });
            row.append(&button);
        }
        row.upcast()
    }

    /// Open the DM with `user`, creating it if there is none yet.
    pub(super) fn message_user(self: &Rc<Self>, user: Snowflake) {
        let existing = self
            .state
            .borrow()
            .private_channels
            .iter()
            .find(|c| c.kind == ChannelKind::Dm && c.recipients.first().is_some_and(|u| u.id == user))
            .map(|c| c.id);
        if let Some(channel) = existing {
            return self.open_channel(channel);
        }
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rest.open_dm(user).await {
                Ok(channel) => {
                    let id = channel.id;
                    this.on_private_channel(channel);
                    this.open_channel(id);
                }
                Err(e) => this.show_friends_error(&format!("Could not open the DM: {e}")),
            }
        });
    }

    fn confirm_remove_friend(self: &Rc<Self>, user: Snowflake) {
        let Some(name) = self.state.borrow().relationships.get(user).map(|r| r.name().to_owned()) else { return };
        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message(format!("Remove {name}?"))
            .detail("They will no longer be your friend. You can send them a new request later.")
            .buttons(["Cancel", "Remove Friend"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let this = self.clone();
        glib::spawn_future_local(async move {
            if matches!(dialog.choose_future(Some(&this.window)).await, Ok(1)) {
                this.remove_relationship(user);
            }
        });
    }

    fn accept_friend(self: &Rc<Self>, user: Snowflake) {
        self.friend_action(move |rest| async move { rest.accept_friend_request(user).await });
    }

    /// Unfriend, unblock, or ignore or cancel a request.
    fn remove_relationship(self: &Rc<Self>, user: Snowflake) {
        self.friend_action(move |rest| async move { rest.remove_relationship(user).await });
    }

    /// Run a relationship request. Success needs no handling here: the
    /// gateway reports the change, which updates the page.
    fn friend_action<F, Fut>(self: &Rc<Self>, action: F)
    where
        F: FnOnce(Rest) -> Fut,
        Fut: Future<Output = Result<(), discord::Error>> + 'static,
    {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        self.friends.error.set_visible(false);
        let request = action(rest);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = request.await {
                this.show_friends_error(&e.to_string());
            }
        });
    }

    fn show_friends_error(&self, text: &str) {
        self.friends.error.set_label(text);
        self.friends.error.set_visible(true);
    }

    fn send_friend_request(self: &Rc<Self>) {
        let page = &self.friends;
        let username = page.entry.text().trim().trim_start_matches('@').to_owned();
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        if username.is_empty() {
            return;
        }
        page.send.set_sensitive(false);
        page.add_status.set_visible(false);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.send_friend_request(&username).await;
            let page = &this.friends;
            page.add_status.set_css_classes(&[if result.is_ok() { "success" } else { "error" }]);
            match result {
                Ok(()) => {
                    page.add_status.set_label(&format!("Friend request sent to {username}."));
                    page.entry.set_text("");
                }
                Err(e) => {
                    page.add_status.set_label(&e.to_string());
                    page.send.set_sensitive(true);
                }
            }
            page.add_status.set_visible(true);
        });
    }
}

/// The coloured dot at the corner of an avatar.
fn presence_dot(status: Status) -> gtk::Widget {
    let class = match status {
        Status::Online => "online",
        Status::Idle => "idle",
        Status::DoNotDisturb => "dnd",
        Status::Offline => "offline",
    };
    let dot = gtk::Box::builder()
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .tooltip_text(status.label())
        .css_classes(["presence", class])
        .build();
    dot.upcast()
}
