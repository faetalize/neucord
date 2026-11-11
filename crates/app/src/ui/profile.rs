//! A user's profile, in the popout the client opens from a name, an avatar,
//! a mention or a member list row (`342296`, drawn by `348434`), and the
//! menu on those places (`user-context`, 841549). Asking for the profile and
//! keeping it is [`discord::profile`]'s; this draws what it holds.

use std::rc::Rc;

use adw::prelude::*;
use discord::profile::{self, Badge, Banner, Fetch, Profile, Request};
use discord::relationships::{Kind as Relation, Presence, Status};
use discord::{Snowflake, User};
use gtk::glib;

use super::App;
use super::friends::presence_dot;
use super::menus::{Choice, Choices, Entry, Item, Kind, Style};
use super::state::now_ms;
use super::widgets::avatar_from;
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

/// The widest the popout is (`UserPopout` is 340px in 348434's styles).
const WIDTH: i32 = 340;
const BANNER_HEIGHT: i32 = 100;
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

/// The popout on screen: what it shows is built again as answers arrive.
struct Popout {
    popover: gtk::Popover,
    target: UserTarget,
    request: Request,
    /// The note box takes the focus once it shows.
    note: bool,
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

    /// Open `target`'s profile, asking for it as the client does: not at all
    /// while it holds one that is current, in the background while it holds
    /// an old one, and before showing anything if it holds none.
    pub(super) fn show_profile(self: &Rc<Self>, target: &UserTarget, note: bool) {
        if !is_person(target) {
            return;
        }
        let (me, rest) = {
            let state = self.state.borrow();
            (state.me(), state.rest.clone())
        };
        let (Some(me), Some(rest)) = (me, rest) else { return };
        let request = {
            let state = self.state.borrow();
            let role = target.guild.and_then(|g| state.connections_role(g, target.user.id, target.channel));
            Request::popout(&target.user, me, target.guild, role)
        };
        let plan = self.state.borrow().profiles.plan(&request, now_ms() as u64);

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
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        let target = UserTarget {
            user: target.user.clone(),
            guild: target.guild,
            channel: target.channel,
            presence: target.presence.clone(),
            anchor: target.anchor.clone(),
        };
        let popout = Rc::new(Popout { popover, target, request, note });
        popout.fill(self, plan == Fetch::Before);
        popout.popover.popup();

        self.load_note_then(popout.target.user.id, {
            let (this, popout) = (Rc::downgrade(self), popout.clone());
            move || {
                if let Some(this) = this.upgrade() {
                    popout.fill(&this, false);
                }
            }
        });
        if plan == Fetch::No {
            return;
        }
        self.state.borrow_mut().profiles.asked(&popout.request);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let answer = rest.profile(&popout.request).await;
            let now = now_ms() as u64;
            {
                let mut state = this.state.borrow_mut();
                match answer {
                    Ok(profile) => state.profiles.answered(&popout.request, profile, now),
                    Err(e) => {
                        let status = match &e {
                            discord::Error::Http { status, .. } => Some(*status),
                            _ => None,
                        };
                        state.profiles.refused(&popout.request, status, now);
                    }
                }
            }
            popout.fill(&this, false);
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

    /// Our note on `user` changed, from another of our sessions.
    pub(super) fn on_note(self: &Rc<Self>, user: Snowflake, note: Option<String>) {
        self.state.borrow_mut().notes.loaded(user, note);
    }

    /// Write `user`'s mention into the message box.
    pub(super) fn mention_user(self: &Rc<Self>, user: Snowflake) {
        let buffer = self.composer.buffer();
        buffer.insert_at_cursor(&format!("<@{user}> "));
        self.composer.grab_focus();
    }
}

impl Popout {
    /// Show what we hold now, or a spinner while nothing is held to show.
    fn fill(self: &Rc<Self>, app: &Rc<App>, waiting: bool) {
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
        self.popover.set_child(Some(&body));
    }

    fn body(self: &Rc<Self>, app: &Rc<App>) -> gtk::Widget {
        let state = app.state.borrow();
        let held = state.profiles.get(self.target.user.id).cloned();
        // What Discord says of them replaces what we knew when it was clicked.
        let user = &held.as_ref().map_or_else(|| self.target.user.clone(), |p| p.user.clone());
        let guild = self.target.guild.and_then(|g| state.guilds.iter().find(|x| x.id == g));
        let column =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).width_request(WIDTH).build();

        let shown = held.as_ref().map(Profile::shown);
        if let Some(banner) = shown.as_ref().and_then(|s| s.banner) {
            let (url, _) = match banner {
                Banner::User(hash) => (discord::cdn::banner_url(user.id.0, hash, 600), hash),
                Banner::Server { guild, hash } => {
                    (discord::cdn::member_banner_url(guild.0, user.id.0, hash, 600), hash)
                }
            };
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .height_request(BANNER_HEIGHT)
                .overflow(gtk::Overflow::Hidden)
                .build();
            app.images.set(&picture, &url, false);
            column.append(&picture);
        }

