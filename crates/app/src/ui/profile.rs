//! A user's profile, in the popout the client opens from a name, an avatar,
//! a mention or a member list row (`342296`, drawn by `348434`), in the panel
//! beside a DM (`ok` in 289287), and the menu on those places
//! (`user-context`, 841549). Asking for the profile and keeping it is
//! [`discord::profile`]'s; this draws what it holds.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use discord::profile::{self, Badge, Banner, Fetch, Profile, Request};
use discord::relationships::{Kind as Relation, Presence, Status};
use discord::{Snowflake, User};
use gtk::glib;

use super::App;
use super::friends::presence_dot;
use super::menus::{Choice, Choices, Entry, Item, Kind, Style};
use super::state::{State, now_ms};
use super::widgets::{avatar_from, avatar_widget};
use crate::images::Row;
use crate::render::{self, Context, Names};

/// Where a profile or its menu was asked for, and whose it is.
pub(super) struct UserTarget {
    pub(super) user: User,
    /// The server and channel it was asked in.
    pub(super) guild: Option<Snowflake>,
    pub(super) channel: Option<Snowflake>,
    /// How they are, where the place showed it (a member list row).
    pub(super) presence: Option<Presence>,
    /// What the popout points at.
    pub(super) anchor: gtk::Widget,
}

/// A banner's width and height as the client draws it: the popout's
/// (`bannerWidth` and `bannerHeight` of `POPOUT` in 207634), which the panel
/// takes in the redesign (`specOverrides` in 289287), and the panel's own
/// (`SIDEBAR`). The popout is as wide as its banner.
const POPOUT_BANNER: (i32, i32) = (300, 105);
const PANEL_BANNER: (i32, i32) = (340, 120);
const WIDTH: i32 = POPOUT_BANNER.0;
const AVATAR: i32 = 80;
const BADGE: i32 = 20;

/// Not for Discord's own account or a webhook's name, which have no profile
/// (`isNonUserBot`, 619019).
fn is_person(target: &UserTarget) -> bool {
    !target.user.non_user_bot()
}

fn is_me(app: &App, user: &User) -> bool {
    app.state.borrow().me() == Some(user.id)
}

/// Opens the popout (`user-profile`, 243949): any account that has one.
pub(super) const PROFILE: Entry<UserTarget> = Entry::Item(Item {
    name: "profile",
    label: "_Profile",
    shows: |_, target| is_person(target),
    kind: Kind::Action(|app, target| app.show_profile(target, false)),
});

/// Put them in the message box (`mention`, 448290), where we may write.
pub(super) const MENTION: Entry<UserTarget> = Entry::Item(Item {
    name: "mention",
    label: "_Mention",
    shows: |app, target| {
        let state = app.state.borrow();
        let channel = target.channel.or(state.channel);
        is_person(target) && channel.is_some_and(|c| state.can_send(c))
    },
    kind: Kind::Action(|app, target| app.mention_user(target.user.id)),
});

/// Open the DM (`message-user`, 301541), unless it is ourselves.
pub(super) const MESSAGE: Entry<UserTarget> = Entry::Item(Item {
    name: "message-user",
    label: "Messa_ge",
    shows: |app, target| is_person(target) && !is_me(app, &target.user),
    kind: Kind::Action(|app, target| app.message_user(target.user.id)),
});

/// `note` (497600): not on ourselves. It says "Loading…" until the note
/// arrives, and Add or Edit after. The client opens its profile modal on the
/// note; Neucord opens the popout there.
pub(super) const NOTE: Entry<UserTarget> = Entry::Choices(Choices {
    name: "note",
    choices: |app, target| {
        let state = app.state.borrow();
        if !is_person(target) || state.me() == Some(target.user.id) {
            return Vec::new();
        }
        let label = match (state.notes.is_loading(target.user.id), state.notes.get(target.user.id)) {
            (true, _) => "Loading…",
            (false, Some(_)) => "Edit Note",
            (false, None) => "Add Note",
        };
        vec![Choice { value: String::new(), label: label.to_owned(), ticked: false }]
    },
    style: Style::Plain,
    choose: |app, target, _, _| app.show_profile(target, true),
});

