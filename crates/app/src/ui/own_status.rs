//! Our own status and custom status: shown in the account panel, set from
//! its menu as the official client's account popout does (166783 and the
//! status picker, 827827; the custom status dialog, 431273), and kept in
//! step with what Discord shows others (see [`discord::presence`]).
//!
//! Both are protobuf settings, so they sync to the account's other clients
//! and come back from them; what ends is ended here too, as the client's
//! custom status manager does (340137).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::presence::{Local, Send};
use discord::relationships::{Presence, Status};
use discord::settings::{ClearAfter, CustomStatus, StatusSettings, UserStatus};
use discord::{Setting, Snowflake};
use gtk::{gio, glib};

use super::App;
use super::state::now_ms;

/// How long the status picker offers a status for, `None` for ever (`id`,
/// 166783).
const DURATIONS: [(&str, Option<u64>); 6] = [
    ("For 15 Minutes", Some(15 * 60_000)),
    ("For 1 Hour", Some(60 * 60_000)),
    ("For 8 Hours", Some(8 * 60 * 60_000)),
    ("For 24 Hours", Some(24 * 60 * 60_000)),
    ("For 3 Days", Some(3 * 24 * 60 * 60_000)),
    ("Forever", None),
];

/// The emoji picked in the custom status dialog: a server's by id, else the
/// character.
type Picked = Option<(Option<Snowflake>, String)>;

/// The longest a custom status's text is (431273).
const CUSTOM_STATUS_LENGTH: i32 = 128;
/// How long after READY the client looks for what has ended
/// (`POST_CONNECTION_OPEN` fires 2 s after the connection opens, 225123).
const AFTER_READY: Duration = Duration::from_secs(2);
const EMOJI_SIZE: i32 = 16;
/// Our avatar in the account panel, as the client's (`SIZE_32`).
const ME_AVATAR: i32 = 32;

/// The window action's argument: `<status>:<duration in ms, 0 for ever>`.
fn action_target(status: UserStatus, duration_ms: Option<u64>) -> String {
    format!("{}:{}", status.wire(), duration_ms.unwrap_or(0))
}

fn parse_target(target: &str) -> Option<(UserStatus, Option<u64>)> {
    let (status, duration) = target.split_once(':')?;
    let status = UserStatus::ALL.into_iter().find(|s| s.wire() == status)?;
    Some((status, Some(duration.parse().ok()?).filter(|ms| *ms != 0)))
}

fn status_item(label: &str, status: UserStatus, duration_ms: Option<u64>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some("win.set-status"), Some(&action_target(status, duration_ms).to_variant()));
    item
}

/// The window action that turns Focus Mode on for the time it names, in
/// milliseconds (0 for ever), or off with `off`.
const FOCUS_ACTION: &str = "win.set-focus";

fn focus_item(label: &str, target: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(FOCUS_ACTION), Some(&target.to_variant()));
    item
}

/// Focus Mode's submenu (166783): the times it can last, as the status
/// picker's have, under what it does, or until when it is on; and a way to
/// turn it off while it is.
fn focus_menu(focus: Option<u64>) -> gio::MenuItem {
    let durations = gio::Menu::new();
    for (label, duration) in DURATIONS {
        durations.append_item(&focus_item(label, &duration.unwrap_or(0).to_string()));
    }
    let words = match focus {
        Some(0) => "ON".to_owned(),
        Some(ends) => until(ends).map_or_else(|| "ON".to_owned(), |until| format!("ON, {until}")),
        None => "Disables desktop notifications & message sounds.".to_owned(),
    };
    let submenu = gio::Menu::new();
    submenu.append_section(Some(&words), &durations);
    if focus.is_some() {
        let off = gio::Menu::new();
        off.append_item(&focus_item("Turn off Focus Mode", "off"));
        submenu.append_section(None, &off);
    }
    gio::MenuItem::new_submenu(Some("Focus Mode (BETA)"), &submenu)
}

