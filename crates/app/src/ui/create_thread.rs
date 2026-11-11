//! Create Thread: a form for a thread's name, whether it is private, and the
//! message that starts it. The official client opens it in the sidebar, from
//! the attach menu (a new thread in the channel) and from a message's menu
//! (a thread on that message); here it is a dialog. What it asks Discord for
//! is in [`discord::thread_creation`].

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::thread_creation::{
    self, FROM_ATTACH_MENU, FROM_MESSAGE_MENU, Missing, NAME_MAX, NAME_PLACEHOLDER, Privacy, Refused,
};
use discord::{Message, Snowflake};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::menus::{Entry, Item, Kind};
use super::uploads::Stopped;
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
            app.open_thread_form(channel, None);
        }
    }),
});

/// Create Thread in a message's menu, on a message that has no thread, which
/// somebody wrote (`n` in 406704).
pub(super) const MESSAGE_MENU: Entry<MessageTarget> = Entry::Item(Item {
    name: "create-thread",
    label: "Create T_hread",
    shows: |app, target| app.state.borrow().can_start_thread_from(target.shown()),
    kind: Kind::Action(|app, target| {
        let message = target.shown();
        app.open_thread_form(message.channel_id, Some(message.clone()));
    }),
});

/// The form's widgets.
struct Form {
    dialog: adw::Dialog,
    name: gtk::Entry,
    name_error: gtk::Label,
    private: adw::SwitchRow,
    message: gtk::TextView,
    message_error: gtk::Label,
    create: gtk::Button,
    /// The request is out: nothing changes until it answers.
    submitting: Cell<bool>,
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
        let channel = self.state.borrow().channel?;
        let button = self.composer_box.is_visible() || self.drafts.notice_attach.parent().is_some();
        let offered = button && self.editing().is_none() && self.state.borrow().offers_thread_in_attach_menu(channel);
        offered.then_some(channel)
    }

    /// Open the form for a thread in `parent`, on `from` if given. A thread
    /// in the channel starts with what is in the message box, which the
    /// client moves into the form.
    pub(super) fn open_thread_form(self: &Rc<Self>, parent: Snowflake, from: Option<Message>) {
        let privacy = match from {
            Some(_) => Privacy::Public,
            None => self.state.borrow().thread_privacy(parent),
        };
        let default = from.as_ref().map(|message| self.default_thread_name(message));
        let form = Rc::new(self.build_thread_form(privacy, default.as_deref()));
        if from.is_none() {
            let buffer = self.composer.buffer();
            let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            form.message.buffer().set_text(&typed);
        }
        let this = Rc::downgrade(self);
        let location = if from.is_some() { FROM_MESSAGE_MENU } else { FROM_ATTACH_MENU };
        form.create.connect_clicked({
            let (this, form, from, default) = (this.clone(), form.clone(), from.clone(), default.clone());
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.submit_thread_form(&form, parent, from.as_ref(), privacy, default.as_deref(), location);
                }
            }
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
        keys.connect_key_pressed({
            let (this, form, from, default) = (this, form.clone(), from, default);
            move |_, key, _, modifiers| {
                use gtk::gdk::{Key, ModifierType};
                let enter = matches!(key, Key::Return | Key::KP_Enter);
                let Some(this) = this.upgrade().filter(|_| enter && !modifiers.contains(ModifierType::SHIFT_MASK))
                else {
                    return glib::Propagation::Proceed;
                };
                this.submit_thread_form(&form, parent, from.as_ref(), privacy, default.as_deref(), location);
                glib::Propagation::Stop
            }
        });
        form.message.add_controller(keys);
        form.dialog.present(Some(&self.window));
        form.name.grab_focus();
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

    fn build_thread_form(self: &Rc<Self>, privacy: Privacy, default: Option<&str>) -> Form {
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

        let create = gtk::Button::builder().label("Create").css_classes(["suggested-action"]).build();
        let header = adw::HeaderBar::new();
        header.pack_end(&create);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body));
        let dialog = adw::Dialog::builder().title("Create Thread").content_width(480).child(&toolbar).build();
        let form =
            Form { dialog, name, name_error, private, message, message_error, create, submitting: Cell::new(false) };
        self.guard_thread_form(&form);
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

    /// Closing a form with something typed asks first, as the client does.
    fn guard_thread_form(self: &Rc<Self>, form: &Form) {
        let update = {
            let (dialog, name, message) = (form.dialog.downgrade(), form.name.downgrade(), form.message.downgrade());
            move || {
                let (Some(dialog), Some(name), Some(message)) = (dialog.upgrade(), name.upgrade(), message.upgrade())
                else {
                    return;
                };
                let buffer = message.buffer();
                let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                dialog.set_can_close(name.text().is_empty() && typed.trim().is_empty());
            }
        };
        let update = Rc::new(update);
        update();
        form.name.connect_changed({
            let update = update.clone();
            move |_| update()
        });
        form.message.buffer().connect_changed(move |_| update());
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
        let typed = form.text().trim().to_owned();
        let text = typed.clone();
        let name = form.name.text().to_string();
        let missing = thread_creation::missing(from.is_some(), &name, &text, false);
        let says = |what| missing.contains(&what);
        form.show_errors(
            says(Missing::Name).then_some("Thread Name is required"),
            says(Missing::Message).then_some("Starter Message is required"),
        );
        if !missing.is_empty() {
            return;
        }
        // `:name:`s go out as emoji, and the length is of what goes out.
        let library = self.emoji_library(parent);
        let translated = library.translate(&text);
        let text = translated.text;
        if let Err(too_long) = self.state.borrow().limits().check_length(&text) {
            let dialog = adw::AlertDialog::new(Some(too_long.heading()), Some(&too_long.body()));
            dialog.add_response("okay", "_Okay");
            dialog.present(Some(&form.dialog));
            return;
        }
        let (rest, channel) = {
            let state = self.state.borrow();
            (state.rest.clone(), state.find_channel(parent).map(|(_, c)| c.clone()))
        };
        let (Some(rest), Some(channel)) = (rest, channel) else { return };
        let private = from.is_none() && privacy.is_private(form.private.is_active());
        let name = if name.is_empty() { default.unwrap_or_default().to_owned() } else { name };
        let body = thread_creation::body(&channel, &name, private, location);
        let from_message = from.map(|m| m.id);
        let from_composer = from.is_none();
        form.set_busy(true);
        form.show_errors(None, None);
        let (this, form) = (self.clone(), form.clone());
        glib::spawn_future_local(async move {
            let thread = match rest.create_thread(parent, from_message, body).await {
                Ok(thread) => thread,
                Err(error) => {
                    form.set_busy(false);
                    return this.thread_refused(&form, &discord::thread_creation::Refused::from(&error), &error);
                }
            };
            // The client waits for the gateway to bring the thread, and sends
            // the first message into it once it's there.
            if !this.thread_arrived(thread.id).await {
                // Another go would start a second thread, so the form closes,
                // and its message goes back to the message box.
                form.dialog.force_close();
                this.set_composer_text(&typed);
                let dialog = adw::AlertDialog::new(
                    Some("Cannot create thread"),
                    Some(
                        "The thread was created, but Discord hasn't announced it yet. Your message is in the message box.",
                    ),
                );
                dialog.add_response("okay", "_Okay");
                dialog.present(Some(&this.window));
                return;
            }
            form.dialog.force_close();
            if from_composer {
                this.set_composer_text("");
            }
            if this.state.borrow().channel == Some(parent) {
                this.select_channel(thread.id);
            }
            match rest.send_message(thread.id, &text, None, &[], None).await {
                Ok(_) => this.message_sent(thread.id),
                Err(error) => this.message_stopped(thread.id, &Stopped::Send(error)),
            }
        });
    }

    /// Say why Discord turned a thread down, as the client does: an alert for
    /// most, next to the name for the name's own refusals.
    fn thread_refused(self: &Rc<Self>, form: &Form, refused: &Refused, error: &discord::Error) {
        let alert = |heading: &str, body: &str| {
            let dialog = adw::AlertDialog::new(Some(heading), Some(body));
            dialog.add_response("okay", "_Okay");
            dialog.present(Some(&form.dialog));
        };
        const CANNOT: &str = "Cannot create thread";
        match refused {
            Refused::TooManyThreads => alert(
                CANNOT,
                "This server has reached the maximum number of active threads. Free up space by archiving a thread.",
            ),
            Refused::TooManyAnnouncementThreads => alert(
                CANNOT,
                "This server has reached the maximum number of active announcement threads. Free up space by archiving a thread.",
            ),
            Refused::RateLimited => alert(CANNOT, "You are being rate limited."),
            Refused::Slowmode(seconds) => {
                alert(CANNOT, &format!("Slowmode is on in this channel. Try again in {seconds} seconds."))
            }
            Refused::AutoMod { reason: Some(reason), .. } => form.show_errors(Some(reason), None),
            Refused::Name => form.show_errors(Some("Thread Name is required"), None),
            Refused::AutoMod { reason: None, .. } | Refused::Other => match error {
                discord::Error::Http { .. } => alert("Error", "An error occurred, please try again."),
                other => alert("Error", &other.to_string()),
            },
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
        let state = self.state.borrow();
        self.thread_waiters.borrow_mut().retain(|(id, sender)| {
            let here = state.can_view(*id);
            if here {
                let _ = sender.try_send(());
            }
            !here
        });
    }
}