/// Unfriend (`remove-friend`, 803664), which asks first.
pub(super) const REMOVE_FRIEND: Entry<UserTarget> = Entry::Item(Item {
    name: "remove-friend",
    label: "_Remove Friend",
    shows: |app, target| {
        let is_friend =
            app.state.borrow().relationships.get(target.user.id).is_some_and(|r| r.kind == Relation::Friend);
        is_person(target) && !target.user.bot && is_friend
    },
    kind: Kind::Action(|app, target| app.confirm_remove_friend(target.user.id)),
});

/// Only in Developer Mode, whoever it is (50268).
pub(super) const COPY_USER_ID: Entry<UserTarget> = Entry::Item(Item {
    name: "copy-user-id",
    label: "Copy User _ID",
    shows: |app, _| app.state.borrow().settings.developer_mode,
    kind: Kind::Action(|app, target| app.copy(&target.user.id.to_string())),
});

/// Where a profile is drawn.
enum Host {
    Popout(gtk::Popover),
    /// The panel beside a DM.
    Panel(gtk::Box),
}

/// A profile on screen: what it shows is built again as answers arrive.
pub(super) struct View {
    app: std::rc::Weak<App>,
    host: Host,
    target: UserTarget,
    request: Request,
    /// The width and height of its banner.
    banner: (i32, i32),
    /// The note box takes the focus once it shows.
    note: bool,
    /// The sections the reader opened, which a rebuild leaves open.
    open: RefCell<HashSet<&'static str>>,
    /// The note's box holds text not sent yet: nothing is built over it.
    editing: std::cell::Cell<bool>,
}

/// The profiles on screen: the popout, if one is open, and the panel.
#[derive(Default)]
pub(super) struct Views {
    popout: Option<Rc<View>>,
    panel: Option<Rc<View>>,
}

impl App {
    /// The menu of `target`, opened from what it is on. A note the menu
    /// offers is loaded by then, as the client's item loads it as it renders.
    pub(super) fn user_menu(self: &Rc<Self>, target: UserTarget, parent: &gtk::Widget, at: super::menus::At) {
        if is_person(&target) {
            self.load_note(target.user.id);
        }
        super::menus::popup(self, super::menus::USER, target, parent, at);
    }

