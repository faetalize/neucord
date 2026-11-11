//! Our own status and custom status: shown in the account panel, set from
//! its menu as the official client's account popout does (998034 and the
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
use discord::presence::Send;
use discord::settings::{ClearAfter, CustomStatus, StatusSettings, UserStatus};
use discord::{Setting, Snowflake};
use gtk::{gio, glib};

use super::App;
use super::state::now_ms;

/// How long the status picker offers a status for, `None` for ever (`it`,
/// 998034).
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

/// The status picker's menu: Online, then each other status with the times
/// it can last, then the custom status.
fn menu(status: &StatusSettings) -> gio::Menu {
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

    let custom = gio::Menu::new();
    let label = if status.custom.is_some() { "Edit Custom _Status…" } else { "Set a Custom _Status…" };
    custom.append(Some(label), Some("win.custom-status"));
    if status.custom.is_some() {
        custom.append(Some("C_lear Custom Status"), Some("win.clear-custom-status"));
    }
    menu.append_section(None, &custom);
    menu
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
    /// The account panel's status: a dot, and under the name the custom
    /// status, else the status. Opens the status menu.
    pub(super) fn build_status_button(self: &Rc<Self>) -> gtk::MenuButton {
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        text.append(&self.me_label);
        text.append(&self.me_status);
        let content = gtk::Box::builder().spacing(10).build();
        content.append(&self.me_dot);
        content.append(&text);
        let button = gtk::MenuButton::builder()
            .child(&content)
            .hexpand(true)
            .direction(gtk::ArrowType::Up)
            .css_classes(["flat"])
            .build();
        button.set_menu_model(Some(&menu(&StatusSettings::default())));
        *self.status_button.borrow_mut() = Some(button.clone());
        button
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
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                if let Some(this) = this.upgrade() {
                    this.change_setting(Setting::CustomStatus(None));
                }
            })
            .build();
        vec![set, custom, clear]
    }

    /// The status picker (827827): the status, and when it goes back to
    /// Online if it was given a time.
    fn set_status(self: &Rc<Self>, status: UserStatus, duration_ms: Option<u64>) {
        let now_ms = now_ms() as u64;
        let expires_at_ms = duration_ms.map_or(0, |d| now_ms + d);
        self.change_setting(Setting::Status { status, expires_at_ms, now_ms });
    }

    /// Show the status in the account panel and its menu.
    pub(super) fn refresh_status(&self) {
        let status = self.state.borrow().settings.status.clone();
        let class = match status.status {
            UserStatus::Online => "online",
            UserStatus::Idle => "idle",
            UserStatus::DoNotDisturb => "dnd",
            UserStatus::Invisible => "offline",
        };
        self.me_dot.set_css_classes(&["presence", class]);

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
            button.set_menu_model(Some(&menu(&status)));
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
                let size = ((EMOJI_SIZE * self.images.scale()) as u32).next_power_of_two().clamp(16, 4096) as u16;
                self.images.set(&picture, &discord::cdn::emoji_url(id.0, false, size), false);
                Some(picture.upcast())
            }
            (None, "") => None,
            (None, name) => Some(gtk::Label::builder().label(name).margin_end(4).build().upcast()),
        }
    }

    /// Tell Discord what we show as, if that changed (`B.update()` in
    /// 225123). Also what the first READY does: an account that is not
    /// Online is told so.
    pub(super) fn report_presence(self: &Rc<Self>) {
        let send = {
            let mut state = self.state.borrow_mut();
            let status = state.settings.status.status;
            state.presence.changed(status, now_ms() as u64)
        };
        if let Some(send) = send {
            self.send_presence(send);
        }
    }

    /// After RESUMED the client says again what it last said.
    pub(super) fn resend_presence(self: &Rc<Self>) {
        let send = self.state.borrow_mut().presence.resumed(now_ms() as u64);
        self.send_presence(send);
    }

    fn send_presence(self: &Rc<Self>, send: Send) {
        // A newer decision replaces one waiting for its turn.
        let generation = {
            let mut state = self.state.borrow_mut();
            state.presence_timer += 1;
            state.presence_timer
        };
        match send {
            Send::Now(status) => {
                if let Some(gateway) = &self.state.borrow().gateway {
                    gateway.update_presence(status);
                }
            }
            Send::After(wait, status) => {
                let this = Rc::downgrade(self);
                glib::timeout_add_local_once(wait, move || {
                    let Some(this) = this.upgrade() else { return };
                    let send = {
                        let mut state = this.state.borrow_mut();
                        (state.presence_timer == generation).then(|| state.presence.admit(status, now_ms() as u64))
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
            let mut state = self.state.borrow_mut();
            state.expiry_timer += 1;
            (state.settings.status.expiries(now_ms() as u64), state.expiry_timer)
        };
        let later = |wait: Duration, end: fn(&Rc<App>)| {
            let this = Rc::downgrade(self);
            glib::timeout_add_local_once(wait, move || {
                if let Some(this) = this.upgrade().filter(|t| t.state.borrow().expiry_timer == generation) {
                    end(&this);
                }
            });
        };
        if let Some(wait) = expiries.custom {
            later(wait, |app| app.change_setting(Setting::CustomStatus(None)));
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
        self.report_presence();
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(AFTER_READY, move || {
            if let Some(this) = this.upgrade() {
                this.manage_expiries();
            }
        });
    }

    /// The custom status dialog (431273).
    fn show_custom_status(self: &Rc<Self>) {
        let current = self.state.borrow().settings.status.custom.clone();
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
