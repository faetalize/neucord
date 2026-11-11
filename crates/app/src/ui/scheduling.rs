//! Scheduled messages: choosing when the next message goes out (the attach
//! menu's Schedule Message and the time picker), the bar over the message
//! box that says so, and the list of what is waiting, where each can be
//! sent now, edited, moved or cancelled. What the times allow, and what the
//! requests say, is `discord::scheduled`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use discord::edits::{self, Edit};
use discord::scheduled::{self, Scheduled};
use discord::{Rest, Snowflake};
use gtk::glib;

use super::App;
use super::friends::confirm;
use super::menus::{Choice, Choices, Entry, Item, Kind, Style};
use super::state::now_unix;
use super::uploads::Stopped;

/// Schedule Message, in the attach menu: the presets, then a time of your own.
pub(super) const SCHEDULE_MESSAGE: Entry<()> = Entry::Item(Item {
    name: "schedule-message",
    label: "_Schedule Message",
    shows: |app, _| app.schedule_channel().is_some(),
    kind: Kind::Submenu(&[
        &[Entry::Choices(Choices {
            name: "schedule-preset",
            choices: |_, _| {
                scheduled::presets(now_unix(), &glib::TimeZone::local())
                    .into_iter()
                    .map(|p| Choice { value: p.at.to_string(), label: p.label.to_owned(), ticked: false })
                    .collect()
            },
            style: Style::Plain,
            choose: |app, _, value, _| {
                if let (Some(channel), Ok(at)) = (app.schedule_channel(), value.parse()) {
                    app.set_schedule(channel, at);
                }
            },
        })],
        &[Entry::Item(Item {
            name: "schedule-custom",
            label: "_Custom…",
            shows: |_, _| true,
            kind: Kind::Action(|app, _| app.schedule_custom_time()),
        })],
    ]),
});

/// `at` for a time the picker starts from when nothing is chosen yet.
pub(super) fn default_start() -> i64 {
    scheduled::default_time(now_unix(), &glib::TimeZone::local())
}

/// What the bar and the poll editor's button say of a chosen time.
pub(super) fn scheduling_for(at: i64) -> String {
    format!("Scheduling for {} at {}", day(at), clock(at))
}

/// What the picker's window is called, and its confirming button.
#[derive(Clone, Copy)]
pub(super) struct Picker {
    title: &'static str,
    confirm: &'static str,
}

impl Picker {
    /// For a message about to be written.
    pub(super) const NEW: Picker = Picker { title: "Schedule Message", confirm: "Set Schedule" };
    /// For one already waiting.
    const MOVE: Picker = Picker { title: "Update Scheduled Message", confirm: "Update" };
}

/// What the app keeps for scheduling.
pub(super) struct Scheduling {
    /// When the next message written in each channel goes out, in unix
    /// seconds, until it is scheduled or the time is cleared.
    drafts: RefCell<HashMap<Snowflake, i64>>,
    /// "Scheduling message for …", above the message box.
    bar: gtk::Box,
    when: gtk::Label,
}

impl Scheduling {
    pub(super) fn new() -> Self {
        Self {
            drafts: RefCell::default(),
            // Styled as the reply bar, which it stacks with.
            bar: gtk::Box::builder().spacing(8).visible(false).css_classes(["reply-bar"]).build(),
            when: gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build(),
        }
    }

    pub(super) fn clear(&self) {
        self.drafts.borrow_mut().clear();
    }
}

fn local(at: i64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_utc(at).and_then(|t| t.to_local()).ok()
}

/// "November 12", for the bar.
fn day(at: i64) -> String {
    local(at).and_then(|t| t.format("%B %-e").ok()).map(|t| t.trim().to_owned()).unwrap_or_default()
}

/// "9:00 AM".
fn clock(at: i64) -> String {
    local(at).and_then(|t| t.format("%-I:%M %p").ok()).map(String::from).unwrap_or_default()
}

/// "November 12, 2026 at 9:00 AM".
fn long(at: i64) -> String {
    let date = local(at).and_then(|t| t.format("%B %-e, %Y").ok()).map(|t| t.trim().to_owned()).unwrap_or_default();
    format!("{date} at {}", clock(at))
}