    /// Open `target`'s profile in a popout, asking for it as the client does:
    /// not at all while it holds one that is current, in the background while
    /// it holds an old one, and before showing anything if it holds none.
    pub(super) fn show_profile(self: &Rc<Self>, target: &UserTarget, note: bool) {
        if !is_person(target) {
            return;
        }
        let Some(me) = self.state.borrow().me() else { return };
        let request = {
            let state = self.state.borrow();
            let role = target.guild.and_then(|g| state.connections_role(g, target.user.id, target.channel));
            Request::popout(&target.user, me, target.guild, role)
        };
        let popover = gtk::Popover::builder().has_arrow(true).build();
        popover.set_parent(&self.window);
        if let Some(bounds) = target.anchor.compute_bounds(&self.window) {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            )));
        }
        let view = Rc::new(View {
            app: Rc::downgrade(self),
            host: Host::Popout(popover.clone()),
            target: target.copy(),
            request,
            banner: POPOUT_BANNER,
            note,
            open: RefCell::default(),
            editing: std::cell::Cell::default(),
        });
        // The app holds it while it is open: nothing else does once its
        // requests are answered, and it still has to follow what changes.
        popover.connect_closed({
            let (app, view) = (Rc::downgrade(self), Rc::downgrade(&view));
            move |popover| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                let mut views = app.profile_views.borrow_mut();
                let gone = views.popout.take_if(|open| Rc::ptr_eq(open, &view));
                drop(views);
                drop(gone);
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            }
        });
        // One at a time: an open one would take this click to dismiss itself.
        let old = self.profile_views.borrow_mut().popout.replace(view.clone());
        if let Some(Host::Popout(old)) = old.as_deref().map(|v| &v.host) {
            old.popdown();
        }
        self.begin_profile(&view);
        popover.popup();
    }

    /// Show the profile of whoever the DM `channel` is with in `host`, the
    /// panel beside it, asking for it as the client's panel does. It is
    /// left as it is if it is already theirs.
    pub(super) fn show_profile_panel(self: &Rc<Self>, channel: Snowflake, host: &gtk::Box) {
        let (user, redesign) = {
            let state = self.state.borrow();
            let user = state.private_channels.iter().find(|c| c.id == channel).and_then(|c| c.recipients.first());
            let (Some(user), Some(me)) = (user.cloned(), state.me()) else { return };
            let redesign = profile::sidebar_redesign(&state.experiments, me);
            (user, redesign)
        };
        let same = self.profile_views.borrow().panel.as_ref().is_some_and(|v| v.target.user.id == user.id);
        if same {
            return self.refresh_profile_views();
        }
        let view = Rc::new(View {
            app: Rc::downgrade(self),
            host: Host::Panel(host.clone()),
            request: Request::sidebar(&user, redesign),
            banner: if redesign { POPOUT_BANNER } else { PANEL_BANNER },
            target: UserTarget {
                user,
                guild: None,
                channel: Some(channel),
                presence: None,
                anchor: host.clone().upcast(),
            },
            note: false,
            open: RefCell::default(),
            editing: std::cell::Cell::default(),
        });
        self.profile_views.borrow_mut().panel = Some(view.clone());
        self.begin_profile(&view);
    }

    /// The panel isn't shown any more.
    pub(super) fn forget_profile_panel(&self) {
        self.profile_views.borrow_mut().panel = None;
    }

    /// Draw `view`, and ask for what it needs.
    fn begin_profile(self: &Rc<Self>, view: &Rc<View>) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let plan = self.state.borrow().profiles.plan(&view.request, now_ms() as u64);
        view.fill(self, plan == Fetch::Before);
        self.load_note_then(view.target.user.id, {
            let (this, view) = (Rc::downgrade(self), view.clone());
            move || {
                if let Some(this) = this.upgrade() {
                    view.fill(&this, false);
                }
            }
        });
        if plan == Fetch::No {
            return;
        }
        self.state.borrow_mut().profiles.asked(&view.request);
        let (this, view) = (self.clone(), view.clone());
        glib::spawn_future_local(async move {
            let answer = rest.profile(&view.request).await;
            let now = now_ms() as u64;
            {
                let mut state = this.state.borrow_mut();
                match answer {
                    Ok(profile) => state.profiles.answered(&view.request, profile, now),
                    Err(e) => {
                        let status = match &e {
                            discord::Error::Http { status, .. } => Some(*status),
                            _ => None,
                        };
                        state.profiles.refused(&view.request, status, now);
                    }
                }
            }
            view.fill(&this, false);
        });
    }

    /// What a profile on screen shows may have changed: someone's note,
    /// status, friendship, or what an answer brought.
    pub(super) fn refresh_profile_views(self: &Rc<Self>) {
        let (popout, panel) = {
            let views = self.profile_views.borrow();
            (views.popout.clone(), views.panel.clone())
        };
        for view in popout.into_iter().chain(panel) {
            view.fill(self, false);
        }
    }

    /// Ask for `user`'s mutual friends when the panel's section opens
    /// (`ob` in 229187), unless they are here or on their way.
    fn ask_friend_list(self: &Rc<Self>, user: Snowflake) {
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest.filter(|_| self.state.borrow().profiles.wants_friend_list(user)) else { return };
        self.state.borrow_mut().profiles.asked_friend_list(user);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let answer = rest.mutual_friends(user).await;
            {
                let mut state = this.state.borrow_mut();
                match answer {
                    Ok(friends) => state.profiles.friend_list_arrived(user, friends),
                    Err(_) => state.profiles.friend_list_failed(user),
                }
            }
            this.refresh_profile_views();
        });
    }

    /// Ask for `user`'s note unless we hold it or have asked: the client does
    /// whenever something that shows it renders (612630). Whatever comes, or
    /// fails to, it counts as the note (none).
    pub(super) fn load_note(self: &Rc<Self>, user: Snowflake) {
        self.load_note_then(user, || {});
    }

    fn load_note_then(self: &Rc<Self>, user: Snowflake, then: impl FnOnce() + 'static) {
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest.filter(|_| self.state.borrow_mut().notes.start(user)) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let note = rest.note(user).await.ok().flatten();
            this.state.borrow_mut().notes.loaded(user, note);
            then();
        });
    }

    /// Our note on `user` changed, from another of our sessions: what is
    /// open shows it, unless it is being written.
    pub(super) fn on_note(self: &Rc<Self>, user: Snowflake, note: Option<String>) {
        self.state.borrow_mut().notes.loaded(user, note);
        self.refresh_profile_views();
    }

    /// Write `user`'s mention into the message box.
    pub(super) fn mention_user(self: &Rc<Self>, user: Snowflake) {
        let buffer = self.composer.buffer();
        buffer.insert_at_cursor(&format!("<@{user}> "));
        self.composer.grab_focus();
    }
}

