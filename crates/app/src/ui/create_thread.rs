//! Create Thread: a form for a thread's name, whether it is private, and the
//! message that starts it. The official client opens it in the sidebar, from
//! the attach menu (a new thread in the channel) and from a message's menu
//! (a thread on that message); here it is a dialog. What it asks Discord for
//! is in [`discord::thread_creation`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::chat_bar;
use discord::cooldowns::wait_words;
use discord::thread_creation::{
    self, FROM_ATTACH_MENU, FROM_MESSAGE_MENU, FROM_SHORTCUT, Missing, NAME_MAX, NAME_PLACEHOLDER, Privacy, Refused,
};
use discord::{Message, Snowflake};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::menus::{Entry, Item, Kind};
use super::state::now_ms;
use super::uploads::{Draft, Outgoing, Stopped};
use crate::render;

/// Create Thread in the attach menu, where the client offers it
/// (`canStartThreads` in 316576): in a channel with a message box, or whose
/// notice keeps the button, as the client keeps it for Create Thread alone.
pub(super) const ATTACH_MENU: Entry<()> = Entry::Item(Item {
    name: "create-thread",
    label: "Create T_hread",
    shows: |app, _| app.thread_channel().is_some(),
    kind: Kind::Action(|app, _| {
        if let Some(channel) = app.thread_channel() {
            app.open_thread_form(channel, None, FROM_ATTACH_MENU);
        }
    }),
});

/// Create Thread in a message's menu, on a message that has no thread, which
/// somebody wrote (`n` in 406704).
pub(super) const MESSAGE_MENU: Entry<MessageTarget> = Entry::Item(Item {
    name: "create-thread",
    label: "Create T_hread",
    shows: |app, target| app.account.state.borrow().can_start_thread_from(target.shown()),
    kind: Kind::Action(|app, target| {
        let message = target.shown();
        app.open_thread_form(message.channel_id, Some(message.clone()), FROM_MESSAGE_MENU);
    }),
});

/// View Thread in a message's menu, on one that started a thread we may see
/// (`view-thread` in 812266, where Create Thread isn't offered): it opens the thread.
pub(super) const VIEW_THREAD: Entry<MessageTarget> = Entry::Item(Item {
    name: "view-thread",
    label: "View Thread",
    shows: |app, target| app.has_viewable_thread(target.shown()),
    kind: Kind::Action(|app, target| app.open_thread(target.shown().id)),
});

/// The form's widgets.
pub(super) struct Form {
    /// The channel the thread is started in, whose draft of the first
    /// message's files the form keeps.
    parent: Snowflake,
    dialog: adw::Dialog,
    name: gtk::Entry,
    name_error: gtk::Label,
    private: adw::SwitchRow,
    message: gtk::TextView,
    message_error: gtk::Label,
    /// The files' rows, which `uploads.rs` fills.
    files: gtk::Box,
    /// Where files may be added, if the form takes them.
    attach: Option<gtk::Button>,
    /// The sticker waiting to go with the message, and where one is picked.
    strip: super::stickers::Strip,
    sticker_button: Option<gtk::Button>,
    /// What Create does, which a sticker picked from an empty form does too.
    submit: RefCell<Option<Rc<dyn Fn()>>>,
    /// The wait slowmode asks before another thread, which the client shows
    /// and doesn't hold Create for (420018).
    slowmode: gtk::Label,
    create: gtk::Button,
    /// While the files go up, which the message waits for.
    stop: gtk::Button,
    /// The request is out: nothing changes until it answers.
    submitting: Cell<bool>,
    /// The thread, once Discord has made it: another go sends the message
    /// into it, not into a second thread.
    created: Cell<Option<Snowflake>>,
}

impl Form {
    fn text(&self) -> String {
        let buffer = self.message.buffer();
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
    }

