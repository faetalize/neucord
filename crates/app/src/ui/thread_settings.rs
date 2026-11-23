//! Edit Thread and Edit Post: the form that changes a thread's name, its
//! slowmode, when it archives itself, who may invite to a private one, and
//! whether it hides its media. The client opens its channel settings on the
//! thread (200273, 603280); here it is a dialog. What it saves is in
//! [`discord::threads::settings_body`], who may open it in
//! [`discord::permissions::can_edit_thread`].

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use discord::permissions;
use discord::thread_creation::{NAME_MAX, cut, single_line};
use discord::threads::{self, ARCHIVE_AFTER, SLOWMODES, Settings};
use discord::{ChannelKind, Snowflake};
use gtk::glib;
use serde_json::Value;

use super::App;
use super::menus::{Entry, Item, Kind};
use super::state::now_unix;
use super::threads::ThreadTarget;

macro_rules! edit_item {
    ($name:ident, $id:literal, $label:literal, $post:literal) => {
        /// Edit Thread / Edit Post (200273).
        pub(super) const $name: Entry<ThreadTarget> = Entry::Item(Item {
            name: $id,
            label: $label,
            shows: |app, target| {
                app.thread_can_edit(target.0) && app.account.state.borrow().is_forum_post(target.0) == $post
            },
            kind: Kind::Action(|app, target| app.open_thread_settings(target.0)),
        });
    };
}
edit_item!(EDIT_THREAD, "edit-thread", "_Edit Thread", false);
edit_item!(EDIT_POST, "edit-post", "_Edit Post", true);

/// The form's widgets.
struct Form {
    dialog: adw::Dialog,
    name: gtk::Entry,
    slowmode: adw::ComboRow,
    archive: adw::ComboRow,
    invitable: Option<adw::SwitchRow>,
    spoiler: adw::SwitchRow,
    name_error: gtk::Label,
    error: gtk::Label,
    save: gtk::Button,
    /// The request is out: nothing changes until it answers.
    submitting: Cell<bool>,
    /// The values the slowmode and archive choices stand for.
    slowmodes: Vec<u32>,
    archives: Vec<u32>,
}

impl Form {
    /// What the form says now.
    fn settings(&self) -> Settings {
        Settings {
            name: self.name.text().to_string(),
            slowmode: self.slowmodes.get(self.slowmode.selected() as usize).copied().unwrap_or(0),
            archive_after: self.archives.get(self.archive.selected() as usize).copied().unwrap_or(0),
            invitable: self.invitable.as_ref().map(|row| row.is_active()),
            spoiler: self.spoiler.is_active(),
        }
    }

    fn set_busy(&self, busy: bool) {
        self.submitting.set(busy);
        self.save.set_sensitive(!busy);
    }
}

/// The choices of a drop-down: the form's values, with the thread's own
/// first when it isn't one of them (`nO` in 603280 does the same).
fn choices(values: &[u32], current: u32) -> Vec<u32> {
    let mut all = values.to_vec();
    if !all.contains(&current) {
        all.insert(0, current);
    }
    all
}

fn combo(title: &str, subtitle: &str, labels: &[String], current: usize, sensitive: bool) -> adw::ComboRow {
    let strings: Vec<&str> = labels.iter().map(String::as_str).collect();
    adw::ComboRow::builder()
        .title(title)
        .subtitle(subtitle)
        .model(&gtk::StringList::new(&strings))
        .selected(current as u32)
        .sensitive(sensitive)
        .build()
}

impl App {
    /// Whether Edit Thread shows on `thread` now.
    pub(super) fn thread_can_edit(&self, thread: Snowflake) -> bool {
        let state = self.account.state.borrow();
        match (state.find_channel(thread), &state.account) {
            (Some((Some(guild), channel)), Some(me)) => permissions::can_edit_thread(guild, channel, me, now_unix()),
            _ => false,
        }
    }

