//! The quick switcher (Ctrl+K): a box over the window that finds a server,
//! channel, DM or person from what we already hold and opens it. What it
//! ranks, and how, is [`discord::switcher`]; this draws it and gathers its
//! sources from the session.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use discord::animation::Subject;
use discord::channel_icon::{self, Glyph};
use discord::notifications::{Level, Place};
use discord::switcher::{self, Hit, Home, Kind, Row, Sources};
use discord::{Channel, ChannelKind, Snowflake, User, threads};
use gtk::{gdk, glib, pango};

use super::App;
use super::state::{State, now_unix};
use super::widgets::{avatar_widget, round_picture};
use crate::images::Row as Mover;

/// The box, open over the window.
pub(super) struct View {
    dialog: adw::Dialog,
    entry: gtk::Entry,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    /// Shown under the list when nothing matches.
    none: gtk::Label,
    /// What each row of `list` is.
    rows: RefCell<Vec<Row>>,
    /// The row chosen with the keys or the pointer.
    selected: Cell<Option<usize>>,
}

impl View {
    fn new(app: &Rc<App>) -> Rc<View> {
        let entry = gtk::Entry::builder()
            .placeholder_text("Where would you like to go?")
            .accessible_role(gtk::AccessibleRole::SearchBox)
            .activates_default(false)
            .build();
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        let scroller =
            gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let none = gtk::Label::builder()
            .label("Can’t seem to find what you’re looking for?")
            .css_classes(["dimmed"])
            .margin_top(24)
            .margin_bottom(24)
            .visible(false)
            .build();
        let hint = gtk::Label::builder()
            .label("Start searches with @ users, # text channels, ! voice channels, * servers to narrow results.")
            .css_classes(["caption", "dimmed"])
            .wrap(true)
            .xalign(0.0)
            .build();
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&entry);
        content.append(&scroller);
        content.append(&none);
        content.append(&hint);
        let dialog = adw::Dialog::builder()
            .title("Quick Switcher")
            .content_width(560)
            .content_height(480)
            .focus_widget(&entry)
            .child(&content)
            .build();
        let view =
            Rc::new(View { dialog, entry, list, scroller, none, rows: RefCell::default(), selected: Cell::new(None) });