impl UserTarget {
    /// Another of the same.
    fn copy(&self) -> UserTarget {
        UserTarget {
            user: self.user.clone(),
            guild: self.guild,
            channel: self.channel,
            presence: self.presence.clone(),
            anchor: self.anchor.clone(),
        }
    }
}

/// `unix` seconds as the client writes a date (`An` in 370480).
fn date(unix: i64) -> String {
    glib::DateTime::from_unix_local(unix)
        .ok()
        .and_then(|d| d.format("%b %-d, %Y").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn caption(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(classes.to_vec()).build()
}

impl View {
    fn is_panel(&self) -> bool {
        matches!(self.host, Host::Panel(_))
    }

    /// Show what we hold now, or a spinner while nothing is held to show.
    fn fill(self: &Rc<Self>, app: &Rc<App>, waiting: bool) {
        // Building over the box would lose, or send, what it holds half written.
        if !waiting && self.editing.get() {
            return;
        }
        let body = match waiting {
            true => gtk::Spinner::builder()
                .spinning(true)
                .margin_top(24)
                .margin_bottom(24)
                .width_request(WIDTH)
                .build()
                .upcast(),
            false => self.body(app),
        };
        match &self.host {
            // However much a profile holds, the popout is no taller than
            // the window can show.
            Host::Popout(popover) => {
                let height = (app.window.height() - 120).clamp(200, 700);
                // Its minimum height stays small however much it holds: GTK
                // hides a popover the compositor sized below its minimum,
                // which a profile that replaces the spinner would be.
                let scroller = gtk::ScrolledWindow::builder()
                    .child(&body)
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .propagate_natural_height(true)
                    .max_content_height(height)
                    .build();
                popover.set_child(Some(&scroller));
                // Or the focus goes to the first thing that takes it, the
                // bio, which shows selected.
                scroller.grab_focus();
                self.resized();
            }
            Host::Panel(panel) => {
                while let Some(old) = panel.first_child() {
                    panel.remove(&old);
                }
                panel.append(&body);
            }
        }
    }

    /// What the popout holds changed its size. GTK sizes a popover as it
    /// opens and keeps that size, so a profile that replaced the spinner
    /// would show through a window the spinner's size. It lays out again at
    /// the size it has unless a resize is pending.
    fn resized(&self) {
        if let Host::Popout(popover) = &self.host
            && popover.is_mapped()
        {
            popover.queue_resize();
            popover.present();
        }
    }

    fn body(self: &Rc<Self>, app: &Rc<App>) -> gtk::Widget {
        let state = app.state.borrow();
        let held = state.profiles.get_in(self.target.user.id, self.target.guild);
        // What Discord says of them replaces what we knew when it was clicked.
        let user = &held.as_ref().map_or_else(|| self.target.user.clone(), |p| p.user.clone());
        let guild = self.target.guild.and_then(|g| state.guilds.iter().find(|x| x.id == g));
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        if !self.is_panel() {
            column.set_width_request(WIDTH);
        }

        let shown = held.as_ref().map(Profile::shown);
        if let Some(banner) = shown.as_ref().and_then(|s| s.banner) {
            let (width, height) = self.banner;
            // The size the client asks the CDN for at this width on this screen.
            let size = discord::cdn::asset_size(width as u32 * app.images.scale() as u32);
            let url = match banner {
                Banner::User(hash) => discord::cdn::banner_url(user.id.0, hash, size),
                Banner::Server { guild, hash } => discord::cdn::member_banner_url(guild.0, user.id.0, hash, size),
            };
            // Shown once it has loaded: until then, or if it never does, it
            // would be an empty box.
            let (frame, picture) = fixed_picture(-1, height, gtk::ContentFit::Cover);
            frame.set_visible(false);
            app.images.load(&url, {
                let (frame, view) = (frame.downgrade(), Rc::downgrade(self));
                move |paintable| {
                    picture.set_paintable(Some(&paintable));
                    if let (Some(frame), Some(view)) = (frame.upgrade(), view.upgrade()) {
                        frame.set_visible(true);
                        view.resized();
                    }
                }
            });
            column.append(&frame);
        }

        let member = held.as_ref().and_then(|p| p.member.clone());
        let name = member
            .as_ref()
            .and_then(|m| m.nick.clone())
            .or_else(|| self.target.channel.and_then(|c| state.nick_in(c, user.id)))
            .unwrap_or_else(|| user.display_name().to_owned());
        let picture = match (member.as_ref().and_then(|m| m.avatar.as_deref()), self.target.guild) {
            (Some(hash), Some(guild)) => Some((discord::cdn::member_avatar_url(guild.0, user.id.0, hash, 128), hash)),
            _ => user.avatar.as_deref().map(|hash| (discord::cdn::avatar_url(user.id.0, hash, 128), hash)),
        };
        let status = self.target.presence.clone().or_else(|| {
            let friend = state.relationships.get(user.id).is_some_and(|r| r.kind == Relation::Friend);
            friend.then(|| state.relationships.presence(user.id))
        });

        let header = gtk::Box::builder().spacing(12).margin_top(12).margin_start(12).margin_end(12).build();
        let avatar = gtk::Overlay::builder()
            .child(&avatar_from(&name, AVATAR, Some(&app.images), picture, Row::None))
            .valign(gtk::Align::Start)
            .overflow(gtk::Overflow::Visible)
            .build();
        // What they put around it, a little larger than it is, and not in
        // the way of anything.
        let decoration = held.as_ref().and_then(|p| p.decoration.as_ref());
        if let Some(url) = decoration.and_then(|d| d.url(now_ms() / 1000, 256)) {
            let size = AVATAR * 6 / 5;
            let (frame, picture) = fixed_picture(size, size, gtk::ContentFit::Contain);
            frame.set_halign(gtk::Align::Center);
            frame.set_valign(gtk::Align::Center);
            frame.set_can_target(false);
            app.images.set(&picture, &url);
            avatar.add_overlay(&frame);
            avatar.set_measure_overlay(&frame, false);
        }
        if let Some(status) = &status {
            avatar.add_overlay(&presence_dot(status.status));
        }
        header.append(&avatar);
        let names = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        let title = gtk::Box::builder().spacing(6).build();
        title.append(&gtk::Label::builder().label(&name).xalign(0.0).wrap(true).css_classes(["title-3"]).build());
        if user.bot {
            title.append(
                &gtk::Label::builder().label("APP").valign(gtk::Align::Center).css_classes(["caption"]).build(),
            );
        }
        names.append(&title);
        let pronouns = shown.as_ref().map_or("", |s| s.pronouns);
        let username = match pronouns.is_empty() {
            true => user.username.clone(),
            false => format!("{} · {pronouns}", user.username),
        };
        names.append(&caption(&username, &["dimmed"]));
        if let Some(Presence { text: Some(text), .. }) = &status {
            names.append(&caption(text, &["caption"]));
        } else if let Some(status) = status.as_ref().filter(|s| s.status != Status::Offline) {
            names.append(&caption(status.status.label(), &["caption", "dimmed"]));
        }
        header.append(&names);
        column.append(&header);

        if let Some(held) = &held {
            let badges: Vec<&Badge> = held.badges.iter().chain(&held.server_badges).collect();
            if !badges.is_empty() {
                let row = gtk::Box::builder().spacing(6).margin_start(12).margin_end(12).build();
                for badge in badges {
                    row.append(&badge_picture(app, badge));
                }
                column.append(&row);
            }
        }
        if held.is_none() && state.profiles.failed(user.id) {
            let label = caption("Couldn't load their profile.", &["caption", "dimmed"]);
            label.set_margin_start(12);
            label.set_margin_end(12);
            column.append(&label);
        }

        if let Some(shown) = shown.filter(|s| !s.bio.is_empty()) {
            let spans = discord::markup::parse(shown.bio);
            let names = Names::default();
            let cx = Context::new(&names, Some(&app.images));
            let bio = render::render(&spans, &cx, false);
            bio.set_margin_start(12);
            bio.set_margin_end(12);
            // The popout has no heading over it (`hideHeading` in 442228).
            column.append(&match self.is_panel() {
                true => section("Bio", &bio),
                false => bio,
            });
        }

        column.append(&self.since(&state, user, guild, member.as_ref().and_then(|m| m.joined_at)));

        if let (Some(guild), Some(member)) = (guild, &member) {
            let roles = profile::listed_roles(&guild.roles, guild.id, &member.roles);
            if !roles.is_empty() {
                let flow = gtk::FlowBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .homogeneous(false)
                    .column_spacing(6)
                    .row_spacing(2)
                    .max_children_per_line(20)
                    .margin_start(12)
                    .margin_end(12)
                    .build();
                for role in roles {
                    let dot = match guild.role_colour(role) {
                        Some(colour) => render::coloured("●", &colour),
                        None => "●".to_owned(),
                    };
                    let label = gtk::Label::builder()
                        .use_markup(true)
                        .label(format!("{dot} {}", glib::markup_escape_text(&role.name)))
                        .xalign(0.0)
                        .build();
                    flow.append(&label);
                }
                column.append(&section("Roles", &flow));
            }
        }

        for list in self.mutual(app, &state, user.id) {
            column.append(&list);
        }

        column.append(&self.note_box(app, &state));
        if !self.is_panel() && !is_me(app, user) {
            let message =
                gtk::Button::builder().label("Message").margin_start(12).margin_end(12).margin_bottom(12).build();
            message.connect_clicked({
                let (app, host, user) = (Rc::downgrade(app), self.close_handle(), user.id);
                move |_| {
                    host();
                    if let Some(app) = app.upgrade() {
                        app.message_user(user);
                    }
                }
            });
            column.append(&message);
        } else {
            column.set_margin_bottom(12);
        }
        column.upcast()
    }

    /// Closes the popout, where this is one.
    fn close_handle(&self) -> impl Fn() + 'static {
        let popover = match &self.host {
            Host::Popout(popover) => Some(popover.clone()),
            Host::Panel(_) => None,
        };
        move || {
            if let Some(popover) = &popover {
                popover.popdown();
            }
        }
    }

    /// When they made their account, and joined this server: the client's
    /// Member Since, or Created On for an app (743987).
    fn since(&self, _state: &State, user: &User, guild: Option<&discord::Guild>, joined: Option<i64>) -> gtk::Widget {
        let lines = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_start(12)
            .margin_end(12)
            .build();
        let made = date(user.id.unix_ms() / 1000);
        match (guild, joined) {
            (Some(guild), Some(joined)) => {
                lines.append(&caption(&format!("Discord · {made}"), &[]));
                lines.append(&caption(&format!("{} · {}", guild.name, date(joined)), &[]));
            }
            _ => lines.append(&caption(&made, &[])),
        }
        section(if user.bot { "Created On" } else { "Member Since" }, &lines)
    }

    /// The servers and friends we share, each a section that opens on its
    /// list. The panel's friends are asked for as theirs opens.
    fn mutual(self: &Rc<Self>, app: &Rc<App>, state: &State, user: Snowflake) -> Vec<gtk::Widget> {
        let mut out = Vec::new();
        let servers: Vec<(&discord::Guild, Option<&str>)> = (state.profiles.mutual_guilds(user).into_iter().flatten())
            .filter_map(|m| Some((state.guilds.iter().find(|g| g.id == m.id)?, m.nick.as_deref())))
            .collect();
        if !servers.is_empty() {
            let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            for (guild, nick) in &servers {
                let row = gtk::Box::builder().spacing(8).build();
                let (icon, picture) = fixed_picture(24, 24, gtk::ContentFit::Cover);
                if let Some(hash) = &guild.icon {
                    app.images.set(&picture, &discord::cdn::guild_icon_url(guild.id.0, hash, 64));
                }
                row.append(&icon);
                row.append(
                    &gtk::Label::builder()
                        .label(&guild.name)
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .build(),
                );
                if let Some(nick) = nick {
                    row.append(&caption(nick, &["caption", "dimmed"]));
                }
                let button = gtk::Button::builder().child(&row).css_classes(["flat"]).build();
                button.connect_clicked({
                    let (app, close, guild) = (Rc::downgrade(app), self.close_handle(), guild.id);
                    move |_| {
                        close();
                        if let Some(app) = app.upgrade() {
                            app.select_guild(guild);
                        }
                    }
                });
                list.append(&button);
            }
            out.push(self.expander("servers", "Mutual Server", "Mutual Servers", servers.len(), &list, None));
        }
        let count = state.profiles.mutual_friend_count(user).map(|n| n as usize);
        if count.is_some_and(|n| n > 0) && !state.profiles.get(user).is_some_and(|p| p.user.bot) {
            let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            match state.profiles.mutual_friends(user).filter(|friends| friends.len() == count.unwrap_or_default()) {
                Some(friends) => {
                    for friend in friends {
                        list.append(&self.friend_row(app, state, friend));
                    }
                }
                None => list.append(&gtk::Spinner::builder().spinning(true).margin_top(6).margin_bottom(6).build()),
            }
            let asks = self.is_panel().then_some(user);
            out.push(self.expander(
                "friends",
                "Mutual Friend",
                "Mutual Friends",
                count.unwrap_or_default(),
                &list,
                asks,
            ));
        }
        out
    }

    fn friend_row(self: &Rc<Self>, app: &Rc<App>, state: &State, friend: &User) -> gtk::Widget {
        // What we call them, as the friends page does.
        let name = state
            .relationships
            .get(friend.id)
            .map_or_else(|| friend.display_name().to_owned(), |r| r.name().to_owned());
        let row = gtk::Box::builder().spacing(8).build();
        row.append(&avatar_widget(friend, 24, Some(&app.images), Row::None));
        row.append(&gtk::Label::builder().label(&name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build());
        let button = gtk::Button::builder().child(&row).css_classes(["flat"]).build();
        button.connect_clicked({
            let (app, close, friend) = (Rc::downgrade(app), self.close_handle(), friend.clone());
            move |button| {
                close();
                if let Some(app) = app.upgrade() {
                    let target = UserTarget {
                        user: friend.clone(),
                        guild: None,
                        channel: None,
                        presence: None,
                        anchor: button.clone().upcast(),
                    };
                    app.show_profile(&target, false);
                }
            }
        });
        button.upcast()
    }

    /// A section that opens on `list`, the popout's as a count ("2 Mutual
    /// Friends"), the panel's headed ("Mutual Friends — 2"). It asks for the
    /// friends of `asks` once opened.
    fn expander(
        self: &Rc<Self>,
        key: &'static str,
        one: &str,
        many: &str,
        count: usize,
        list: &gtk::Box,
        asks: Option<Snowflake>,
    ) -> gtk::Widget {
        let label = match (self.is_panel(), count) {
            (true, n) => format!("{many} — {n}"),
            (false, 1) => format!("1 {one}"),
            (false, n) => format!("{n} {many}"),
        };
        let scroller = gtk::ScrolledWindow::builder()
            .child(list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(180)
            .build();
        let expander = gtk::Expander::builder()
            .label(label)
            .child(&scroller)
            .expanded(self.open.borrow().contains(key))
            .margin_start(12)
            .margin_end(12)
            .build();
        let (view, app) = (Rc::downgrade(self), self.app_handle());
        expander.connect_expanded_notify(move |expander| {
            let Some(view) = view.upgrade() else { return };
            match expander.is_expanded() {
                true => view.open.borrow_mut().insert(key),
                false => view.open.borrow_mut().remove(key),
            };
            if let (true, Some(user), Some(app)) = (expander.is_expanded(), asks, app()) {
                app.ask_friend_list(user);
            }
            // Once it is laid out with its list.
            glib::idle_add_local_once(move || view.resized());
        });
        if expander.is_expanded()
            && let Some(user) = asks
            && let Some(app) = self.app_handle()()
        {
            // Open already: its list is asked for if a rebuild lost it.
            glib::idle_add_local_once(move || app.ask_friend_list(user));
        }
        expander.upcast()
    }

    /// The app, from a widget's closure.
    fn app_handle(&self) -> impl Fn() -> Option<Rc<App>> + 'static {
        let app = self.app.clone();
        move || app.upgrade()
    }

    /// Our note on them: written where it was loaded, sent when the box is
    /// left or Enter is pressed, and only if it changed.
    fn note_box(self: &Rc<Self>, app: &Rc<App>, state: &State) -> gtk::Widget {
        let user = self.target.user.id;
        let entry = gtk::Entry::builder()
            .placeholder_text(match state.notes.is_loading(user) {
                true => "Loading…",
                false => "Click to add a note",
            })
            .max_length(discord::notes::MAX_LEN as i32)
            .sensitive(!state.notes.is_loading(user))
            .margin_start(12)
            .margin_end(12)
            .build();
        let held = state.notes.get(user).unwrap_or_default().to_owned();
        entry.set_text(&held);
        entry.connect_changed({
            let (view, held) = (Rc::downgrade(self), held.clone());
            move |entry| {
                if let Some(view) = view.upgrade() {
                    view.editing.set(entry.text() != held);
                }
            }
        });
        let send = {
            let (app, view, sent) = (Rc::downgrade(app), Rc::downgrade(self), std::cell::RefCell::new(held));
            move |entry: &gtk::Entry| {
                let text = entry.text().to_string();
                let Some(app) = app.upgrade() else { return };
                if let Some(view) = view.upgrade() {
                    view.editing.set(false);
                }
                if *sent.borrow() == text {
                    return;
                }
                *sent.borrow_mut() = text.clone();
                let Some(rest) = app.state.borrow().rest.clone() else { return };
                glib::spawn_future_local(async move {
                    let _ = rest.set_note(user, &text).await;
                });
            }
        };
        let send = Rc::new(send);
        entry.connect_activate({
            let send = send.clone();
            move |entry| send(entry)
        });
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave({
            let (send, entry) = (send.clone(), entry.downgrade());
            move |_| {
                if let Some(entry) = entry.upgrade() {
                    send(&entry);
                }
            }
        });
        entry.add_controller(focus);
        if self.note {
            let entry = entry.clone();
            glib::idle_add_local_once(move || {
                entry.grab_focus();
            });
        }
        section("Note", &entry)
    }
}

/// A heading over its body.
fn section(heading: &str, body: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
    column.append(
        &gtk::Label::builder()
            .label(heading)
            .xalign(0.0)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["caption-heading", "dimmed"])
            .build(),
    );
    column.append(body);
    column.upcast()
}

fn badge_picture(app: &Rc<App>, badge: &Badge) -> gtk::Widget {
    let (frame, picture) = fixed_picture(BADGE, BADGE, gtk::ContentFit::Contain);
    frame.set_tooltip_text(Some(&badge.description));
    app.images.set(&picture, &discord::cdn::badge_icon_url(&badge.icon));
    frame
}

/// A picture held to `width` by `height` (`-1`: as wide as it is given),
/// however large its image is: a `Picture` asks for its image's own size,
/// which a size request only raises.
fn fixed_picture(width: i32, height: i32, fit: gtk::ContentFit) -> (gtk::Widget, gtk::Picture) {
    let frame = gtk::Overlay::builder()
        .child(&gtk::Box::builder().width_request(width).height_request(height).build())
        .overflow(gtk::Overflow::Hidden)
        .build();
    let picture = gtk::Picture::builder().content_fit(fit).can_shrink(true).build();
    frame.add_overlay(&picture);
    (frame.upcast(), picture)
}
