//! The member list pane beside the chat: a server's, which Discord sends a
//! window of as we scroll, or a group DM's, which its channel holds.
//!
//! Everything that decides what is asked for and what a row holds lives in
//! `discord::subscriptions` and `discord::member_list`. Here the pane shows
//! them: a list of as many rows as the list has, in which the ones we hold
//! are drawn and the others wait.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::member_list::{self, Change, Slot};
use discord::relationships::{Presence, Status};
use discord::subscriptions::view_ranges;
use discord::{Snowflake, User};
use gtk::{gdk, glib};

use super::App;
use super::friends::presence_dot;
use super::menus;
use super::profile::UserTarget;
use super::state::Target;
use super::widgets::avatar_widget_in;
use crate::config;
use crate::render;

/// How long the list rests before the ranges it shows are sent, as the
/// client waits (`updateSubscription`, 289287).
const SETTLE: Duration = Duration::from_millis(50);

/// What a row is as tall as when it shows nobody yet.
const ROW: i32 = 48;

mod rows {
    use std::cell::Cell;

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gio, glib};

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct Rows {
            pub(super) len: Cell<u32>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Rows {
            const NAME: &'static str = "NeucordMemberRows";
            type Type = super::Rows;
            type Interfaces = (gio::ListModel,);
        }

        impl ObjectImpl for Rows {}

        impl ListModelImpl for Rows {
            fn item_type(&self) -> glib::Type {
                glib::Object::static_type()
            }

            fn n_items(&self) -> u32 {
                self.len.get()
            }

            // The list only asks for rows it draws, so a server with
            // hundreds of thousands of members costs no more than one with
            // a few. What a row shows is read from the state as it is bound.
            fn item(&self, position: u32) -> Option<glib::Object> {
                (position < self.len.get()).then(glib::Object::new::<glib::Object>)
            }
        }
    }

    glib::wrapper! {
        /// A list of `len` rows that stand for what the pane draws.
        pub struct Rows(ObjectSubclass<imp::Rows>) @implements gio::ListModel;
    }

    impl Rows {
        pub fn new() -> Self {
            glib::Object::new()
        }

        /// The list is now `len` long, `removed` rows from `position`
        /// having given way to `added`.
        pub fn changed(&self, len: usize, position: usize, removed: usize, added: usize) {
            self.imp().len.set(len as u32);
            self.items_changed(position as u32, removed as u32, added as u32);
        }
    }
}

/// The pane, and what it shows.
pub(super) struct Pane {
    pub(super) root: adw::ToolbarView,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    /// A DM's panel, which the profile is drawn in.
    panel: gtk::Box,
    scroller: gtk::ScrolledWindow,
    rows: rows::Rows,
    /// The toggles in the headers of the pages beside the pane.
    toggles: RefCell<Vec<gtk::ToggleButton>>,
    /// Whose list the rows are of.
    shown: RefCell<Option<Target>>,
    /// The ranges are sent when the list has rested.
    settle: RefCell<Option<glib::SourceId>>,
}

impl Pane {
    pub(super) fn new() -> Self {
        let rows = rows::Rows::new();
        let title = adw::WindowTitle::new("Members", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let stack = gtk::Stack::new();
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&adw::Spinner::new(), Some("loading"));
        let panel = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        stack.add_named(
            &gtk::ScrolledWindow::builder().child(&panel).hscrollbar_policy(gtk::PolicyType::Never).build(),
            Some("profile"),
        );
        let root = adw::ToolbarView::builder().content(&stack).build();
        root.add_top_bar(&header);
        Pane {
            root,
            stack,
            title,
            panel,
            scroller,
            rows,
            toggles: RefCell::default(),
            shown: RefCell::default(),
            settle: RefCell::default(),
        }
    }
}