        view.entry.connect_changed({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                view.refresh(&app, true);
            }
        });
        // Enter chooses; the arrows and Ctrl+N and Ctrl+P move; Escape
        // clears the box, then closes it; Ctrl+K closes it.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_, key, _, modifiers| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else {
                    return glib::Propagation::Proceed;
                };
                view.key(&app, key, modifiers)
            }
        });
        view.dialog.add_controller(keys);
        view.list.connect_row_activated({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_, row| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                if let Ok(i) = usize::try_from(row.index()) {
                    view.choose(&app, i);
                }
            }
        });
        view
    }

    fn key(self: &Rc<Self>, app: &Rc<App>, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        match key {
            gdk::Key::Escape if !self.entry.text().is_empty() => self.entry.set_text(""),
            gdk::Key::k | gdk::Key::t if control => self.dialog.force_close(),
            gdk::Key::Down => self.step(true),
            gdk::Key::Up => self.step(false),
            gdk::Key::n if control => self.step(true),
            gdk::Key::p if control => self.step(false),
            gdk::Key::Return | gdk::Key::KP_Enter if !alt => {
                if let Some(i) = self.selected.get() {
                    self.choose(app, i);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn step(&self, down: bool) {
        let next = switcher::step(&self.rows.borrow(), self.selected.get(), down);
        self.select(next);
    }

    fn select(&self, index: Option<usize>) {
        self.selected.set(index);
        let row = index.and_then(|i| self.list.row_at_index(i as i32));
        match &row {
            Some(row) => self.list.select_row(Some(row)),
            None => self.list.unselect_all(),
        }
        let Some(row) = row else { return };
        // Keep the chosen row in view.
        if let Some(bounds) = row.compute_bounds(&self.list) {
            let adjustment = self.scroller.vadjustment();
            let (top, bottom) = (f64::from(bounds.y()), f64::from(bounds.y() + bounds.height()));
            if top < adjustment.value() {
                adjustment.set_value(top);
            } else if bottom > adjustment.value() + adjustment.page_size() {
                adjustment.set_value(bottom - adjustment.page_size());
            }
        }
    }

    /// Search again for what is typed. A new query starts on its first
    /// result; a refresh for changed data keeps the chosen one.
    fn refresh(self: &Rc<Self>, app: &Rc<App>, new_query: bool) {
        let typed = self.entry.text();
        let kept = self.selected.get().and_then(|i| self.rows.borrow().get(i).cloned());
        let rows = app.switcher_rows(&switcher::parse(&typed));
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for row in &rows {
            let row = match row {
                Row::Header(text) => heading(text),
                Row::Hit(hit) => app.switcher_row(hit),
            };
            self.list.append(&row);
        }
        self.none.set_visible(rows.is_empty() && !typed.is_empty());
        let selected = match kept {
            Some(Row::Hit(kept)) if !new_query => {
                rows.iter().position(|r| matches!(r, Row::Hit(h) if h.kind == kept.kind && h.id == kept.id))
            }
            _ => None,
        }
        .or_else(|| switcher::first_selectable(&rows));
        *self.rows.borrow_mut() = rows;
        self.select(selected);
        if new_query {
            self.scroller.vadjustment().set_value(0.0);
        }
    }

    fn choose(self: &Rc<Self>, app: &Rc<App>, index: usize) {
        let Some(Row::Hit(hit)) = self.rows.borrow().get(index).cloned() else { return };
        self.dialog.force_close();
        match hit.kind {
            Kind::Server => app.select_guild(hit.id),
            Kind::Channel | Kind::Voice | Kind::GroupDm => app.open_channel(hit.id),
            Kind::User => app.message_user(hit.id),
        }
    }
}

fn heading(text: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .margin_top(12)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    gtk::ListBoxRow::builder().child(&label).activatable(false).selectable(false).build()
}

/// Who spoke in `channel`'s loaded messages, newest first, without us and
/// without webhooks and Discord's own account; in a server only members we
/// know of (`getRecentlyTalked`, 997625).
fn speakers(app: &App, state: &State) -> Vec<User> {
    let Some(channel) = state.channel else { return Vec::new() };
    let guild = state.find_channel(channel).and_then(|(g, _)| g.map(|g| g.id));
    let store = app.messages();
    let mut seen = HashSet::new();
    (0..store.n_items())
        .rev()
        .filter_map(|i| crate::timelines::message_at(&store, i))
        .map(|m| m.author)
        .filter(|u| !u.non_user_bot() && seen.insert(u.id))
        .filter(|u| guild.is_none_or(|g| state.members.roles(g, u.id).is_some()))
        .collect()
}

/// The ids "Mentions" and "Unread Channels" list (see [`Home`]).
fn unread_and_mentions(state: &State) -> (Vec<Snowflake>, Vec<Snowflake>) {
    let now = now_unix();
    let mut mentions: Vec<&Channel> = state
        .guilds
        .iter()
        .flat_map(|g| g.channels.iter().chain(&g.threads))
        .chain(&state.private_channels)
        .filter(|c| state.read.mentions(c.id) > 0)
        .collect();
    // The client lists them newest-first, in the order the mentions came.
    mentions.sort_by_key(|c| c.activity());
    let mentions = mentions.into_iter().map(|c| c.id).collect();
    let Some(guild) = state.guild.and_then(|id| state.guilds.iter().find(|g| g.id == id)) else {
        return (mentions, Vec::new());
    };
    let place = |c: &Channel| Place {
        guild: Some(guild.id),
        guild_default: guild.default_message_notifications,
        channel: c.id,
        parent: c.parent_id,
    };
    let mut channels: Vec<&Channel> = guild
        .channels
        .iter()
        .filter(|c| {
            matches!(c.kind, ChannelKind::Text | ChannelKind::Announcement | ChannelKind::Forum | ChannelKind::Media)
        })
        .filter(|c| state.can_view(c.id))
        .collect();
    channels.sort_by_key(|c| c.position.unwrap_or(0));
    let mut unread: Vec<Snowflake> = channels
        .into_iter()
        .filter(|c| {
            let place = place(c);
            state.is_unread(c)
                && !state.notifications.channel_muted(&place, now)
                && state.notifications.level(&place) == Level::All
        })
        .map(|c| c.id)
        .collect();
    unread.extend(
        threads::joined_active(guild)
            .filter(|t| state.can_view(t.id) && state.is_unread(t) && !State::thread_muted(t))
            .map(|t| t.id),
    );
    (mentions, unread)
}

impl App {
    /// Open the box, or close it if it is open.
    pub(super) fn toggle_switcher(self: &Rc<Self>) {
        if let Some(open) = self.switcher.take() {
            open.dialog.force_close();
            return;
        }
        let ready =
            self.account.state.borrow().me().is_some() && self.stack.visible_child_name().as_deref() == Some("main");
        if !ready {
            return;
        }
        let view = View::new(self);
        *self.switcher.borrow_mut() = Some(view.clone());
        view.dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |closed| {
                let Some(this) = this.upgrade() else { return };
                let ours = this.switcher.borrow().as_ref().is_some_and(|v| v.dialog == *closed);
                if ours {
                    this.switcher.take();
                }
            }
        });
        view.dialog.present(Some(&self.window));
        view.refresh(self, true);
    }

    /// What the box lists for `query`, from the session as it is now.
    fn switcher_rows(&self, query: &switcher::Query) -> Vec<Row> {
        let state = self.account.state.borrow();
        let Some(me) = state.me() else { return Vec::new() };
        let now = now_unix();
        let account = state.account.as_ref();
        let can_access = |guild: &discord::Guild, channel: &Channel| {
            account.is_some_and(|account| guild.can_view(channel, account, now))
        };
        let is_member = |guild, user| state.members.roles(guild, user).is_some();
        let sources = Sources {
            me,
            guild: state.guild,
            channel: state.channel,
            guilds: &state.guilds,
            private_channels: &state.private_channels,
            relationships: &state.relationships,
            can_access: &can_access,
            is_member: &is_member,
        };
        let (mentions, unread) = unread_and_mentions(&state);
        let speakers = speakers(self, &state);
        let home = Home { history: &state.quick_history, mentions: &mentions, unread: &unread, speakers: &speakers };
        switcher::search(&sources, &home, query)
    }

    /// A result's row: what it is, its name, and where it is.
    fn switcher_row(&self, hit: &Hit) -> gtk::ListBoxRow {
        let state = self.account.state.borrow();
        let line =
            gtk::Box::builder().spacing(10).margin_top(6).margin_bottom(6).margin_start(12).margin_end(12).build();
        let mut context = String::new();
        let icon: gtk::Widget = match hit.kind {
            Kind::User => {
                let user = state.relationships.get(hit.id).map(|r| r.user.clone()).or_else(|| {
                    state.private_channels.iter().flat_map(|c| &c.recipients).find(|u| u.id == hit.id).cloned()
                });
                match user {
                    Some(user) => {
                        if user.username != hit.comparator {
                            context = user.username.clone();
                        }
                        avatar_widget(&user, 32, Some(&self.images), Mover::Parent)
                    }
                    None => adw::Avatar::new(32, Some(&hit.comparator), true).upcast(),
                }
            }
            Kind::GroupDm => match state.private_channels.iter().find(|c| c.id == hit.id) {
                Some(group) => super::sidebar::dm_icon(group, &self.images),
                None => adw::Avatar::new(32, Some(&hit.comparator), true).upcast(),
            },
            Kind::Server => {
                context = "Server".to_owned();
                let icon = state.guilds.iter().find(|g| g.id == hit.id).and_then(|g| g.icon.as_deref());
                match icon {
                    Some(hash) => {
                        let picture = round_picture(32);
                        let url = discord::cdn::guild_icon_url(hit.id.0, hash, 64);
                        if discord::cdn::is_animated(hash) {
                            let still = discord::cdn::guild_icon_still_url(hit.id.0, hash, 64);
                            self.images.set_moving(&picture, &still, &url, Subject::ServerIcon, Mover::None);
                        } else {
                            self.images.set(&picture, &url);
                        }
                        picture.upcast()
                    }
                    None => adw::Avatar::new(32, Some(&hit.comparator), true).upcast(),
                }
            }
            Kind::Channel | Kind::Voice => {
                let found = state.find_channel(hit.id);
                if let Some((Some(guild), channel)) = found {
                    let parent = channel.parent_id.and_then(|p| guild.find(p)).and_then(|p| p.name.as_deref());
                    context = [parent, Some(guild.name.as_str())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                }
                let glyph = found
                    .and_then(|(g, c)| g.and_then(|g| channel_icon::of(g, c)))
                    .map_or(if hit.kind == Kind::Voice { Glyph::Voice } else { Glyph::Text }, |i| i.glyph);
                super::sidebar::glyph_widget(glyph)
            }
        };
        icon.set_valign(gtk::Align::Center);
        line.append(&icon);
        line.append(
            &gtk::Label::builder()
                .label(&hit.comparator)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(pango::EllipsizeMode::End)
                .build(),
        );
        if !context.is_empty() {
            line.append(
                &gtk::Label::builder()
                    .label(&context)
                    .css_classes(["caption", "dimmed"])
                    .ellipsize(pango::EllipsizeMode::End)
                    .build(),
            );
        }
        gtk::ListBoxRow::builder().child(&line).build()
    }
}

/// Keep `channel` in "Previous Channels".
pub(super) fn visited(state: &mut State, channel: Snowflake) {
    switcher::visited(&mut state.quick_history, channel);
}