impl App {
    /// The open channel, if the client would offer Schedule Message in it
    /// now: the message box shows, no edit has it, and we may post there.
    pub(super) fn schedule_channel(&self) -> Option<Snowflake> {
        let channel = self.account.state.borrow().channel?;
        let allowed = self.composer_box.is_visible()
            && self.editing().is_none()
            && !self.has_voice_draft(channel)
            && self.account.state.borrow().can_send(channel);
        allowed.then_some(channel)
    }

    /// When the message being written in `channel` goes out, if it is scheduled.
    pub(super) fn scheduled_time(&self, channel: Snowflake) -> Option<i64> {
        self.scheduling.drafts.borrow().get(&channel).copied()
    }

    pub(super) fn set_schedule(self: &Rc<Self>, channel: Snowflake, at: i64) {
        self.scheduling.drafts.borrow_mut().insert(channel, at);
        self.refresh_schedule_bar();
        self.composer.grab_focus();
    }

    pub(super) fn clear_schedule(self: &Rc<Self>, channel: Snowflake) {
        self.scheduling.drafts.borrow_mut().remove(&channel);
        self.refresh_schedule_bar();
    }

    /// The bar over the message box: the open channel's time, if it has one.
    pub(super) fn refresh_schedule_bar(&self) {
        let shown =
            self.account.state.borrow().channel.filter(|_| self.composer_box.is_visible() && self.editing().is_none());
        let at = shown.and_then(|channel| self.scheduled_time(channel));
        self.scheduling.bar.set_visible(at.is_some());
        if let Some(at) = at {
            self.scheduling.when.set_label(&format!("Scheduling message for {} at {}", day(at), clock(at)));
        }
    }