        let member = held.as_ref().and_then(|p| p.member.clone());
        let name = member
            .as_ref()
            .and_then(|m| m.nick.clone())
            .or_else(|| state.relationships.get(user.id).and_then(|r| r.nickname.clone()))
            .unwrap_or_else(|| user.display_name().to_owned());
        let picture = match (&member.as_ref().and_then(|m| m.avatar.as_deref()), self.target.guild) {
            (Some(hash), Some(guild)) => Some((discord::cdn::member_avatar_url(guild.0, user.id.0, hash, 128), *hash)),
            _ => user.avatar.as_deref().map(|hash| (discord::cdn::avatar_url(user.id.0, hash, 128), hash)),
        };
        let status = self.target.presence.clone().or_else(|| {
            let friend = state.relationships.get(user.id).is_some_and(|r| r.kind == Relation::Friend);
            friend.then(|| state.relationships.presence(user.id))
        });

        let header = gtk::Box::builder().spacing(12).margin_top(12).margin_start(12).margin_end(12).build();
        let avatar = gtk::Overlay::builder()
            .child(&avatar_from(&name, AVATAR, Some(&app.images), picture))
            .valign(gtk::Align::Start)
            .build();
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
        names.append(&gtk::Label::builder().label(&username).xalign(0.0).css_classes(["dimmed"]).build());
        if let Some(Presence { text: Some(text), .. }) = &status {
            names.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["caption"]).build());
        } else if let Some(status) = status.as_ref().filter(|s| s.status != Status::Offline) {
            names.append(
                &gtk::Label::builder()
                    .label(status.status.label())
                    .xalign(0.0)
                    .css_classes(["caption", "dimmed"])
                    .build(),
            );
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
            column.append(
                &gtk::Label::builder()
                    .label("Couldn't load their profile.")
                    .xalign(0.0)
                    .margin_start(12)
                    .margin_end(12)
                    .css_classes(["caption", "dimmed"])
                    .build(),
            );
        }

        if let Some(shown) = shown.filter(|s| !s.bio.is_empty()) {
            let spans = discord::markup::parse(shown.bio);
            let names = Names::default();
            let cx = Context::new(&names, Some(&app.images));
            let bio = render::render(&spans, &cx, false);
            bio.set_margin_start(12);
            bio.set_margin_end(12);
            column.append(&section("About Me", &bio));
        }

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

        let mutual = mutual_line(&state, user.id);
        if let Some(mutual) = mutual {
            column.append(&mutual);
        }

        column.append(&self.note_box(app, &state));
        if !is_me(app, user) {
            let message =
                gtk::Button::builder().label("Message").margin_start(12).margin_end(12).margin_bottom(12).build();
            message.connect_clicked({
                let (app, popover, user) = (Rc::downgrade(app), self.popover.clone(), user.id);
                move |_| {
                    popover.popdown();
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

    /// Our note on them: written where it was loaded, sent when the box is
    /// left or Enter is pressed, and only if it changed.
    fn note_box(self: &Rc<Self>, app: &Rc<App>, state: &super::state::State) -> gtk::Widget {
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
        let send = {
            let (app, sent) = (Rc::downgrade(app), std::cell::RefCell::new(held));
            move |entry: &gtk::Entry| {
                let text = entry.text().to_string();
                let Some(app) = app.upgrade() else { return };
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
    let picture = gtk::Picture::builder()
        .width_request(BADGE)
        .height_request(BADGE)
        .content_fit(gtk::ContentFit::Contain)
        .tooltip_text(&badge.description)
        .build();
    app.images.set(&picture, &discord::cdn::badge_icon_url(&badge.icon), false);
    picture.upcast()
}

/// How many friends and servers we share, as the popout counts them: the
/// servers we are in, by name on hover.
fn mutual_line(state: &super::state::State, user: Snowflake) -> Option<gtk::Widget> {
    let servers: Vec<&str> = state
        .profiles
        .mutual_guilds(user)
        .into_iter()
        .flatten()
        .filter_map(|m| state.guilds.iter().find(|g| g.id == m.id))
        .map(|g| g.name.as_str())
        .collect();
    let friends: Vec<&str> =
        state.profiles.mutual_friends(user).into_iter().flatten().map(User::display_name).collect();
    if friends.is_empty() && servers.is_empty() {
        return None;
    }
    let row = gtk::Box::builder().spacing(12).margin_start(12).margin_end(12).build();
    let count = |n: usize, one: &str, many: &str, names: &[&str]| {
        let text = match n {
            1 => format!("1 {one}"),
            n => format!("{n} {many}"),
        };
        gtk::Label::builder().label(text).tooltip_text(names.join(", ")).css_classes(["caption"]).build()
    };
    if !friends.is_empty() {
        row.append(&count(friends.len(), "Mutual Friend", "Mutual Friends", &friends));
    }
    if !servers.is_empty() {
        row.append(&count(servers.len(), "Mutual Server", "Mutual Servers", &servers));
    }
    Some(row.upcast())
}
