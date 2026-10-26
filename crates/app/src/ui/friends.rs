//! Home's friends page: friends and their presence, pending requests,
//! blocked users, and adding someone by username.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use adw::prelude::*;
use discord::relationships::{Kind, List, Relationship, Status};
use discord::{ChannelKind, Rest, Snowflake};
use gtk::glib;

use super::App;
use super::state::Page;
use super::widgets::avatar_widget;

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

const TABS: [(Tab, &str, &str, &str); 5] = [
    (Tab::List(List::Online), "online", "Online", "user-available-symbolic"),
    (Tab::List(List::All), "all", "All", "system-users-symbolic"),
    (Tab::List(List::Pending), "pending", "Pending", "user-status-pending-symbolic"),
    (Tab::List(List::Blocked), "blocked", "Blocked", "action-unavailable-symbolic"),
    (Tab::Add, "add", "Add Friend", "contact-new-symbolic"),
];

fn tab_name(tab: Tab) -> &'static str {
    TABS.iter().find(|t| t.0 == tab).map_or("online", |t| t.1)
}

/// The page's widgets. Its rows are rebuilt from state whenever it is shown
/// and something on it changed.
pub(super) struct FriendsPage {
    pub(super) root: adw::BreakpointBin,
    stack: adw::ViewStack,
    /// Each list tab's heading and rows.
    lists: Vec<(List, gtk::Label, gtk::ListBox)>,
    error: gtk::Label,
    entry: gtk::Entry,
    send: gtk::Button,
    add_status: gtk::Label,
    /// A rebuild is already scheduled.
    queued: Cell<bool>,
}

impl FriendsPage {
    pub(super) fn new() -> Self {
        let lists = TABS
            .iter()
            .filter_map(|&(tab, ..)| match tab {
                Tab::List(list) => Some(list),
                Tab::Add => None,
            })
            .map(|list| {
                let heading = label_builder().css_classes(["heading"]).build();
                (list, heading, gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build())
            })
            .collect();
        FriendsPage {
            root: adw::BreakpointBin::builder().width_request(300).height_request(200).build(),
            stack: adw::ViewStack::new(),
            lists,
            error: label_builder().visible(false).css_classes(["error"]).build(),
            entry: gtk::Entry::builder().placeholder_text("Username").hexpand(true).build(),
            send: gtk::Button::builder()
                .label("Send Request")
                .sensitive(false)
                .css_classes(["suggested-action"])
                .build(),
            add_status: label_builder().visible(false).build(),
            queued: Cell::new(false),
        }
    }
}

fn label_builder() -> gtk::builders::LabelBuilder {
    gtk::Label::builder().xalign(0.0).wrap(true)
}

/// A page's content at a readable width, scrolling if it runs long.
pub(super) fn clamped(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder()
        .maximum_size(720)
        .child(child)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(12)
        .margin_end(12)
        .build();
    gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build()
}

impl App {
    pub(super) fn build_friends(self: &Rc<Self>) -> gtk::Widget {
        let page = &self.friends;
        for &(tab, name, title, icon) in &TABS {
            let child = match tab {
                Tab::Add => self.build_add_friend(),
                Tab::List(list) => {
                    let (_, heading, rows) = page.lists.iter().find(|l| l.0 == list).expect("a list per tab");
                    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
                    column.append(heading);
                    column.append(rows);
                    clamped(&column).upcast()
                }
            };
            page.stack.add_titled_with_icon(&child, Some(name), title, icon);
        }
        page.stack.connect_visible_child_name_notify({
            let this = Rc::downgrade(self);
            move |stack| {
                let tab = stack.visible_child_name().and_then(|name| TABS.iter().find(|t| t.1 == name)).map(|t| t.0);
                if let (Some(this), Some(tab)) = (this.upgrade(), tab) {
                    this.select_friends_tab(tab);
                }
            }
        });

        let switcher = adw::ViewSwitcher::builder().stack(&page.stack).policy(adw::ViewSwitcherPolicy::Wide).build();
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        let bar = adw::ViewSwitcherBar::builder().stack(&page.stack).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&page.error);
        content.append(&page.stack);
        let view = adw::ToolbarView::builder().content(&content).build();
        view.add_top_bar(&header);
        view.add_bottom_bar(&bar);