    fn set_busy(&self, busy: bool) {
        self.submitting.set(busy);
        self.create.set_sensitive(!busy);
        self.name.set_sensitive(!busy);
        self.private.set_sensitive(!busy);
        self.message.set_sensitive(!busy);
        for button in self.attach.iter().chain(&self.sticker_button) {
            button.set_sensitive(!busy);
        }
        self.stop.set_visible(false);
    }

    fn show_errors(&self, name: Option<&str>, message: Option<&str>) {
        for (label, error) in [(&self.name_error, name), (&self.message_error, message)] {
            label.set_label(error.unwrap_or_default());
            label.set_visible(error.is_some());
        }
    }
}

impl App {
    /// The open channel, if the attach menu offers Create Thread there now:
    /// with the message box, or on the notice that replaces it where only
    /// Send Messages is missing (see [`discord::notice::Notice`]).
    pub(super) fn thread_channel(&self) -> Option<Snowflake> {
        let channel = self.account.state.borrow().channel?;
        let button = self.composer_box.is_visible() || self.drafts.notice_attach.parent().is_some();
        let offered =
            button && self.editing().is_none() && self.account.state.borrow().offers_thread_in_attach_menu(channel);
        offered.then_some(channel)
    }

    /// Open the form for a thread in `parent`, on `from` if given. A thread
    /// in the channel starts with what is in the message box, which the
    /// client moves into the form.
    pub(super) fn open_thread_form(self: &Rc<Self>, parent: Snowflake, from: Option<Message>, location: &'static str) {
        if self.thread_form.borrow().is_some() {
            return;
        }
        let privacy = match from {
            Some(_) => Privacy::Public,
            None => self.account.state.borrow().thread_privacy(parent),
        };
        let default = from.as_ref().map(|message| self.default_thread_name(message));
        let form = Rc::new(self.build_thread_form(parent, privacy, default.as_deref()));
        self.thread_form.replace(Some(form.clone()));
        self.guard_thread_form(&form);
        self.connect_thread_files(&form);
        self.connect_thread_stickers(&form);
        if from.is_none() {
            let buffer = self.composer.buffer();
            let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            form.message.buffer().set_text(&typed);
        }
        // Create, Enter, and a sticker picked from an empty form all go the same way.
        let submit: Rc<dyn Fn()> = Rc::new({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(&form));
            move || {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.submit_thread_form(&form, parent, from.as_ref(), privacy, default.as_deref(), location);
                }
            }
        });
        form.submit.replace(Some(submit.clone()));
        form.create.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        form.name.connect_activate({
            let form = Rc::downgrade(&form);
            move |_| {
                if let Some(form) = form.upgrade() {
                    form.message.grab_focus();
                }
            }
        });
        // Enter creates the thread, as in the message box; Shift+Enter breaks the line.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            use gtk::gdk::{Key, ModifierType};
            let enter = matches!(key, Key::Return | Key::KP_Enter);
            if !enter || modifiers.contains(ModifierType::SHIFT_MASK) {
                return glib::Propagation::Proceed;
            }
            submit();
            glib::Propagation::Stop
        });
        form.message.add_controller(keys);
        self.refresh_thread_slowmode(&form, parent);
        glib::timeout_add_local(Duration::from_secs(1), {
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(&form));
            move || match (this.upgrade(), form.upgrade()) {
                (Some(this), Some(form)) => {
                    this.refresh_thread_slowmode(&form, parent);
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
        form.dialog.present(Some(&self.window));
        form.name.grab_focus();
    }

    /// The `t` key on a focused message (812266): a thread from it where one
    /// may be started, else the thread it started, if we may see it. Whether
    /// it did something, which then takes the key.
    pub(super) fn thread_shortcut(self: &Rc<Self>, message: &Message) -> bool {
        if self.account.state.borrow().can_start_thread_from(message) {
            self.open_thread_form(message.channel_id, Some(message.clone()), FROM_SHORTCUT);
            return true;
        }
        if self.has_viewable_thread(message) {
            self.open_thread(message.id);
            return true;
        }
        false
    }

    /// Show how long before another thread in `parent`, while it lasts.
    fn refresh_thread_slowmode(&self, form: &Form, parent: Snowflake) {
        let wait = self.thread_slowmode_left(parent);
        form.slowmode.set_visible(wait > 0);
        if wait > 0 {
            form.slowmode.set_label(&format!(
                "Slowmode is enabled. You can create another thread in this channel in {}.",
                wait_words((wait + 999) / 1000)
            ));
        }
    }

    /// What a thread on `message` is called when its name is left empty.
    fn default_thread_name(&self, message: &Message) -> String {
        let forwarded = message.forwarded();
        let content = forwarded.as_ref().unwrap_or(message);
        let names = self.message_names(content);
        let text = render::plain_text(&discord::markup::parse(&content.content), &names);
        let title = content.embeds.first().and_then(|e| e.title.as_deref());
        let question = message.poll.as_ref().and_then(|p| p.question.text.as_deref());
        thread_creation::default_name(title, question, &text)
    }

    fn build_thread_form(self: &Rc<Self>, parent: Snowflake, privacy: Privacy, default: Option<&str>) -> Form {
        let name = gtk::Entry::builder()
            .placeholder_text(default.filter(|d| !d.is_empty()).unwrap_or(NAME_PLACEHOLDER))
            .activates_default(false)
            .build();
        // The limit is in UTF-16 units, and a name is a single line.
        name.connect_changed(|entry| {
            let text = entry.text();
            let kept = thread_creation::single_line(thread_creation::cut(&text, NAME_MAX), false);
            if kept != text {
                entry.set_text(&kept);
                entry.set_position(-1);
            }
        });
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave({
            let name = name.downgrade();
            move |_| {
                if let Some(name) = name.upgrade() {
                    let text = name.text();
                    let trimmed = thread_creation::single_line(&text, true);
                    if trimmed != text {
                        name.set_text(&trimmed);
                    }
                }
            }
        });
        name.add_controller(focus);
        let optional = if default.is_some() { " (Optional)" } else { "" };
        let message = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&message)
            .min_content_height(120)
            .max_content_height(240)
            .propagate_natural_height(true)
            .css_classes(["card"])
            .build();
        let private = adw::SwitchRow::builder()
            .title("Private Thread")
            .subtitle("Only people you invite and moderators can see")
            .active(privacy == Privacy::PrivateOnly)
            .sensitive(privacy == Privacy::Choice)
            .build();
        let hint = gtk::Label::builder()
            .label("You can invite new people by @mentioning them.")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .visible(private.is_active())
            .build();
        private.connect_active_notify({
            let hint = hint.downgrade();
            move |row| {
                if let Some(hint) = hint.upgrade() {
                    hint.set_visible(row.is_active());
                }
            }
        });
        let switch =
            gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        switch.append(&private);
        let error = || gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["error"]).visible(false).build();
        let (name_error, message_error) = (error(), error());
        let slowmode = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label"]).visible(false).build();
        let label = |text: &str| gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading"]).build();
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.append(&label(&format!("Thread Name{optional}")));
        body.append(&name);
        body.append(&name_error);
        if privacy != Privacy::Public {
            body.append(&switch);
            body.append(&hint);
        }
        body.append(&label("Message"));
        body.append(&scroller);
        body.append(&message_error);
        let files = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let attach =
            self.account.state.borrow().can_attach_to_new_thread(parent).then(|| {
                gtk::Button::builder().label("Add Files").halign(gtk::Align::Start).css_classes(["flat"]).build()
            });
        let strip = super::stickers::Strip::new();
        let stickers = {
            let bar = chat_bar::Bar { open: true, customizable: self.chat_bar_customizable(), ..Default::default() };
            chat_bar::shown(bar, self.chat_bar_prefs()).sticker
        };
        let sticker_button = stickers.then(|| {
            gtk::Button::builder().label("Add Sticker").halign(gtk::Align::Start).css_classes(["flat"]).build()
        });
        let buttons = gtk::Box::builder().spacing(6).build();
        for button in attach.iter().chain(&sticker_button) {
            buttons.append(button);
        }
        body.append(&strip.bar);
        body.append(&files);
        body.append(&buttons);
        body.append(&slowmode);

        let create = gtk::Button::builder().label("Create").css_classes(["suggested-action"]).build();
        let stop = gtk::Button::builder().label("Cancel").visible(false).build();
        let header = adw::HeaderBar::new();
        header.pack_end(&create);
        header.pack_start(&stop);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body));
        let dialog = adw::Dialog::builder().title("Create Thread").content_width(480).child(&toolbar).build();
        let form = Form {
            parent,
            dialog,
            name,
            name_error,
            private,
            message,
            message_error,
            files,
            attach,
            strip,
            sticker_button,
            submit: RefCell::default(),
            slowmode,
            create,
            stop,
            submitting: Cell::new(false),
            created: Cell::new(None),
        };
        // A refusal is said until its field is changed, which answers it.
        form.name.connect_changed({
            let error = form.name_error.downgrade();
            move |_| {
                if let Some(error) = error.upgrade() {
                    error.set_visible(false);
                }
            }
        });
        form.message.buffer().connect_changed({
            let error = form.message_error.downgrade();
            move |_| {
                if let Some(error) = error.upgrade() {
                    error.set_visible(false);
                }
            }
        });
        form
    }

    /// Closing a form with something typed, or files added, asks first, as
    /// the client does; not at all while its message is on its way.
    fn guard_thread_form(self: &Rc<Self>, form: &Rc<Form>) {
        self.refresh_thread_guard(form);
        let changed = {
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            Rc::new(move || {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.refresh_thread_guard(&form);
                }
            })
        };
        form.name.connect_changed({
            let changed = changed.clone();
            move |_| changed()
        });
        form.message.buffer().connect_changed(move |_| changed());
        form.dialog.connect_close_attempt({
            let (this, dialog) = (Rc::downgrade(self), form.dialog.downgrade());
            move |_| {
                let (Some(this), Some(dialog)) = (this.upgrade(), dialog.upgrade()) else { return };
                let ask = super::friends::confirm(
                    "Are you sure you want to go?",
                    "Any changes you made will be lost if you exit now.",
                    "_Exit",
                    adw::ResponseAppearance::Destructive,
                );
                ask.set_response_label("cancel", "_Back to editing");
                glib::spawn_future_local(async move {
                    if ask.choose_future(Some(&this.window)).await == "confirm" {
                        dialog.force_close();
                    }
                });
            }
        });
        form.dialog.connect_closed({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.thread_form_closed(&form);
                }
            }
        });
    }

    fn refresh_thread_guard(&self, form: &Form) {
        let empty = form.name.text().is_empty()
            && form.text().trim().is_empty()
            && self.drafts.of(Draft::FirstThreadMessage(form.parent)).is_empty()
            && self.waiting.of(Draft::FirstThreadMessage(form.parent)).is_none();
        form.dialog.set_can_close(empty && !form.submitting.get());
    }

    /// The form's request starts or ends.
    fn thread_form_busy(&self, form: &Form, busy: bool) {
        form.set_busy(busy);
        self.refresh_thread_guard(form);
    }

    /// The form is gone: the files it had go too, unless they went in a message.
    fn thread_form_closed(self: &Rc<Self>, form: &Rc<Form>) {
        if self.thread_form.borrow().as_ref().is_some_and(|open| Rc::ptr_eq(open, form)) {
            self.thread_form.replace(None);
        }
        self.drop_post_files(Draft::FirstThreadMessage(form.parent));
        self.waiting.set(Draft::FirstThreadMessage(form.parent), None);
    }

    /// The channel whose first-message draft the open form takes files for now.
    pub(super) fn thread_form_channel(&self) -> Option<Snowflake> {
        let parent = self.thread_form.borrow().as_ref()?.parent;
        self.account.state.borrow().can_attach_to_new_thread(parent).then_some(parent)
    }

    /// Show the open form's files as the draft is now.
    pub(super) fn refresh_thread_files(self: &Rc<Self>) {
        let Some(form) = self.thread_form.borrow().clone() else { return };
        let may_stop = self.fill_tray(&form.files, Draft::FirstThreadMessage(form.parent));
        form.stop.set_sensitive(may_stop);
        self.refresh_thread_guard(&form);
    }

    /// Wire the form's sticker button: the picker, and taking a pick back.
    fn connect_thread_stickers(self: &Rc<Self>, form: &Rc<Form>) {
        if let Some(button) = &form.sticker_button {
            button.connect_clicked({
                let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
                move |button| {
                    let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) else { return };
                    let weak = Rc::downgrade(&form);
                    this.open_sticker_picker(form.parent, button, move |app, entry| {
                        if let Some(form) = weak.upgrade() {
                            app.pick_form_sticker(&form, entry);
                        }
                    });
                }
            });
        }
        form.strip.remove.connect_clicked({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.set_form_sticker(&form, None);
                }
            }
        });
    }

    /// Wait to send `listed` with the first message, or stop waiting (`None`).
    fn set_form_sticker(&self, form: &Form, listed: Option<discord::stickers::Listed>) {
        self.waiting.set(Draft::FirstThreadMessage(form.parent), listed.clone());
        form.strip.show(self, listed);
        self.refresh_thread_guard(form);
    }

    /// What a pick does in the form (`nE` in 989161): with nothing else in
    /// it, Create at once with the sticker alone, as a box sends one; else
    /// it waits for the message. Create turning it down (no name, say) keeps
    /// nothing, as the client does.
    fn pick_form_sticker(self: &Rc<Self>, form: &Rc<Form>, entry: &discord::stickers::Entry) {
        let draft = Draft::FirstThreadMessage(form.parent);
        let alone =
            form.text().trim().is_empty() && self.drafts.of(draft).is_empty() && self.waiting.of(draft).is_none();
        self.set_form_sticker(form, Some(entry.listed.clone()));
        if alone {
            let submit = form.submit.borrow().clone();
            if let Some(submit) = submit {
                submit();
            }
            if !form.submitting.get() {
                self.set_form_sticker(form, None);
            }
        }
    }

    /// Wire the form's file buttons: adding by the picker or by a drop, and
    /// taking back what is going up.
    fn connect_thread_files(self: &Rc<Self>, form: &Rc<Form>) {
        let draft = Draft::FirstThreadMessage(form.parent);
        if let Some(attach) = &form.attach {
            attach.connect_clicked({
                let this = Rc::downgrade(self);
                move |_| {
                    if let Some(this) = this.upgrade() {
                        this.choose_form_files(draft);
                    }
                }
            });
        }
        form.stop.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.cancel_sending_of(draft);
                }
            }
        });
        let target = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        target.connect_drop({
            let this = Rc::downgrade(self);
            move |_, value, _, _| {
                let (Some(this), Ok(list)) = (this.upgrade(), value.get::<gtk::gdk::FileList>()) else { return false };
                let files = list.files();
                glib::spawn_future_local(async move { this.add_paths(draft, files).await });
                true
            }
        });
        form.dialog.add_controller(target);
        self.refresh_thread_files();
    }

    fn submit_thread_form(
        self: &Rc<Self>,
        form: &Rc<Form>,
        parent: Snowflake,
        from: Option<&Message>,
        privacy: Privacy,
        default: Option<&str>,
        location: &'static str,
    ) {
        if form.submitting.get() {
            return;
        }
        let draft = Draft::FirstThreadMessage(parent);
        let files = self.drafts.of(draft);
        let waiting = self.waiting.of(draft);
        let text = form.text().trim().to_owned();
        let name = form.name.text().to_string();
        let missing = thread_creation::missing(from.is_some(), &name, &text, !files.is_empty() || waiting.is_some());
        let says = |what| missing.contains(&what);
        form.show_errors(
            says(Missing::Name).then_some("Thread Name is required"),
            says(Missing::Message).then_some("Starter Message is required"),
        );
        if !missing.is_empty() {
            return;
        }
        // What was true when the sticker was picked may not be now.
        if let Some(listed) = &waiting
            && self.sticker_sendable(parent, listed).is_err()
        {
            self.set_form_sticker(form, None);
            return form.show_errors(None, Some("That sticker can't be sent here."));
        }
        // `:name:`s go out as emoji, and the length is of what goes out.
        let library = self.emoji_library(parent);
        let translated = library.translate(&text);
        let (text, used) = (translated.text, translated.used);
        if let Err(too_long) = self.account.state.borrow().limits().check_length(&text) {
            let dialog = adw::AlertDialog::new(Some(too_long.heading()), Some(&too_long.body()));
            dialog.add_response("okay", "_Okay");
            dialog.present(Some(&form.dialog));
            return;
        }
        let (rest, channel, limits) = {
            let state = self.account.state.borrow();
            let found = state.find_channel(parent);
            let limits = state.limits().uploads(found.and_then(|(guild, _)| guild));
            (state.rest.clone(), found.map(|(_, c)| c.clone()), limits)
        };
        let (Some(rest), Some(channel)) = (rest, channel) else { return };
        if !files.is_empty()
            && let Err(refusal) = discord::uploads::check_send(&limits, &Self::sizes(&files))
        {
            return self.show_refusal(&refusal);
        }
        let private = from.is_none() && privacy.is_private(form.private.is_active());
        let name = if name.is_empty() { default.unwrap_or_default().to_owned() } else { name };
        let body = thread_creation::body(&channel, &name, private, location);
        let from_message = from.map(|m| m.id);
        let from_composer = from.is_none();
        self.thread_form_busy(form, true);
        form.stop.set_visible(!files.is_empty());
        form.show_errors(None, None);
        let (this, form) = (self.clone(), form.clone());
        glib::spawn_future_local(async move {
            let thread = match form.created.get() {
                Some(thread) => thread,
                None => match rest.create_thread(parent, from_message, body).await {
                    Ok(thread) => {
                        this.record_slowmode(parent, |slowmode, c, bypass| slowmode.thread_made(c, bypass, now_ms()));
                        form.created.set(Some(thread.id));
                        thread.id
                    }
                    Err(error) => {
                        this.thread_form_busy(&form, false);
                        return this.thread_refused(&form, parent, &Refused::from(&error), &error);
                    }
                },
            };
            // The client waits for the gateway to bring the thread, and sends
            // the first message into it once it's there.
            if !this.thread_arrived(thread).await {
                // Another go sends into this thread, not into a second one.
                this.thread_form_busy(&form, false);
                let dialog = adw::AlertDialog::new(
                    Some("Cannot create thread"),
                    Some("The thread was created, but Discord hasn't announced it yet. Try again in a moment."),
                );
                dialog.add_response("okay", "_Okay");
                dialog.present(Some(&form.dialog));
                return;
            }
            let sticker = waiting.as_ref().map(|w| w.sticker.id);
            let message = Outgoing { text: &text, reply: None, sticker, at: None, tts: false };
            match this.send_draft(&rest, draft, thread, &message, files).await {
                Ok(()) => {
                    this.track_message_use(&used, sticker);
                    this.set_form_sticker(&form, None);
                    this.message_sent(thread);
                    form.dialog.force_close();
                    if from_composer {
                        this.set_composer_text("");
                    }
                    if this.account.state.borrow().channel == Some(parent) {
                        this.select_channel(thread);
                    }
                }
                Err(stopped) => {
                    this.thread_form_busy(&form, false);
                    this.thread_message_stopped(&form, thread, &stopped);
                }
            }
        });
    }

    /// Say why the first message didn't go out, with the thread made: next
    /// to the message, or an alert for a file, as the client does. The files
    /// stay for another go, unless they were taken back.
    fn thread_message_stopped(self: &Rc<Self>, form: &Form, thread: Snowflake, stopped: &Stopped) {
        let why = self.record_stopped(thread, stopped);
        match (stopped, why) {
            (Stopped::Upload(why), _) => {
                let dialog = adw::AlertDialog::new(Some("Upload Failed"), Some(why));
                dialog.add_response("okay", "_Okay");
                dialog.present(Some(&form.dialog));
            }
            (_, Some(why)) => form.show_errors(None, Some(&why)),
            (_, None) => {}
        }
    }

    /// Say why Discord turned a thread down, as the client does: an alert for
    /// most, next to the name for the name's own refusals, and slowmode
    /// recorded without a word.
    fn thread_refused(self: &Rc<Self>, form: &Form, parent: Snowflake, refused: &Refused, error: &discord::Error) {
        match refused {
            Refused::Slowmode(_) => self.thread_slowmode_refused(parent, error),
            Refused::AutoMod { reason: Some(reason), .. } => form.show_errors(Some(reason), None),
            Refused::Name => form.show_errors(Some("Thread Name is required"), None),
            _ => {
                if let Some((heading, body)) = refusal_alert(refused, error) {
                    let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
                    dialog.add_response("okay", "_Okay");
                    dialog.present(Some(&form.dialog));
                }
            }
        }
    }

    /// Wait for the gateway to bring thread `id`, where we can see it: a
    /// private one only once our membership has arrived. Gives up after a
    /// while, which the client never does.
    pub(super) async fn thread_arrived(self: &Rc<Self>, id: Snowflake) -> bool {
        let (sender, receiver) = async_channel::bounded(1);
        self.thread_waiters.borrow_mut().push((id, sender));
        self.thread_arrivals();
        let arrived = glib::future_with_timeout(Duration::from_secs(15), receiver.recv()).await;
        self.thread_waiters.borrow_mut().retain(|(waiting, _)| *waiting != id);
        arrived.is_ok()
    }

    /// Let those waiting for a thread know if it is here.
    pub(super) fn thread_arrivals(&self) {
        let state = self.account.state.borrow();
        self.thread_waiters.borrow_mut().retain(|(id, sender)| {
            let here = state.can_view(*id);
            if here {
                let _ = sender.try_send(());
            }
            !here
        });
    }
}

/// The alert the client shows for a thread Discord refused, its heading and
/// body (`Y` in 971389); none for slowmode, whose wait is only recorded, nor
/// for the name's own refusals, which show by the name.
pub(super) fn refusal_alert(refused: &Refused, error: &discord::Error) -> Option<(&'static str, String)> {
    const CANNOT: &str = "Cannot create thread";
    let text = match refused {
        Refused::TooManyThreads => {
            "This server has reached the maximum number of active threads. Free up space by archiving a thread."
        }
        Refused::TooManyAnnouncementThreads => {
            "This server has reached the maximum number of active announcement threads. Free up space by archiving a thread."
        }
        Refused::RateLimited => "You are being rate limited.",
        Refused::Slowmode(_) | Refused::AutoMod { reason: Some(_), .. } | Refused::Name => return None,
        Refused::AutoMod { reason: None, .. } | Refused::Other => {
            return Some(match error {
                discord::Error::Http { .. } => ("Error", "An error occurred, please try again.".to_owned()),
                other => ("Error", other.to_string()),
            });
        }
    };
    Some((CANNOT, text.to_owned()))
}