    /// Open the form on `thread`.
    pub(super) fn open_thread_settings(self: &Rc<Self>, thread: Snowflake) {
        let (channel, moderator, post) = {
            let state = self.account.state.borrow();
            let (Some((Some(guild), channel)), Some(me)) = (state.find_channel(thread), &state.account) else {
                return;
            };
            let moderator = permissions::thread_moderator(guild, channel, me, now_unix());
            (channel.clone(), moderator, state.is_forum_post(thread))
        };
        let settings = Settings::of(&channel);
        let slowmodes = choices(&SLOWMODES, settings.slowmode);
        let archives = choices(&ARCHIVE_AFTER, settings.archive_after);
        let at = |values: &[u32], value: u32| values.iter().position(|&v| v == value).unwrap_or(0);

        let name = gtk::Entry::builder().text(&settings.name).build();
        // The limit is in UTF-16 units, and a name is a single line.
        name.connect_changed(|entry| {
            let text = entry.text();
            let kept = single_line(cut(&text, NAME_MAX), false);
            if kept != text {
                entry.set_text(&kept);
                entry.set_position(-1);
            }
        });
        let error = || gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["error"]).visible(false).build();
        let (name_error, general_error) = (error(), error());
        name.connect_changed({
            let name_error = name_error.downgrade();
            move |_| {
                if let Some(error) = name_error.upgrade() {
                    error.set_visible(false);
                }
            }
        });
        let slowmode_labels: Vec<String> = slowmodes.iter().map(|&s| threads::duration_label(s, "Off")).collect();
        let archive_labels: Vec<String> = archives.iter().map(|&m| threads::archive_label(m)).collect();
        let slowmode = combo(
            "Slowmode",
            "Members will be restricted to sending one message per this interval, unless they have the Bypass \
             Slowmode permission.",
            &slowmode_labels,
            at(&slowmodes, settings.slowmode),
            // Only those with Manage Threads on the thread (`isThreadModerator`).
            moderator,
        );
        let archive = combo(
            "Hide After Inactivity",
            if post {
                "Posts will not show in the channel list after being inactive for the specified duration."
            } else {
                "Threads will not show in the channel list after being inactive for the specified duration."
            },
            &archive_labels,
            at(&archives, settings.archive_after),
            true,
        );
        let invitable = (channel.kind == ChannelKind::PrivateThread && settings.invitable.is_some()).then(|| {
            adw::SwitchRow::builder()
                .title("Anyone can invite")
                .subtitle("When disabled, only moderators and the thread creator can add new members to this thread.")
                .active(settings.invitable.unwrap_or_default())
                .build()
        });
        let spoiler = adw::SwitchRow::builder()
            .title("Spoiler Channel")
            .subtitle(
                "Mark this channel as containing spoilers, so that plot twists, heavy topics, or other sensitive \
                 discussions are hidden until users choose to view.",
            )
            .active(settings.spoiler)
            .build();

        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        list.append(&slowmode);
        list.append(&archive);
        if let Some(row) = &invitable {
            list.append(row);
        }
        list.append(&spoiler);
        let heading = gtk::Label::builder()
            .label(if post { "Post Title" } else { "Thread Name" })
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(12);
        body.set_margin_end(12);
        for widget in [heading.upcast_ref::<gtk::Widget>(), name.upcast_ref(), name_error.upcast_ref()] {
            body.append(widget);
        }
        let spacer = gtk::Box::builder().height_request(6).build();
        for widget in [spacer.upcast_ref::<gtk::Widget>(), list.upcast_ref(), general_error.upcast_ref()] {
            body.append(widget);
        }