    pub(super) fn build_schedule_bar(self: &Rc<Self>, area: &gtk::Box) {
        let bar = &self.scheduling.bar;
        let edit = gtk::Button::builder()
            .child(&self.scheduling.when)
            .tooltip_text("Edit schedule time")
            .hexpand(true)
            .css_classes(["flat"])
            .build();
        edit.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                let Some(channel) = this.account.state.borrow().channel else { return };
                this.schedule_custom_time_in(channel);
            }
        });
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close")
            .css_classes(["flat"])
            .build();
        close.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                let Some(channel) = this.account.state.borrow().channel else { return };
                this.clear_schedule(channel);
                this.composer.grab_focus();
            }
        });
        bar.append(&edit);
        bar.append(&close);
        area.append(bar);
    }

    /// Custom…, from the attach menu.
    fn schedule_custom_time(self: &Rc<Self>) {
        if let Some(channel) = self.schedule_channel() {
            self.schedule_custom_time_in(channel);
        }
    }

    /// Ask for the time of `channel`'s next message, starting from the one it
    /// has, or the next full hour.
    fn schedule_custom_time_in(self: &Rc<Self>, channel: Snowflake) {
        let start = self.scheduled_time(channel).unwrap_or_else(default_start);
        let this = Rc::downgrade(self);
        self.pick_time(&self.window, Picker::NEW, start, None, None, move |at| {
            if let Some(this) = this.upgrade() {
                this.set_schedule(channel, at);
            }
        });
    }

    /// Ask when to send: a day and a time, which have to be a time a message
    /// may go out at, as `check` says, with `waiting` the scheduled message
    /// being moved. `clear`, if there is one, takes the place of Cancel and
    /// is for taking a chosen time back.
    pub(super) fn pick_time(
        self: &Rc<Self>,
        parent: &impl IsA<gtk::Widget>,
        picker: Picker,
        start: i64,
        waiting: Option<Snowflake>,
        clear: Option<Box<dyn Fn()>>,
        set: impl Fn(i64) + 'static,
    ) {
        let dialog = adw::Dialog::builder().title(picker.title).content_width(360).follows_content_size(true).build();
        let tz = glib::TimeZone::local();
        let start = glib::DateTime::from_unix_utc(start).and_then(|t| t.to_timezone(&tz));
        let calendar = gtk::Calendar::new();
        let hour = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
        let minute = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
        for spin in [&hour, &minute] {
            spin.set_wrap(true);
            spin.set_numeric(true);
            spin.connect_output(|spin| {
                spin.set_text(&format!("{:02}", spin.value() as i32));
                glib::Propagation::Stop
            });
        }
        if let Ok(start) = &start {
            calendar.set_date(start);
            hour.set_value(f64::from(start.hour()));
            minute.set_value(f64::from(start.minute()));
        }
        let error = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["error"]).build();
        let confirm = gtk::Button::builder().label(picker.confirm).css_classes(["suggested-action"]).build();

        let chosen = {
            let (calendar, hour, minute, tz) = (calendar.clone(), hour.clone(), minute.clone(), tz.clone());
            move || {
                let date = calendar.date();
                glib::DateTime::new(
                    &tz,
                    date.year(),
                    date.month(),
                    date.day_of_month(),
                    hour.value_as_int(),
                    minute.value_as_int(),
                    0.0,
                )
                .map(|t| t.to_unix())
                .ok()
            }
        };
        let validate = {
            let (chosen, error, confirm) = (chosen.clone(), error.clone(), confirm.clone());
            Rc::new(move || {
                let refusal = chosen().map(|at| scheduled::check(at, now_unix(), waiting));
                match refusal {
                    Some(Err(refusal)) => error.set_label(refusal.text()),
                    _ => error.set_label(""),
                }
                confirm.set_sensitive(matches!(refusal, Some(Ok(()))));
            })
        };
        validate();
        calendar.connect_day_selected({
            let validate = validate.clone();
            move |_| validate()
        });
        for spin in [&hour, &minute] {
            let validate = validate.clone();
            spin.connect_value_changed(move |_| validate());
        }

        let time = gtk::Box::builder().spacing(6).halign(gtk::Align::Center).build();
        let heading = gtk::Label::builder().label("Time").css_classes(["heading"]).build();
        time.append(&heading);
        time.append(&hour);
        time.append(&gtk::Label::new(Some(":")));
        time.append(&minute);
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(18)
            .margin_end(18)
            .build();
        body.append(&calendar);
        body.append(&time);
        body.append(&error);
        let secondary = gtk::Button::with_label(if clear.is_some() { "Clear" } else { "Cancel" });
        let actions = gtk::ActionBar::new();
        actions.pack_start(&secondary);
        actions.pack_end(&confirm);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&body));
        toolbar.add_bottom_bar(&actions);
        dialog.set_child(Some(&toolbar));
        dialog.set_default_widget(Some(&confirm));

        secondary.connect_clicked({
            let dialog = dialog.downgrade();
            move |_| {
                if let Some(clear) = &clear {
                    clear();
                }
                if let Some(dialog) = dialog.upgrade() {
                    dialog.force_close();
                }
            }
        });
        confirm.connect_clicked({
            let dialog = dialog.downgrade();
            move |_| {
                if let Some(at) = chosen().filter(|&at| scheduled::check(at, now_unix(), waiting).is_ok()) {
                    set(at);
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.force_close();
                    }
                }
            }
        });
        dialog.present(Some(parent));
    }

    /// Tell the user something, briefly.
    pub(super) fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    /// What follows a message handed to Discord to send later.
    pub(super) fn message_scheduled(self: &Rc<Self>, channel: Snowflake, at: i64) {
        self.clear_schedule(channel);
        self.toast_scheduled(at);
    }

    pub(super) fn toast_scheduled(&self, at: i64) {
        self.toast(&format!("Message scheduled for {}", long(at)));
    }

    /// Say why a message wasn't scheduled. One over the limit gets the
    /// client's dialog, which offers the list to take one back from.
    pub(super) fn scheduling_failed(self: &Rc<Self>, stopped: &Stopped) {
        match stopped {
            Stopped::Send(e) if e.code() == Some(scheduled::TOO_MANY) => self.show_limit_reached(&self.window),
            Stopped::Send(e) => self.show_composer_error(&format!("Failed to schedule message. {e}")),
            Stopped::Upload(why) => self.show_composer_error(&format!("Not sent: {why}")),
            Stopped::Canceled => {}
        }
    }

    /// Over `parent`, which may be a dialog of the window.
    pub(super) fn show_limit_reached(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let max = self.account.state.borrow().limits().scheduled_messages().max;
        let dialog = adw::AlertDialog::new(
            Some("Oops! Limit reached."),
            Some(&format!("You've reached the limit of {max} scheduled messages. Cancel one to schedule another.")),
        );
        dialog.add_responses(&[("okay", "_Okay"), ("manage", "_Manage Messages")]);
        dialog.set_close_response("okay");
        let (this, parent) = (self.clone(), parent.clone().upcast::<gtk::Widget>());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&parent)).await == "manage" {
                this.show_scheduled_messages();
            }
        });
    }

    /// The list of what is waiting, fetched as it opens, as the client does
    /// when its inbox's Scheduled tab opens.
    pub(super) fn show_scheduled_messages(self: &Rc<Self>) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let dialog = adw::Dialog::builder().title("Scheduled Messages").content_width(520).content_height(560).build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .css_classes(["boxed-list"])
            .build();
        let page = adw::Clamp::builder()
            .maximum_size(520)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .child(&list)
            .build();
        let scroller = gtk::ScrolledWindow::builder().child(&page).hscrollbar_policy(gtk::PolicyType::Never).build();
        let empty = adw::StatusPage::builder()
            .icon_name("alarm-symbolic")
            .title("Send it at the perfect moment")
            .description(
                "Draft your message then click the plus button and pick Schedule Message to choose when to send. \
                We'll handle the rest.",
            )
            .build();
        let failed = adw::StatusPage::builder().icon_name("dialog-error-symbolic").title("Couldn't load").build();
        let stack = gtk::Stack::new();
        stack.add_named(&adw::Spinner::new(), Some("loading"));
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&failed, Some("failed"));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&stack));
        dialog.set_child(Some(&toolbar));

        let view = Rc::new(View { stack, list, failed, items: RefCell::default(), busy: RefCell::default() });
        dialog.present(Some(&self.window));
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rest.scheduled_messages().await {
                Ok(items) => {
                    *view.items.borrow_mut() = items;
                    view.render(&this, &rest);
                }
                Err(e) => {
                    view.failed.set_description(Some(&e.to_string()));
                    view.stack.set_visible_child_name("failed");
                }
            }
        });
    }
}