/// The status picker's menu: Online, then each other status with the times
/// it can last, then Focus Mode, then the custom status. `focus` is when
/// Focus Mode ends (0 for never) while it is on.
fn menu(status: &StatusSettings, focus: Option<u64>) -> gio::Menu {
    let menu = gio::Menu::new();
    let statuses = gio::Menu::new();
    statuses.append_item(&status_item(UserStatus::Online.label(), UserStatus::Online, None));
    menu.append_section(None, &statuses);

    let others = gio::Menu::new();
    for which in [UserStatus::Idle, UserStatus::DoNotDisturb, UserStatus::Invisible] {
        let durations = gio::Menu::new();
        for (label, duration) in DURATIONS {
            durations.append_item(&status_item(label, which, duration));
        }
        others.append_submenu(Some(which.label()), &durations);
    }
    menu.append_section(None, &others);

    let focus_section = gio::Menu::new();
    focus_section.append_item(&focus_menu(focus));
    menu.append_section(None, &focus_section);

    let custom = gio::Menu::new();
    let label = if status.custom.is_some() { "Edit Custom _Status…" } else { "Set a Custom _Status…" };
    custom.append(Some(label), Some("win.custom-status"));
    if status.custom.is_some() {
        custom.append(Some("C_lear Custom Status"), Some("win.clear-custom-status"));
    }
    menu.append_section(None, &custom);
    menu
}

/// Our presence as our profile shows it: the status we set, and our
/// custom status.
pub(super) fn own_presence(status: &StatusSettings) -> Presence {
    let shown = match status.status {
        UserStatus::Online => Status::Online,
        UserStatus::Idle => Status::Idle,
        UserStatus::DoNotDisturb => Status::DoNotDisturb,
        UserStatus::Invisible => Status::Offline,
    };
    Presence { status: shown, text: status.custom.as_ref().map(|c| c.text.clone()), mobile: false }
}

/// A row of a card in our profile popout: an optional leading widget, the
/// label, and an arrow when it opens a menu.
fn popout_row(lead: Option<&gtk::Widget>, label: &str, opens: bool) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(10).build();
    if let Some(lead) = lead {
        content.append(lead);
    }
    content.append(
        &gtk::Label::builder().label(label).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build(),
    );
    if opens {
        content.append(&gtk::Image::from_icon_name("go-next-symbolic"));
    }
    gtk::ListBoxRow::builder().child(&content).css_classes(["popout-row"]).build()
}

/// Open `model` beside `row`; closing it closes the popout too, as choosing
/// from it does in the client.
fn submenu(row: &gtk::ListBoxRow, model: &gio::Menu, close: Rc<dyn Fn()>) {
    let menu = gtk::PopoverMenu::from_model(Some(model));
    menu.set_parent(row);
    menu.set_position(gtk::PositionType::Right);
    menu.connect_closed(move |menu| {
        close();
        let menu = menu.clone();
        glib::idle_add_local_once(move || menu.unparent());
    });
    menu.popup();
}

/// "Until 3:45 PM", "until tomorrow at 3:45 PM": when an expiring status
/// ends.
fn until(expires_at_ms: u64) -> Option<String> {
    let at = glib::DateTime::from_unix_local((expires_at_ms / 1000) as i64).ok()?;
    let today = glib::DateTime::now_local().ok()?;
    let time = at.format("%-I:%M %p").ok()?;
    Some(match (at.ymd(), today.ymd()) {
        (a, b) if a == b => format!("until {time}"),
        _ => format!("until {} {time}", at.format("%a").ok()?),
    })
}

impl App {
    /// The account panel's button: our avatar with the status at its
    /// corner, our name, and under it the custom status, else the status.
    /// As wide as that, and it opens our profile popout, as the client's
    /// account panel does.
    pub(super) fn build_status_button(self: &Rc<Self>) -> gtk::Button {
        let avatar = gtk::Overlay::builder().child(&self.me_avatar).valign(gtk::Align::Center).build();
        avatar.add_overlay(&self.me_dot);
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        text.append(&self.me_label);
        text.append(&self.me_status);
        let content = gtk::Box::builder().spacing(8).build();
        content.append(&avatar);
        content.append(&text);
        let button =
            gtk::Button::builder().child(&content).halign(gtk::Align::Start).css_classes(["flat", "me-button"]).build();
        button.connect_clicked({
            let this = Rc::downgrade(self);
            move |button| {
                if let Some(this) = this.upgrade() {
                    this.show_own_profile(button.upcast_ref());
                }
            }
        });
        *self.status_button.borrow_mut() = Some(button.clone());
        self.refresh_me_avatar();
        button
    }