        let save = gtk::Button::builder().label("Save Changes").css_classes(["suggested-action"]).build();
        let header = adw::HeaderBar::new();
        header.pack_end(&save);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body));
        let dialog = adw::Dialog::builder()
            .title(if post { "Post Settings" } else { "Thread Settings" })
            .content_width(480)
            .child(&toolbar)
            .build();
        let form = Rc::new(Form {
            dialog,
            name,
            slowmode,
            archive,
            invitable,
            spoiler,
            name_error,
            error: general_error,
            save,
            submitting: Cell::new(false),
            slowmodes,
            archives,
        });
        form.save.connect_clicked({
            let (this, form) = (Rc::downgrade(self), form.clone());
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.save_thread_settings_form(&form, thread);
                }
            }
        });
        form.dialog.present(Some(&self.window));
    }

    /// Check the form and send it.
    fn save_thread_settings_form(self: &Rc<Self>, form: &Rc<Form>, thread: Snowflake) {
        if form.submitting.get() {
            return;
        }
        form.error.set_visible(false);
        let (rest, channel, reopens, post) = {
            let state = self.account.state.borrow();
            let (Some((guild, channel)), Some(me)) = (state.find_channel(thread), &state.account) else { return };
            let reopens = permissions::unarchives_first(guild, channel, me, now_unix());
            (state.rest.clone(), channel.clone(), reopens, state.is_forum_post(thread))
        };
        let Some(rest) = rest else { return };
        let body = match threads::settings_body(&channel, &form.settings()) {
            Ok(body) => body,
            Err(invalid) => {
                form.name_error.set_label(invalid.words());
                return form.name_error.set_visible(true);
            }
        };
        form.set_busy(true);
        let (this, form) = (self.clone(), form.clone());
        glib::spawn_future_local(async move {
            // As `saveChannel` does: a thread that is archived is reopened first.
            if reopens && let Err(e) = rest.unarchive_thread(thread).await {
                eprintln!("thread {thread}: {}", e.for_log());
                let (title, text) = threads::reopen_alert(&e, post);
                let alert = adw::AlertDialog::new(Some(title), Some(text));
                alert.add_response("ok", "_OK");
                alert.present(Some(&form.dialog));
                return form.set_busy(false);
            }
            match rest.edit_thread(thread, body).await {
                Ok(()) => {
                    // The thread's new state arrives as an event.
                    form.dialog.close();
                    this.composer.grab_focus();
                }
                Err(e) => {
                    eprintln!("thread {thread}: {}", e.for_log());
                    form.set_busy(false);
                    let (field, message) = refusal(&e);
                    let target = if field { &form.name_error } else { &form.error };
                    target.set_label(&message);
                    target.set_visible(true);
                }
            }
        });
    }
}

/// What Discord said went wrong, and whether it was the name: the field's own
/// words if it gave them, else its message, else the client's generic line.
fn refusal(error: &discord::Error) -> (bool, String) {
    let generic = "An error occurred, please try again.".to_owned();
    let discord::Error::Http { body, .. } = error else { return (false, generic) };
    let body: Value = serde_json::from_str(body).unwrap_or_default();
    if let Some(words) = body["errors"]["name"]["_errors"][0]["message"].as_str() {
        return (true, words.to_owned());
    }
    (false, body["message"].as_str().map_or(generic, str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_threads_own_value_is_offered_first_when_it_is_odd() {
        assert_eq!(choices(&[60, 1440], 1440), [60, 1440]);
        assert_eq!(choices(&[60, 1440], 2880), [2880, 60, 1440]);
    }

    #[test]
    fn a_refusal_names_the_field_it_is_about() {
        let http = |body: &str| discord::Error::Http { status: 400, body: body.into() };
        let named = http(
            r#"{"code":50035,"errors":{"name":{"_errors":[{"message":"Must be 100 or fewer."}]}},"message":"Invalid Form Body"}"#,
        );
        assert_eq!(refusal(&named), (true, "Must be 100 or fewer.".to_owned()));
        assert_eq!(
            refusal(&http(r#"{"message":"Missing Permissions","code":50013}"#)),
            (false, "Missing Permissions".to_owned())
        );
        assert_eq!(refusal(&http("nonsense")), (false, "An error occurred, please try again.".to_owned()));
    }
}