impl App {
    /// The pane, wired to the chat: its rows, and its scrolling.
    pub(super) fn build_members(self: &Rc<Self>) -> gtk::Widget {
        let pane = &self.members;
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind({
            let this = Rc::downgrade(self);
            move |_, item| {
                let Some(this) = this.upgrade() else { return };
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                item.set_activatable(false);
                item.set_child(Some(&this.member_row(item.position() as usize)));
            }
        });
        let list = gtk::ListView::new(Some(gtk::NoSelection::new(Some(pane.rows.clone()))), Some(factory));
        pane.scroller.set_child(Some(&list));

        // The rows asked for follow what is on screen.
        let adjustment = pane.scroller.vadjustment();
        let rest = {
            let this = Rc::downgrade(self);
            move |_: &gtk::Adjustment| {
                if let Some(this) = this.upgrade() {
                    this.settle_members();
                }
            }
        };
        adjustment.connect_value_changed(rest.clone());
        adjustment.connect_upper_notify(rest.clone());
        adjustment.connect_page_size_notify(rest);

        self.right.connect_show_sidebar_notify({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.members_shown_changed();
                }
            }
        });
        // A pane over the chat starts closed, and the docked one is as we left it.
        self.right.connect_collapsed_notify({
            let this = Rc::downgrade(self);
            move |right| {
                let Some(this) = this.upgrade() else { return };
                if right.is_collapsed() {
                    right.set_show_sidebar(false);
                }
                this.sync_members();
            }
        });
        pane.root.clone().upcast()
    }

    /// The button in a header bar that shows and hides the pane.
    pub(super) fn members_toggle(self: &Rc<Self>) -> gtk::ToggleButton {
        let button = gtk::ToggleButton::builder()
            .icon_name("system-users-symbolic")
            .tooltip_text("Member List")
            .visible(false)
            .build();
        button.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                // The button has flipped; the pane follows, and the buttons
                // follow the pane.
                let show = !this.right.shows_sidebar();
                if !this.right.is_collapsed() {
                    this.state.borrow_mut().members_closed = !show;
                    if let Err(e) = this.remembered.set(&config::MEMBERS_CLOSED, !show) {
                        eprintln!("config: {e}");
                    }
                }
                this.right.set_show_sidebar(show);
            }
        });
        self.members.toggles.borrow_mut().push(button.clone());
        button
    }

    /// Whether the pane shows a list now, beside the chat or over it.
    fn members_visible(&self) -> bool {
        self.right.shows_sidebar() && self.state.borrow().member_target().is_some()
    }

    /// Where we are changed: what the pane shows and what we are
    /// subscribed to follow.
    pub(super) fn sync_members(self: &Rc<Self>) {
        // Wherever we are changed, the header's threads button may change.
        self.sync_threads_button();
        let target = self.state.borrow().member_target();
        let wanted = target.is_some() && {
            // Over the chat, the pane stays as it was; docked, as it was left.
            if self.right.is_collapsed() { self.right.shows_sidebar() } else { !self.state.borrow().members_closed }
        };
        let tooltip = if matches!(target, Some(Target::Dm(_))) { "User Profile" } else { "Member List" };
        for toggle in &*self.members.toggles.borrow() {
            toggle.set_visible(target.is_some());
            toggle.set_tooltip_text(Some(tooltip));
        }
        self.right.set_show_sidebar(wanted);
        self.show_member_list(target);
        self.members_shown_changed();
    }

    /// The pane was shown or hidden.
    fn members_shown_changed(self: &Rc<Self>) {
        let shown = self.right.shows_sidebar();
        for toggle in &*self.members.toggles.borrow() {
            toggle.set_active(shown);
        }
        self.sync_profile_panel();
        self.subscribe_open();
        self.settle_members();
    }

    /// Subscribe to the open server as the client does when a channel opens
    /// (`CHANNEL_SELECT`, 661670): to its events, and, with its member
    /// list showing, to the first rows of that.
    pub(super) fn subscribe_open(&self) {
        let visible = self.members_visible();
        let state = &mut *self.state.borrow_mut();
        let Some(guild) = state.guild else { return };
        let list = match state.member_target() {
            Some(Target::Server { channel, .. }) if visible => Some(channel),
            _ => None,
        };
        state.subscriptions.select(guild, list);
        self.send_subscriptions(state);
    }

    /// A session began: send what we are subscribed to again, as the client
    /// does for READY (`fresh`) and RESUMED.
    pub(super) fn subscribe_session(&self, fresh: bool) {
        let visible = self.members_visible();
        {
            let state = &mut *self.state.borrow_mut();
            if fresh {
                state.member_lists.reset();
            }
            let open = state.guild.map(|guild| {
                let list = match state.member_target() {
                    Some(Target::Server { channel, .. }) if visible => Some(channel),
                    _ => None,
                };
                (guild, list)
            });
            let guilds = &state.guilds;
            let messages = state.subscriptions.session(fresh, open, |id| guilds.iter().any(|g| g.id == id));
            if let Some(gateway) = &state.gateway {
                gateway.send_subscriptions(&messages);
            }
        }
        if fresh {
            self.refresh_member_rows();
        }
    }

    /// Send what was queued for the gateway.
    fn send_subscriptions(&self, state: &mut super::state::State) {
        let guilds = &state.guilds;
        let messages = state.subscriptions.flush(|id| guilds.iter().any(|g| g.id == id));
        if let Some(gateway) = &state.gateway {
            gateway.send_subscriptions(&messages);
        }
    }

    /// Once the list has rested, ask for the rows it shows (`NJ`, 361610).
    fn settle_members(self: &Rc<Self>) {
        if let Some(old) = self.members.settle.take() {
            old.remove();
        }
        let this = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(SETTLE, move || {
            let Some(this) = this.upgrade() else { return };
            this.members.settle.take();
            this.send_member_ranges();
        });
        *self.members.settle.borrow_mut() = Some(id);
    }

    fn send_member_ranges(&self) {
        let Some(Target::Server { guild, channel, .. }) = self.members.shown.borrow().clone() else { return };
        let rows = self.members.rows.n_items();
        if !self.members_visible() || rows == 0 {
            return;
        }
        let adjustment = self.members.scroller.vadjustment();
        let row = adjustment.upper() / f64::from(rows);
        if row <= 0.0 {
            return;
        }
        let ranges = view_ranges(adjustment.value(), adjustment.page_size(), row);
        let state = &mut *self.state.borrow_mut();
        state.subscriptions.ranges(guild, channel, ranges);
        self.send_subscriptions(state);
    }

    /// Show `target`'s list, starting at its top where it isn't the one shown.
    fn show_member_list(self: &Rc<Self>, target: Option<Target>) {
        let changed = *self.members.shown.borrow() != target;
        if changed {
            *self.members.shown.borrow_mut() = target;
            let len = self.member_rows();
            let old = self.members.rows.n_items() as usize;
            self.members.rows.changed(len, 0, old, len);
            self.members.scroller.vadjustment().set_value(0.0);
        }
        self.sync_profile_panel();
        self.show_loading_or_list();
    }

    /// A DM's panel shows the profile once it is shown, as the client's
    /// renders it; anywhere else there is none.
    fn sync_profile_panel(self: &Rc<Self>) {
        let target = self.members.shown.borrow().clone();
        match target {
            Some(Target::Dm(channel)) => {
                self.members.title.set_title("User Profile");
                if self.right.shows_sidebar() {
                    self.show_profile_panel(channel, &self.members.panel);
                }
            }
            _ => {
                self.members.title.set_title("Members");
                self.forget_profile_panel();
            }
        }
    }

    fn show_loading_or_list(&self) {
        if matches!(&*self.members.shown.borrow(), Some(Target::Dm(_))) {
            return self.members.stack.set_visible_child_name("profile");
        }
        let loading = match &*self.members.shown.borrow() {
            Some(Target::Server { guild, list, .. }) => {
                self.state.borrow().member_lists.get(*guild, list).is_none_or(|l| l.is_unknown())
            }
            _ => false,
        };
        self.members.stack.set_visible_child_name(if loading { "loading" } else { "list" });
    }

    /// How many rows the shown list has: Discord's list, or a group DM's
    /// heading and its people.
    fn member_rows(&self) -> usize {
        let state = self.state.borrow();
        match &*self.members.shown.borrow() {
            Some(Target::Server { guild, list, .. }) => {
                state.member_lists.get(*guild, list).filter(|l| !l.is_unknown()).map_or(0, |l| l.len())
            }
            Some(Target::Group(id)) => state.find_channel(*id).map_or(0, |(_, c)| 2 + c.recipients.len()),
            Some(Target::Dm(_)) | None => 0,
        }
    }

    /// Rows of a server's member list arrived.
    pub(super) fn on_member_list(self: &Rc<Self>, update: member_list::Update) {
        self.learn_users(update.members().into_iter().map(|m| &m.user));
        let shown = matches!(
            &*self.members.shown.borrow(),
            Some(Target::Server { guild, list, .. }) if *guild == update.guild && *list == update.list
        );
        let (known, change) = {
            let mut state = self.state.borrow_mut();
            (state.member_lists.get(update.guild, &update.list).is_some(), state.member_lists.apply(&update))
        };
        if !shown {
            return;
        }
        // A list we had none of was no rows to the pane, whatever its change.
        match (known, change) {
            (true, change) => self.announce_member_rows(change),
            (false, _) => self.refresh_member_rows(),
        }
    }

    /// `user`'s presence changed in a server, or everywhere.
    pub(super) fn on_list_presence(
        self: &Rc<Self>,
        guild: Option<Snowflake>,
        user: Snowflake,
        presence: &discord::relationships::Presence,
    ) {
        let changes = self.state.borrow_mut().member_lists.set_presence(guild, user, presence);
        let shown = self.members.shown.borrow().clone();
        for (g, list, change) in changes {
            if matches!(&shown, Some(Target::Server { guild, list: l, .. }) if *guild == g && *l == list) {
                self.announce_member_rows(Some(change));
            }
        }
    }

    /// The shown list's rows changed as `change` says.
    fn announce_member_rows(&self, change: Option<Change>) {
        if let Some(c) = change {
            self.members.rows.changed(self.member_rows(), c.position, c.removed, c.added);
            self.show_loading_or_list();
        }
    }

    /// Every row may be another, or drawn another way: names, colours, who
    /// owns the server, who is a friend, a new session.
    pub(super) fn refresh_member_rows(&self) {
        let (old, len) = (self.members.rows.n_items() as usize, self.member_rows());
        self.members.rows.changed(len, 0, old, len);
        self.show_loading_or_list();
    }

    /// What the row at `position` shows.
    fn member_row(self: &Rc<Self>, position: usize) -> gtk::Widget {
        let state = self.state.borrow();
        match &*self.members.shown.borrow() {
            Some(Target::Server { guild, list, .. }) => {
                let Some(guild) = state.guilds.iter().find(|g| g.id == *guild) else { return placeholder() };
                match state.member_lists.get(guild.id, list).map(|l| l.slot(position)) {
                    Some(Slot::Header(group)) => header(&group_title(guild, &group.id), group.count),
                    Some(Slot::Member(member)) => {
                        let roles = state.members.roles(guild.id, member.user.id).unwrap_or(&member.roles);
                        let colour = guild.member_colour(roles);
                        let owner = guild.owner_id == Some(member.user.id);
                        self.person(&member.user, Some(guild.id), member.name(), colour, &member.presence, owner)
                    }
                    _ => placeholder(),
                }
            }
            Some(Target::Group(id)) => {
                let (Some(me), Some((_, channel))) = (&state.me, state.find_channel(*id)) else { return placeholder() };
                let people = member_list::group_members(me, &channel.recipients);
                if position == 0 {
                    return header("Members", people.len() as u32);
                }
                let Some(user) = people.get(position - 1) else { return placeholder() };
                // We know how our friends are, and ours is that we are here.
                let mut presence = state.relationships.presence(user.id);
                if user.id == me.id {
                    presence.status = Status::Online;
                }
                let owner = channel.owner_id == Some(user.id);
                self.person(user, None, &state.name_in(*id, user), None, &presence, owner)
            }
            Some(Target::Dm(_)) | None => placeholder(),
        }
    }

    /// A person's row: their picture and status, name, what they are up to,
    /// and what marks them.
    fn person(
        self: &Rc<Self>,
        user: &User,
        guild: Option<Snowflake>,
        name: &str,
        colour: Option<discord::members::RoleColour>,
        presence: &Presence,
        owner: bool,
    ) -> gtk::Widget {
        let (status, detail) = (presence.status, presence.text.as_deref());
        let row =
            gtk::Box::builder().spacing(12).margin_start(12).margin_end(12).margin_top(4).margin_bottom(4).build();
        if status == Status::Offline {
            row.add_css_class("dimmed");
        }
        let state = self.state.borrow();
        let server = guild.and_then(|g| Some((g, state.members.avatar(g, user.id)?)));
        let avatar = gtk::Overlay::builder()
            .child(&avatar_widget_in(user, 32, Some(&self.images), server))
            .valign(gtk::Align::Center)
            .build();
        avatar.add_overlay(&presence_dot(status));
        row.append(&avatar);

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        match colour {
            Some(colour) => label.set_markup(&render::coloured(name, &colour)),
            None => label.set_text(name),
        }
        text.append(&label);
        if let Some(detail) = detail {
            text.append(
                &gtk::Label::builder()
                    .label(detail)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["caption", "dimmed"])
                    .build(),
            );
        }
        row.append(&text);
        if owner {
            row.append(
                &gtk::Image::builder()
                    .icon_name("starred-symbolic")
                    .tooltip_text("Owner")
                    .valign(gtk::Align::Center)
                    .build(),
            );
        }
        if user.bot {
            row.append(&gtk::Label::builder().label("APP").valign(gtk::Align::Center).css_classes(["caption"]).build());
        }
        let target = {
            let (this, user, row, presence) = (Rc::downgrade(self), user.clone(), row.downgrade(), presence.clone());
            move || {
                let (this, row) = (this.upgrade()?, row.upgrade()?);
                let channel = this.state.borrow().channel;
                let (user, presence) = (user.clone(), Some(presence.clone()));
                Some((this, UserTarget { user, guild, channel, presence, anchor: row.upcast() }))
            }
        };
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        click.connect_released({
            let target = target.clone();
            move |_, _, _, _| {
                if let Some((this, target)) = target() {
                    this.show_profile(&target, false);
                }
            }
        });
        row.add_controller(click);
        menus::on_secondary(&row, {
            let row = row.downgrade();
            move |x, y| {
                if let (Some(row), Some((this, target))) = (row.upgrade(), target()) {
                    this.user_menu(target, row.upcast_ref(), menus::At::Point(x, y));
                }
            }
        });
        row.upcast()
    }
}

/// What a group is called: a server's roles by name.
fn group_title(guild: &discord::Guild, id: &str) -> String {
    match id {
        member_list::ONLINE => "Online".to_owned(),
        member_list::OFFLINE => "Offline".to_owned(),
        member_list::UNKNOWN => String::new(),
        role => role.parse().ok().and_then(|id| guild.role(Snowflake(id))).map(|r| r.name.clone()).unwrap_or_default(),
    }
}

/// A group's title, with how many it holds.
fn header(title: &str, count: u32) -> gtk::Widget {
    gtk::Label::builder()
        .label(format!("{title} — {count}"))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(4)
        .css_classes(["caption-heading"])
        .build()
        .upcast()
}

/// A row we haven't been sent, which keeps its place.
fn placeholder() -> gtk::Widget {
    gtk::Box::builder().height_request(ROW).build().upcast()
}