    /// The cards under our profile in its popout, as the client's: our status,
    /// which opens the status menu, then Switch Accounts and, in Developer
    /// Mode, Copy User ID.
    pub(super) fn own_popout_actions(self: &Rc<Self>, close: impl Fn() + 'static) -> gtk::Widget {
        let close: Rc<dyn Fn()> = Rc::new(close);
        let (status, focus, developer, me) = {
            let state = self.account.state.borrow();
            let settings = &state.settings;
            let focus = settings.focus_mode(now_ms() as u64).then_some(settings.notifications.focus_expires_at_ms);
            (settings.status.clone(), focus, settings.developer_mode, state.me())
        };
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(12)
            .margin_end(12)
            .build();

        let card =
            || gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        let statuses = card();
        let dot = super::friends::presence_dot(&own_presence(&status));
        dot.set_valign(gtk::Align::Center);
        statuses.append(&popout_row(Some(&dot), status.status.label(), true));
        statuses.connect_row_activated({
            let close = close.clone();
            let model = menu(&status, focus);
            move |_, row| submenu(row, &model, close.clone())
        });
        column.append(&statuses);

        let accounts = card();
        accounts.append(&popout_row(None, "Switch Accounts", true));
        if developer {
            accounts.append(&popout_row(None, "Copy User ID", false));
        }
        accounts.connect_row_activated({
            let this = Rc::downgrade(self);
            move |_, row| {
                let Some(this) = this.upgrade() else { return };
                match row.index() {
                    0 => {
                        // The client asks who each account is when its list opens.
                        this.validate_accounts();
                        submenu(row, &this.switch_menu(), close.clone());
                    }
                    _ => {
                        if let Some(me) = me {
                            this.copy(&me.to_string());
                        }
                        close();
                    }
                }
            }
        });
        column.append(&accounts);
        column.upcast()
    }

    /// Our avatar in the account panel, as it is now.
    pub(super) fn refresh_me_avatar(&self) {
        let me = self.account.state.borrow().me.clone();
        let name = me.as_ref().map_or("", |u| u.display_name());
        let picture = me.as_ref().and_then(|u| {
            let hash = u.avatar.as_deref()?;
            let size = super::widgets::asked(ME_AVATAR, Some(&self.images));
            Some((discord::cdn::avatar_url(u.id.0, hash, size), hash))
        });
        let avatar =
            super::widgets::avatar_from(name, ME_AVATAR, Some(&self.images), picture, crate::images::Row::None);
        while let Some(old) = self.me_avatar.first_child() {
            self.me_avatar.remove(&old);
        }
        self.me_avatar.append(&avatar);
    }