/// The open list, and what it is waiting on.
struct View {
    stack: gtk::Stack,
    list: gtk::ListBox,
    failed: adw::StatusPage,
    items: RefCell<Vec<Scheduled>>,
    /// Being sent, cancelled or changed: the client holds their buttons back.
    busy: RefCell<HashSet<Snowflake>>,
}

/// What a row's button asks Discord to do.
#[derive(Clone, Copy)]
enum Act {
    SendNow,
    Cancel,
    /// Its time or its text, which Discord answers with the message as it is now.
    Change,
}

impl View {
    fn render(self: &Rc<Self>, app: &Rc<App>, rest: &Rest) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let items = self.items.borrow().clone();
        for item in &items {
            self.list.append(&self.row(app, rest, item, None));
        }
        self.stack.set_visible_child_name(if items.is_empty() { "empty" } else { "list" });
    }

    /// Draw one row again, as an editor of `text` if there is any.
    fn redraw(self: &Rc<Self>, app: &Rc<App>, rest: &Rest, id: Snowflake, text: Option<&str>) {
        let items = self.items.borrow();
        let Some(position) = items.iter().position(|s| s.id == id) else { return };
        let Some(old) = self.list.row_at_index(position as i32) else { return };
        let row = self.row(app, rest, &items[position], text);
        self.list.remove(&old);
        self.list.insert(&row, position as i32);
    }

    fn row(self: &Rc<Self>, app: &Rc<App>, rest: &Rest, item: &Scheduled, editing: Option<&str>) -> gtk::ListBoxRow {
        let busy = self.busy.borrow().contains(&item.id);
        let id = item.id;
        let channel = app.account.state.borrow().find_channel(item.channel).map(|(guild, c)| match guild {
            Some(g) => format!("{} · {}", c.title(), g.name),
            None => c.title(),
        });
        let button = |icon: &str, tip: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tip)
                .css_classes(["flat"])
                .sensitive(!busy)
                .valign(gtk::Align::Center)
                .build()
        };
        let cancel = button("window-close-symbolic", "Cancel scheduled message");
        cancel.connect_clicked({
            let (this, app, rest) = (self.clone(), app.clone(), rest.clone());
            move |_| {
                this.run(&app, &rest, id, Act::Cancel, move |rest| async move {
                    rest.cancel_scheduled_message(id).await.map(|()| None)
                })
            }
        });
        let actions = gtk::Box::builder().spacing(2).build();
        let head = gtk::Box::builder().spacing(6).build();
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .build();
        head.append(&column);
        head.append(&actions);
        root.append(&head);
        let row = gtk::ListBoxRow::builder().activatable(false).child(&root).build();
        let line = |text: &str, classes: &[&str]| {
            gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(classes.to_vec()).build()
        };

        let Some(channel) = channel else {
            // Where it was going is gone, so all that is left is to take it back.
            column.append(&line("Scheduled message's channel not found.", &["heading"]));
            actions.append(&cancel);
            return row;
        };
        column.append(&line(&channel, &["heading"]));
        match item.state.failure() {
            Some(why) => column.append(&line(why, &["error"])),
            None => column.append(&line(&format!("Message scheduled for {}", long(item.send_at)), &["dim-label"])),
        }

        let send = button("mail-send-symbolic", "Send now");
        send.connect_clicked({
            let (this, app, rest) = (self.clone(), app.clone(), rest.clone());
            move |_| {
                this.run(&app, &rest, id, Act::SendNow, move |rest| async move {
                    rest.send_scheduled_message_now(id).await.map(|()| None)
                })
            }
        });
        let edit = button("document-edit-symbolic", "Edit message");
        edit.connect_clicked({
            let (this, app, rest, text) = (self.clone(), app.clone(), rest.clone(), item.editable_text());
            move |_| this.redraw(&app, &rest, id, Some(&text))
        });
        let time = button("alarm-symbolic", "Edit schedule time");
        time.connect_clicked({
            let (this, app, rest, root, send_at) =
                (self.clone(), app.clone(), rest.clone(), root.clone(), item.send_at);
            move |_| {
                let (this, rest, app) = (this.clone(), rest.clone(), app.clone());
                let picked = app.clone();
                app.pick_time(&root, Picker::MOVE, send_at, Some(id), None, move |at| {
                    let target = scheduled::iso(at);
                    this.run(&picked, &rest, id, Act::Change, move |rest| {
                        let target = target.clone();
                        async move { rest.move_scheduled_message(id, &target).await.map(Some) }
                    });
                });
            }
        });
        for b in [&send, &edit, &time, &cancel] {
            actions.append(b);
        }

        match editing {
            Some(text) => root.append(&self.editor(app, rest, item, text)),
            None if !item.content.is_empty() => root.append(&line(&item.content, &[])),
            None => {}
        }
        if let Some(question) = &item.poll {
            root.append(&line(&format!("Poll: {question}"), &["dim-label"]));
        }
        if item.attachments > 0 {
            let plural = if item.attachments == 1 { "" } else { "s" };
            root.append(&line(&format!("{} attachment{plural}", item.attachments), &["dim-label"]));
        }
        row
    }

    /// Ask Discord for `act` on a message, with its buttons held back until
    /// it answers, then say how it went as the client's toasts do. What
    /// `call` answers with replaces the message in the list; with nothing,
    /// the message is gone from it.
    fn run<F, Fut>(self: &Rc<Self>, app: &Rc<App>, rest: &Rest, id: Snowflake, act: Act, call: F)
    where
        F: FnOnce(Rest) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<Option<Scheduled>, discord::Error>> + 'static,
    {
        self.busy.borrow_mut().insert(id);
        self.redraw(app, rest, id, None);
        let (this, app, rest) = (self.clone(), app.clone(), rest.clone());
        glib::spawn_future_local(async move {
            let result = call(rest.clone()).await;
            this.busy.borrow_mut().remove(&id);
            match result {
                Ok(Some(updated)) => {
                    let mut items = this.items.borrow_mut();
                    if let Some(slot) = items.iter_mut().find(|s| s.id == id) {
                        *slot = updated;
                    }
                    items.sort_by_key(|s| std::cmp::Reverse(s.send_at));
                    drop(items);
                    app.toast("Scheduled message updated");
                    this.render(&app, &rest);
                }
                Ok(None) => {
                    this.items.borrow_mut().retain(|s| s.id != id);
                    app.toast(match act {
                        Act::SendNow => "Message sent.",
                        _ => "Successfully canceled the message.",
                    });
                    this.render(&app, &rest);
                }
                Err(e) => {
                    let what = match act {
                        Act::SendNow => "Failed to send scheduled message.",
                        Act::Cancel => "Failed to cancel scheduled message.",
                        Act::Change => "Failed to update message.",
                    };
                    app.toast(&format!("{what} {e}"));
                    this.redraw(&app, &rest, id, None);
                }
            }
        });
    }

    /// The text of a message in place of it: Enter saves, Shift+Enter breaks
    /// the line, Escape leaves it be.
    fn editor(self: &Rc<Self>, app: &Rc<App>, rest: &Rest, item: &Scheduled, text: &str) -> gtk::Box {
        let view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).accepts_tab(false).build();
        view.buffer().set_text(text);
        let frame =
            gtk::ScrolledWindow::builder().child(&view).max_content_height(200).propagate_natural_height(true).build();
        let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
        let leave = gtk::Button::with_label("Cancel");
        let buttons = gtk::Box::builder().spacing(6).halign(gtk::Align::End).build();
        buttons.append(&leave);
        buttons.append(&save);
        let editor = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        editor.append(&frame);
        editor.append(&buttons);

        let (id, flags) = (item.id, item.flags);
        let bare = item.attachments == 0 && item.poll.is_none();
        let original = item.editable_text();
        let (this, app, rest) = (self.clone(), app.clone(), rest.clone());
        leave.connect_clicked({
            let (this, app, rest) = (this.clone(), app.clone(), rest.clone());
            move |_| this.redraw(&app, &rest, id, None)
        });
        let submit = {
            let view = view.clone();
            Rc::new(move || {
                let buffer = view.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                let edit = edits::decide(&app.account.state.borrow().limits(), &text, &original, bare);
                match edit {
                    Edit::Unchanged => this.redraw(&app, &rest, id, None),
                    Edit::Refuse(too_long) => app.show_too_long(too_long),
                    Edit::Save(text) => {
                        let text = text.to_owned();
                        this.run(&app, &rest, id, Act::Change, move |rest| async move {
                            rest.edit_scheduled_message(id, &text, flags).await.map(Some)
                        });
                    }
                    // Nothing is left of it, which is taking it back.
                    Edit::Delete => {
                        let dialog = confirm(
                            "Cancel Scheduled Message?",
                            "Nothing is left of this message. It will not be sent.",
                            "_Cancel Message",
                            adw::ResponseAppearance::Destructive,
                        );
                        let (this, app, rest) = (this.clone(), app.clone(), rest.clone());
                        glib::spawn_future_local(async move {
                            if dialog.choose_future(Some(&app.window)).await == "confirm" {
                                this.run(&app, &rest, id, Act::Cancel, move |rest| async move {
                                    rest.cancel_scheduled_message(id).await.map(|()| None)
                                });
                            }
                        });
                    }
                }
            })
        };
        save.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let leave = leave.clone();
            move |_, key, _, modifiers| {
                use gtk::gdk::Key;
                match key {
                    Key::Escape => leave.emit_clicked(),
                    Key::Return | Key::KP_Enter if !modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) => submit(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        view.add_controller(keys);
        glib::idle_add_local_once(move || {
            view.grab_focus();
        });
        editor
    }
}