        // Too narrow for the tabs in the header: move them to the bottom.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            800.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&header, "title-widget", Some(&adw::WindowTitle::new("Friends", "").to_value()));
        narrow.add_setter(&bar, "reveal", Some(&true.to_value()));
        page.root.add_breakpoint(narrow);
        page.root.set_child(Some(&view));
        page.root.clone().upcast()
    }

    fn build_add_friend(self: &Rc<Self>) -> gtk::Widget {
        let page = &self.friends;
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.append(&label_builder().label("Add Friend").css_classes(["title-2"]).build());
        column.append(&label_builder().label("You can add friends with their Discord username.").build());
        let row = gtk::Box::builder().spacing(6).build();
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
        clamped(&column).upcast()
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
        self.set_content_title("Friends");
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
        let pending = page.stack.page(&page.stack.child_by_name("pending").expect("pending tab"));
        pending.set_badge_number(incoming);
        pending.set_needs_attention(incoming > 0);
        // Showing a tab re-enters `select_friends_tab`, which finds no
        // change; so state must not be borrowed here.
        page.stack.set_visible_child_name(tab_name(tab));
        let Tab::List(list) = tab else { return };
        let state = self.state.borrow();
        let Some((_, heading, rows)) = page.lists.iter().find(|l| l.0 == list) else { return };
        rows.remove_all();
        let people = state.relationships.list(list);
        let (title, empty) = match list {
            List::Online => ("Online", "None of your friends are online."),
            List::All => ("All Friends", "No friends yet. Add someone with their username."),
            List::Pending => ("Pending", "No pending friend requests."),
            List::Blocked => ("Blocked", "You haven't blocked anyone."),
        };
        heading.set_label(&format!("{title} — {}", people.len()));
        heading.set_visible(!people.is_empty());
        rows.set_css_classes(if people.is_empty() { &[] } else { &["boxed-list"] });
        if people.is_empty() {
            rows.append(
                &gtk::ListBoxRow::builder()
                    .activatable(false)
                    .child(&label_builder().label(empty).css_classes(["dimmed"]).build())
                    .build(),
            );
        }
        for relationship in people {
            rows.append(&self.friend_row(relationship, state.relationships.presence(relationship.user.id).status));
        }
    }

    fn friend_row(self: &Rc<Self>, relationship: &Relationship, status: Status) -> gtk::Widget {
        let user = &relationship.user;
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
        let subtitle = if detail.is_empty() { user.username.clone() } else { format!("{} · {detail}", user.username) };
        let row = adw::ActionRow::builder()
            .title(relationship.name())
            .subtitle(subtitle)
            .use_markup(false)
            .subtitle_lines(1)
            .build();

        let avatar = gtk::Overlay::builder()
            .child(&avatar_widget(user, 32, Some(&self.images)))
            .valign(gtk::Align::Center)
            .build();
        if relationship.kind == Kind::Friend {
            avatar.add_overlay(&presence_dot(status));
        }
        row.add_prefix(&avatar);

        let id = user.id;
        let actions: &[(&str, Action)] = match relationship.kind {
            Kind::Friend => &[("Message", App::message_user), ("Remove", App::confirm_remove_friend)],
            Kind::Incoming => &[("Accept", App::accept_friend), ("Ignore", App::remove_relationship)],
            Kind::Outgoing => &[("Cancel", App::remove_relationship)],
            Kind::Blocked => &[("Unblock", App::remove_relationship)],
            Kind::Other(_) => &[],
        };
        for &(label, action) in actions {
            let button = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
            if label == "Accept" {
                button.add_css_class("suggested-action");
            }
            let this = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    action(&this, id);
                }
            });
            row.add_suffix(&button);
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
        let dialog = confirm(
            &format!("Remove {name}?"),
            "They will no longer be your friend. You can send them a new request later.",
            "_Remove Friend",
            adw::ResponseAppearance::Destructive,
        );
        let this = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "confirm" {
                this.remove_relationship(user);
            }
        });
    }

    /// Accept a friend request. Discord wants requests from strangers
    /// confirmed, as the official client does with a dialog: up front when the
    /// request is known to be one, else once Discord answers that it is.
    fn accept_friend(self: &Rc<Self>, user: Snowflake) {
        let Some(relationship) = self.state.borrow().relationships.get(user).cloned() else { return };
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        self.friends.error.set_visible(false);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let mut confirmed = relationship.stranger && this.confirm_stranger(&relationship).await;
            if relationship.stranger && !confirmed {
                return;
            }
            loop {
                match rest.accept_friend_request(user, confirmed).await {
                    Err(e) if !confirmed && e.code() == Some(discord::rest::STRANGER_NOT_CONFIRMED) => {
                        if !this.confirm_stranger(&relationship).await {
                            return;
                        }
                        confirmed = true;
                    }
                    Err(e) => return this.show_friends_error(&e.to_string()),
                    Ok(()) => return,
                }
            }
        });
    }

    async fn confirm_stranger(&self, relationship: &Relationship) -> bool {
        let dialog = confirm(
            &format!("Accept a request from {}?", relationship.name()),
            "You don't share any servers or friends with them. Only accept if you know who they are.",
            "_Accept",
            adw::ResponseAppearance::Suggested,
        );
        dialog.choose_future(Some(&self.window)).await == "confirm"
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

/// A dialog asking to go ahead with `verb`, answering "confirm" or "cancel".
pub(super) fn confirm(heading: &str, body: &str, verb: &str, appearance: adw::ResponseAppearance) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", "_Cancel"), ("confirm", verb)]);
    dialog.set_response_appearance("confirm", appearance);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog
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