    pub(super) fn status_actions(self: &Rc<Self>) -> Vec<gio::ActionEntry<adw::ApplicationWindow>> {
        let this = Rc::downgrade(self);
        let set = gio::ActionEntry::builder("set-status")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate({
                let this = this.clone();
                move |_: &adw::ApplicationWindow, _, target| {
                    let parsed = target.and_then(|t| t.str()).and_then(parse_target);
                    if let (Some(this), Some((status, duration))) = (this.upgrade(), parsed) {
                        this.set_status(status, duration);
                    }
                }
            })
            .build();
        let custom = gio::ActionEntry::builder("custom-status")
            .activate({
                let this = this.clone();
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.show_custom_status();
                    }
                }
            })
            .build();
        let clear = gio::ActionEntry::builder("clear-custom-status")
            .activate({
                let this = this.clone();
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.change_setting(Setting::CustomStatus(None));
                    }
                }
            })
            .build();
        let focus = gio::ActionEntry::builder("set-focus")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(move |_: &adw::ApplicationWindow, _, target| {
                let target = target.and_then(|t| t.str());
                let duration = target.and_then(|t| t.parse::<u64>().ok());
                if let Some(this) = this.upgrade() {
                    match (target, duration) {
                        (Some("off"), _) => this.set_focus_mode(false, None),
                        (_, Some(ms)) => this.set_focus_mode(true, Some(ms).filter(|ms| *ms != 0)),
                        _ => {}
                    }
                }
            })
            .build();
        vec![set, custom, clear, focus]
    }

    /// The status picker (827827): the status, and when it goes back to
    /// Online if it was given a time.
    fn set_status(self: &Rc<Self>, status: UserStatus, duration_ms: Option<u64>) {
        let now_ms = now_ms() as u64;
        let expires_at_ms = duration_ms.map_or(0, |d| now_ms + d);
        self.change_setting(Setting::Status { status, expires_at_ms, now_ms });
    }

    /// Focus Mode on for `duration_ms`, or for ever, or off (`ES` in
    /// 970931). Turning it on for ever while the status is Do Not Disturb
    /// asks whether to go Online, which silences as well.
    fn set_focus_mode(self: &Rc<Self>, on: bool, duration_ms: Option<u64>) {
        let expires_at_ms = duration_ms.map_or(0, |d| now_ms() as u64 + d);
        self.change_setting(Setting::FocusMode { on, expires_at_ms });
        if !(on && duration_ms.is_none() && self.account.state.borrow().settings.do_not_disturb()) {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Switch to Online?"),
            Some(
                "With Focus Mode, Discord will no longer show in-app, desktop notifications or play message sounds, \
                 just like with DND. Would you like to switch to Online?",
            ),
        );
        dialog.add_responses(&[("cancel", "No thanks"), ("confirm", "Okay")]);
        dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("cancel");
        dialog.connect_response(None, {
            let this = Rc::downgrade(self);
            move |_, response| {
                if let (Some(this), "confirm") = (this.upgrade(), response) {
                    this.set_status(UserStatus::Online, None);
                }
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Show the status in the account panel and its menu.
    pub(super) fn refresh_status(&self) {
        let status = self.account.state.borrow().settings.status.clone();
        let class = match status.status {
            UserStatus::Online => "online",
            UserStatus::Idle => "idle",
            UserStatus::DoNotDisturb => "dnd",
            UserStatus::Invisible => "offline",
        };
        self.me_dot.set_css_classes(&["presence", class]);
        self.refresh_group_member_rows();

        while let Some(child) = self.me_status.first_child() {
            self.me_status.remove(&child);
        }
        match &status.custom {
            Some(custom) => {
                if let Some(emoji) = self.status_emoji(custom) {
                    self.me_status.append(&emoji);
                }
                let text = gtk::Label::builder()
                    .label(&custom.text)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["caption", "dim-label"])
                    .build();
                self.me_status.append(&text);
            }
            None => {
                let text = gtk::Label::builder()
                    .label(status.status.label())
                    .xalign(0.0)
                    .css_classes(["caption", "dim-label"])
                    .build();
                self.me_status.append(&text);
            }
        }
        let tooltip = match (status.status, until(status.expires_at_ms).filter(|_| status.expires_at_ms != 0)) {
            (UserStatus::Online, _) | (_, None) => status.status.label().to_owned(),
            (_, Some(until)) => format!("{} {until}", status.status.label()),
        };
        if let Some(button) = self.status_button.borrow().as_ref() {
            button.set_tooltip_text(Some(&tooltip));
        }
    }

    /// A custom status's emoji: the picture of a server's, or the character.
    /// Nothing if it has none.
    fn status_emoji(&self, custom: &CustomStatus) -> Option<gtk::Widget> {
        match (custom.emoji_id, custom.emoji_name.as_str()) {
            (Some(id), _) => {
                let picture = gtk::Picture::builder()
                    .width_request(EMOJI_SIZE)
                    .height_request(EMOJI_SIZE)
                    .content_fit(gtk::ContentFit::Contain)
                    .valign(gtk::Align::Center)
                    .margin_end(4)
                    .build();
                let size = discord::cdn::emoji_size(EMOJI_SIZE as u32, self.images.scale());
                self.images.set(&picture, &discord::cdn::emoji_url(id.0, false, size));
                Some(picture.upcast())
            }
            (None, "") => None,
            (None, name) => Some(gtk::Label::builder().label(name).margin_end(4).build().upcast()),
        }
    }

    /// What we show as now: the status setting, which idle turns to Idle
    /// when it is Online, and whether we are away (`SelfPresenceStore`,
    /// 894768).
    fn local_presence(&self) -> Local {
        let status = self.account.state.borrow().settings.status.status;
        let idle = self.idle.borrow();
        Local::of(status, idle.idle_since(), idle.away())
    }

    /// Tell Discord what we show as, if that changed (`B.update()` in
    /// 225123).
    pub(super) fn report_presence(self: &Rc<Self>) {
        let local = self.local_presence();
        let send = self.account.state.borrow_mut().presence.changed(local, now_ms() as u64);
        if let Some(send) = send {
            self.send_presence(send);
        }
    }

    /// A session started: tell Discord what it doesn't know, which is
    /// everything but an Online account that is active and present
    /// (`handleConnectionOpen`, 225123).
    fn open_presence(self: &Rc<Self>) {
        let local = self.local_presence();
        let send = self.account.state.borrow_mut().presence.opened(local, now_ms() as u64);
        if let Some(send) = send {
            self.send_presence(send);
        }
    }

    /// After RESUMED the client says again what it last said.
    pub(super) fn resend_presence(self: &Rc<Self>) {
        let send = self.account.state.borrow_mut().presence.resumed(now_ms() as u64);
        self.send_presence(send);
    }

    fn send_presence(self: &Rc<Self>, send: Send) {
        // A newer decision replaces one waiting for its turn.
        let generation = {
            let mut state = self.account.state.borrow_mut();
            state.presence_timer += 1;
            state.presence_timer
        };
        match send {
            Send::Now(local) => {
                if let Some(gateway) = &self.account.state.borrow().gateway {
                    gateway.update_presence(local);
                }
            }
            Send::After(wait, local) => {
                let this = Rc::downgrade(self);
                let epoch = self.account.epoch.get();
                glib::timeout_add_local_once(wait, move || {
                    let Some(this) = this.upgrade().filter(|this| this.account.epoch.get() == epoch) else { return };
                    let send = {
                        let mut state = this.account.state.borrow_mut();
                        (state.presence_timer == generation).then(|| state.presence.admit(local, now_ms() as u64))
                    };
                    if let Some(send) = send {
                        this.send_presence(send);
                    }
                });
            }
        }
    }

    /// End what has run out, and wait for what will (`manageExpiringStatus`
    /// and `manageExpiringCustomStatus`, 340137).
    pub(super) fn manage_expiries(self: &Rc<Self>) {
        let (expiries, generation) = {
            let mut state = self.account.state.borrow_mut();
            state.expiry_timer += 1;
            (state.settings.status.expiries(now_ms() as u64), state.expiry_timer)
        };
        let later = |wait: Duration, end: fn(&Rc<App>)| {
            let this = Rc::downgrade(self);
            let epoch = self.account.epoch.get();
            glib::timeout_add_local_once(wait, move || {
                if let Some(this) = this
                    .upgrade()
                    .filter(|t| t.account.epoch.get() == epoch && t.account.state.borrow().expiry_timer == generation)
                {
                    end(&this);
                }
            });
        };
        if let Some(wait) = expiries.custom {
            later(wait, |app| app.change_setting(Setting::CustomStatus(None)));
        }
        let focus = {
            let state = self.account.state.borrow();
            let (now, ends) = (now_ms() as u64, state.settings.notifications.focus_expires_at_ms);
            // A time with quiet mode off is nothing.
            (state.settings.notifications.quiet_mode && ends != 0)
                .then(|| Duration::from_millis(ends.saturating_sub(now)))
        };
        if let Some(wait) = focus {
            later(wait, |app| app.set_focus_mode(false, None));
        }
        if let Some(wait) = expiries.status {
            later(wait, |app| {
                let now_ms = now_ms() as u64;
                app.change_setting(Setting::Status { status: UserStatus::Online, expires_at_ms: 0, now_ms });
            });
        }
    }

    /// READY: show the status, say it, and soon look for what has ended.
    pub(super) fn status_ready(self: &Rc<Self>) {
        self.refresh_status();
        self.open_presence();
        let this = Rc::downgrade(self);
        let epoch = self.account.epoch.get();
        glib::timeout_add_local_once(AFTER_READY, move || {
            if let Some(this) = this.upgrade().filter(|this| this.account.epoch.get() == epoch) {
                this.manage_expiries();
            }
        });
    }

    /// The custom status dialog (431273).
    fn show_custom_status(self: &Rc<Self>) {
        let current = self.account.state.borrow().settings.status.custom.clone();
        let dialog =
            adw::Dialog::builder().title("Set Your Status").content_width(420).follows_content_size(true).build();

        let emoji: Rc<RefCell<Picked>> = Rc::new(RefCell::new(
            current
                .as_ref()
                .filter(|c| c.emoji_id.is_some() || !c.emoji_name.is_empty())
                .map(|c| (c.emoji_id, c.emoji_name.clone())),
        ));
        let emoji_button = gtk::Button::builder().css_classes(["flat"]).tooltip_text("Status emoji").build();
        let show_emoji = {
            let (button, emoji, this) = (emoji_button.clone(), emoji.clone(), Rc::downgrade(self));
            Rc::new(move || {
                let chosen = emoji.borrow().clone();
                let (Some(this), Some((id, name))) = (this.upgrade(), chosen) else {
                    return button.set_icon_name("face-smile-symbolic");
                };
                let custom = CustomStatus { emoji_id: id, emoji_name: name, ..CustomStatus::default() };
                match this.status_emoji(&custom) {
                    Some(widget) => button.set_child(Some(&widget)),
                    None => button.set_icon_name("face-smile-symbolic"),
                }
            })
        };
        show_emoji();
        emoji_button.connect_clicked({
            let (this, emoji, show_emoji) = (Rc::downgrade(self), emoji.clone(), show_emoji.clone());
            move |button| {
                let Some(this) = this.upgrade() else { return };
                let (emoji, show_emoji) = (emoji.clone(), show_emoji.clone());
                this.open_library_picker(
                    this.status_emoji_library(),
                    button,
                    discord::emoji::Intention::Chat,
                    move |_, item| {
                        *emoji.borrow_mut() = Some(match item {
                            discord::emoji::Item::Unicode(e) => (None, e.glyph.to_owned()),
                            discord::emoji::Item::Custom(named) => (Some(named.emoji.id), named.emoji.name.clone()),
                        });
                        show_emoji();
                    },
                );
            }
        });

        let text = gtk::Entry::builder()
            .placeholder_text("What's on your mind?")
            .max_length(CUSTOM_STATUS_LENGTH)
            .hexpand(true)
            .activates_default(true)
            .text(current.as_ref().map_or("", |c| c.text.as_str()))
            .build();
        let entry = gtk::Box::builder().spacing(6).build();
        entry.append(&emoji_button);
        entry.append(&text);

        let clear_after = adw::ComboRow::builder()
            .title("Clear after")
            .model(&gtk::StringList::new(&ClearAfter::ALL.map(ClearAfter::label)))
            .build();
        let now = now_ms() as u64;
        let same_day = |at: u64| {
            let (at, today) = (glib::DateTime::from_unix_local((at / 1000) as i64), glib::DateTime::now_local());
            matches!((at, today), (Ok(a), Ok(t)) if a.ymd() == t.ymd())
        };
        let initial = ClearAfter::initial(
            current.as_ref().map(|c| c.expires_at_ms),
            now,
            current.as_ref().is_some_and(|c| same_day(c.expires_at_ms)),
        );
        clear_after.set_selected(ClearAfter::ALL.iter().position(|c| *c == initial).unwrap_or(0) as u32);
        let timing = adw::PreferencesGroup::new();
        timing.add(&clear_after);

        let save = gtk::Button::builder().label("_Save").use_underline(true).css_classes(["suggested-action"]).build();
        let saved = Rc::new(Cell::new(false));
        save.connect_clicked({
            let (this, dialog, text, emoji, clear_after, saved) =
                (Rc::downgrade(self), dialog.clone(), text.clone(), emoji.clone(), clear_after.clone(), saved.clone());
            move |_| {
                let Some(this) = this.upgrade() else { return };
                if saved.replace(true) {
                    return;
                }
                let choice = ClearAfter::ALL.get(clear_after.selected() as usize).copied().unwrap_or(ClearAfter::Today);
                let now = now_ms() as u64;
                // A day from now is the same time tomorrow, which is not
                // 24 hours across a change of clocks.
                let today_ms = glib::DateTime::now_local()
                    .ok()
                    .and_then(|t| t.add_days(1).ok().map(|tomorrow| tomorrow.difference(&t).as_milliseconds() as u64))
                    .unwrap_or(86_400_000);
                let (emoji_id, emoji_name) = emoji.borrow().clone().unwrap_or_default();
                let custom = CustomStatus {
                    text: text.text().trim().to_owned(),
                    emoji_id,
                    emoji_name,
                    expires_at_ms: choice.expires_at_ms(now, today_ms),
                    created_at_ms: now,
                };
                // Nothing to show clears the status instead of saving it.
                this.change_setting(Setting::CustomStatus(Some(custom)));
                dialog.close();
            }
        });
        dialog.set_default_widget(Some(&save));

        let form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        form.append(&entry);
        form.append(&timing);
        form.append(&save);
        dialog.set_child(Some(&form));
        dialog.present(Some(&self.window));
        text.grab_focus();
    }
}
