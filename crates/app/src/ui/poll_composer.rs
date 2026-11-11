//! The poll editor, opened from the attach menu's Create Poll. What a poll
//! holds, refuses and sends is `discord::poll_draft`; this is the dialog
//! around it. The emoji for an answer come from the same picker as the
//! message box's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::emoji::Item;
use discord::model::Emoji;
use discord::poll_draft::{self, ANSWER_MAX, Answer, Draft, Duration, Errors, QUESTION_MAX};
use discord::{Rest, Snowflake};
use gtk::{gdk, glib};

use super::App;
use super::emoji;
use super::menus::{Entry, Item as MenuItem, Kind};
use super::scheduling::{self, Picker};
use super::state::{now_ms, now_unix};

/// Create Poll, in the attach menu.
pub(super) const CREATE_POLL: Entry<()> = Entry::Item(MenuItem {
    name: "create-poll",
    label: "Create _Poll",
    shows: |app, _| app.poll_channel().is_some(),
    kind: Kind::Action(|app, _| app.create_poll()),
});

/// One answer's widgets, in the order of [`Draft::answers`].
struct Row {
    root: gtk::Box,
    entry: gtk::Entry,
    /// Picks its emoji, or asks what to do with the one it has.
    emoji: gtk::Button,
    menu: gtk::Popover,
    /// What the button shows, for the draw; the draft has the emoji as sent.
    picked: RefCell<Option<Item>>,
    error: gtk::Label,
    remove: gtk::Button,
}

impl Drop for Row {
    fn drop(&mut self) {
        self.menu.unparent();
    }
}

struct Editor {
    channel: Snowflake,
    dialog: glib::WeakRef<adw::Dialog>,
    draft: RefCell<Draft>,
    banner: gtk::Label,
    question_count: gtk::Label,
    question_error: gtk::Label,
    answers: gtk::Box,
    rows: RefCell<Vec<Rc<Row>>>,
    add: gtk::Button,
    post: gtk::Button,
    sending: Cell<bool>,
    /// Opens the time picker, and says when the poll goes out if it has one.
    schedule: gtk::Button,
    at: Cell<Option<i64>>,
}

fn error_label() -> gtk::Label {
    gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["error"]).visible(false).build()
}

/// Show `text` in `label`, or hide it.
fn show_error(label: &gtk::Label, text: Option<&str>) {
    label.set_label(text.unwrap_or_default());
    label.set_visible(text.is_some());
}

/// Stop `entry` at `max` of the client's length units: what is typed or
/// pasted past it is left out, not the text already there. Typing reaches
/// the text the entry delegates to, not the entry.
fn limit(entry: &gtk::Entry, max: usize) {
    let Some(text) = entry.delegate() else { return };
    text.connect_insert_text(move |text, new, position| {
        let held = text.text().encode_utf16().count();
        let fits = poll_draft::fit(new, held, max);
        if fits.len() != new.len() {
            text.stop_signal_emission_by_name("insert-text");
            if !fits.is_empty() {
                // Runs this handler again, now for text that fits.
                text.insert_text(fits, position);
            }
        }
    });
}

impl Row {
    fn new(app: &Rc<App>, editor: &Rc<Editor>) -> Rc<Row> {
        let entry = gtk::Entry::builder().placeholder_text("Type your answer").hexpand(true).build();
        limit(&entry, ANSWER_MAX);
        let emoji = gtk::Button::builder()
            .icon_name("face-smile-symbolic")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text("Add emoji")
            .build();
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text("Remove answer")
            .build();
        let menu = gtk::Popover::new();
        let choices = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let replace = gtk::Button::builder().label("Replace Emoji").css_classes(["flat"]).build();
        let delete = gtk::Button::builder().label("Remove Emoji").css_classes(["flat"]).build();
        choices.append(&replace);
        choices.append(&delete);
        menu.set_child(Some(&choices));
        menu.set_parent(&emoji);

        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&emoji);
        line.append(&entry);
        line.append(&remove);
        let error = error_label();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.append(&line);
        root.append(&error);
        let row = Rc::new(Row { root, entry, emoji, menu, picked: RefCell::new(None), error, remove });

        // What each control does with the app, the editor and its row,
        // held weakly so the widgets don't keep the editor alive.
        let (app_weak, editor_weak, row_weak) = (Rc::downgrade(app), Rc::downgrade(editor), Rc::downgrade(&row));
        let with = move |act: fn(&Rc<App>, &Rc<Editor>, &Rc<Row>)| {
            let (app, editor, row) = (app_weak.clone(), editor_weak.clone(), row_weak.clone());
            move || {
                if let (Some(app), Some(editor), Some(row)) = (app.upgrade(), editor.upgrade(), row.upgrade()) {
                    act(&app, &editor, &row);
                }
            }
        };
        let typed = with(|_, editor, row| {
            if let Some(at) = editor.index_of(row) {
                editor.draft.borrow_mut().answers[at].text = row.entry.text().into();
                show_error(&row.error, None);
            }
        });
        row.entry.connect_changed(move |_| typed());
        // Clicking an emoji already there asks what to do with it.
        let click = with(|app, editor, row| match row.picked.borrow().is_some() {
            true => row.menu.popup(),
            false => editor.pick_emoji(app, row),
        });
        row.emoji.connect_clicked(move |_| click());
        let replace_with = with(|app, editor, row| {
            row.menu.popdown();
            editor.pick_emoji(app, row);
        });
        replace.connect_clicked(move |_| replace_with());
        let delete_with = with(|app, editor, row| {
            row.menu.popdown();
            editor.set_emoji(app, row, None);
        });
        delete.connect_clicked(move |_| delete_with());
        let remove_with = with(|_, editor, row| editor.remove(row));
        row.remove.connect_clicked(move |_| remove_with());
        // Enter on the last answer starts another; on any, it moves down.
        let activate = with(|app, editor, row| editor.next_answer(app, row));
        row.entry.connect_activate(move |_| activate());
        row
    }

    /// Draw the emoji chosen, or the button that picks one.
    fn show_emoji(&self, app: &App, item: Option<Item>) {
        match &item {
            Some(item) => {
                self.emoji.set_child(Some(&emoji::glyph(item, &app.images)));
                self.emoji.set_tooltip_text(Some("Update emoji"));
            }
            None => {
                self.emoji.set_child(None::<&gtk::Widget>);
                self.emoji.set_icon_name("face-smile-symbolic");
                self.emoji.set_tooltip_text(Some("Add emoji"));
            }
        }
        *self.picked.borrow_mut() = item;
    }
}

impl Editor {
    fn index_of(&self, row: &Rc<Row>) -> Option<usize> {
        self.rows.borrow().iter().position(|r| Rc::ptr_eq(r, row))
    }

    /// The buttons that add and remove answers show as they can: the editor
    /// takes ten, and keeps one.
    fn refresh_buttons(&self) {
        let draft = self.draft.borrow();
        self.add.set_visible(draft.can_add());
        for row in self.rows.borrow().iter() {
            row.remove.set_visible(draft.can_remove());
        }
    }

    fn add_answer(self: &Rc<Self>, app: &Rc<App>) -> Option<Rc<Row>> {
        if !self.draft.borrow().can_add() {
            return None;
        }
        let row = Row::new(app, self);
        self.draft.borrow_mut().answers.push(Answer::default());
        self.answers.append(&row.root);
        self.rows.borrow_mut().push(row.clone());
        self.refresh_buttons();
        Some(row)
    }

    fn remove(&self, row: &Rc<Row>) {
        if !self.draft.borrow().can_remove() {
            return;
        }
        let Some(at) = self.index_of(row) else { return };
        self.draft.borrow_mut().answers.remove(at);
        self.rows.borrow_mut().remove(at);
        self.answers.remove(&row.root);
        self.refresh_buttons();
        // Focus goes to the remove button of the answer that took its place,
        // or the one before it, and to the add button when one is left.
        let rows = self.rows.borrow();
        match rows.get(at).or_else(|| rows.last()).filter(|_| rows.len() > 1) {
            Some(next) => next.remove.grab_focus(),
            None => self.add.grab_focus(),
        };
    }

    /// What Enter does in `row`: on the last answer, add another, then in
    /// any case move to the one after.
    fn next_answer(self: &Rc<Self>, app: &Rc<App>, row: &Rc<Row>) {
        let Some(at) = self.index_of(row) else { return };
        if at + 1 == self.rows.borrow().len() {
            self.add_answer(app);
        }
        if let Some(next) = self.rows.borrow().get(at + 1) {
            next.entry.grab_focus();
        }
    }

    fn pick_emoji(self: &Rc<Self>, app: &Rc<App>, row: &Rc<Row>) {
        let (editor, row_weak) = (Rc::downgrade(self), Rc::downgrade(row));
        app.open_emoji_picker(self.channel, &row.emoji, discord::emoji::Intention::Chat, move |app, item| {
            let (Some(editor), Some(row)) = (editor.upgrade(), row_weak.upgrade()) else { return };
            editor.set_emoji(app, &row, Some(item.clone()));
        });
    }

    fn set_emoji(&self, app: &App, row: &Rc<Row>, item: Option<Item>) {
        let Some(at) = self.index_of(row) else { return };
        self.draft.borrow_mut().answers[at].emoji = item.as_ref().map(Emoji::from);
        show_error(&row.error, None);
        row.show_emoji(app, item);
    }

    /// Put each refusal where it belongs, and the cursor on the first.
    fn show_errors(&self, errors: &Errors, question: &gtk::Entry) {
        show_error(&self.question_error, errors.question);
        let rows = self.rows.borrow();
        for (row, error) in rows.iter().zip(&errors.answers) {
            show_error(&row.error, *error);
        }
        if errors.question.is_some() {
            question.grab_focus();
        } else if let Some(row) = errors.first_answer().and_then(|at| rows.get(at)) {
            row.entry.grab_focus();
        }
    }

    /// Say why nothing was posted, and take the Post button back.
    fn fail(&self, words: &str) {
        show_error(&self.banner, Some(words));
        self.post.set_sensitive(true);
        self.sending.set(false);
    }

    /// Have Discord post the poll at `at`, which `scheduled::check` has to
    /// allow still. The editor closes once it is scheduled.
    async fn schedule(self: &Rc<Self>, app: &Rc<App>, rest: &Rest, body: serde_json::Value, at: i64) {
        if let Err(refusal) = discord::scheduled::check(at, now_unix(), None) {
            return self.fail(refusal.text());
        }
        let result =
            rest.schedule_message(self.channel, &discord::scheduled::iso(at), "", None, &[], None, Some(body)).await;
        match result {
            Ok(()) => {
                app.toast_scheduled(at);
                if let Some(dialog) = self.dialog.upgrade() {
                    dialog.force_close();
                }
            }
            Err(e) if e.code() == Some(discord::scheduled::TOO_MANY) => {
                self.fail(&format!("Failed to schedule message. {e}"));
                if let Some(dialog) = self.dialog.upgrade() {
                    app.show_limit_reached(&dialog);
                }
            }
            Err(e) => self.fail(&format!("Failed to schedule message. {e}")),
        }
    }

    /// Check the poll and post it. The editor closes once it is posted.
    fn submit(self: &Rc<Self>, app: &Rc<App>, question: &gtk::Entry) {
        if self.sending.get() {
            return;
        }
        let errors = self.draft.borrow().check();
        self.show_errors(&errors, question);
        if !errors.is_empty() {
            return;
        }
        show_error(&self.banner, None);
        let channel = self.channel;
        // The editor follows our permissions, but they may have changed since.
        if !app.state.borrow().can_post_polls(channel) {
            return self.fail("You do not have permission to post polls in this channel.");
        }
        if app.slowmode_left(channel) > 0 {
            return self.fail("Slowmode is enabled in this channel. Try again once it ends.");
        }
        let Some(rest) = app.state.borrow().rest.clone() else { return };
        self.sending.set(true);
        self.post.set_sensitive(false);
        let body = self.draft.borrow().body();
        // Discord posts a scheduled poll later, so it starts no cooldown.
        if self.at.get().is_none() {
            app.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
        }
        let (this, app) = (self.clone(), app.clone());
        glib::spawn_future_local(async move { this.send(&app, &rest, body).await });
    }

    /// The time picker, for a poll that goes out later, and back to a poll
    /// that goes out now with Clear.
    fn pick_time(self: &Rc<Self>, app: &Rc<App>) {
        let Some(dialog) = self.dialog.upgrade() else { return };
        let start = self.at.get().unwrap_or_else(scheduling::default_start);
        let clear: Option<Box<dyn Fn()>> = self.at.get().map(|_| {
            let editor = Rc::downgrade(self);
            Box::new(move || {
                if let Some(editor) = editor.upgrade() {
                    editor.set_time(None);
                }
            }) as Box<dyn Fn()>
        });
        let editor = Rc::downgrade(self);
        app.pick_time(&dialog, Picker::NEW, start, None, clear, move |at| {
            if let Some(editor) = editor.upgrade() {
                editor.set_time(Some(at));
            }
        });
    }

    fn set_time(&self, at: Option<i64>) {
        self.at.set(at);
        let tip = at.map_or_else(|| "Schedule Poll".to_owned(), scheduling::scheduling_for);
        self.schedule.set_tooltip_text(Some(&tip));
        if at.is_some() {
            self.schedule.add_css_class("suggested-action");
        } else {
            self.schedule.remove_css_class("suggested-action");
        }
    }

    async fn send(self: &Rc<Self>, app: &Rc<App>, rest: &Rest, body: serde_json::Value) {
        let channel = self.channel;
        if let Some(at) = self.at.get() {
            return self.schedule(app, rest, body, at).await;
        }
        // The gateway echoes our own message back, which is what adds it to the list.
        match rest.send_poll(channel, body).await {
            Ok(_) => {
                app.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
                // Sending from an old stretch of history returns to the present.
                if app.timelines.borrow().get(channel).is_some_and(|t| t.detached) {
                    app.load_newest(channel);
                }
                if let Some(dialog) = self.dialog.upgrade() {
                    dialog.force_close();
                }
            }
            Err(e) if e.slowmode_wait().is_some() => {
                let wait = e.slowmode_wait().unwrap_or_default();
                app.record_slowmode(channel, |slowmode, c, _| slowmode.refused(c, wait, now_ms()));
                self.fail("Not sent: slowmode is enabled in this channel.");
            }
            Err(e) => {
                app.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                self.fail(&format!("Not sent: {e}"));
            }
        }
    }
}

impl App {
    /// The open channel, if the client would offer Create Poll in it now:
    /// the message box shows, no edit has it, and polls may be posted there.
    pub(super) fn poll_channel(&self) -> Option<Snowflake> {
        let channel = self.state.borrow().channel?;
        let allowed = self.composer_box.is_visible()
            && self.editing().is_none()
            && !self.has_voice_draft(channel)
            && self.state.borrow().can_post_polls(channel);
        allowed.then_some(channel)
    }

    /// Open the editor for a poll in the open channel.
    pub(super) fn create_poll(self: &Rc<Self>) {
        let Some(channel) = self.poll_channel() else { return };
        let dialog =
            adw::Dialog::builder().title("Create a Poll").content_width(480).follows_content_size(true).build();
        // Closing is for the editor to allow: it asks first when there is
        // something to lose.
        dialog.set_can_close(false);

        let heading = |text: &str| gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading"]).build();
        let banner = error_label();
        let question = gtk::Entry::builder().placeholder_text("What question do you want to ask?").build();
        limit(&question, QUESTION_MAX);
        let question_count =
            gtk::Label::builder().label(format!("0/{QUESTION_MAX}")).css_classes(["dim-label"]).build();
        let question_error = error_label();
        let question_head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        question_head.append(&heading("Question"));
        question_head.append(&gtk::Box::builder().hexpand(true).build());
        question_head.append(&question_count);

        let answers = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let add = gtk::Button::builder().label("Add another answer").icon_name("list-add-symbolic").build();
        let duration = gtk::DropDown::from_strings(&Duration::ALL.map(Duration::label));
        duration.set_selected(Duration::ALL.iter().position(|&d| d == Duration::default()).unwrap_or(0) as u32);
        let duration_line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        duration_line.append(&heading("Duration"));
        duration_line.append(&duration);

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(18)
            .margin_end(18)
            .build();
        body.append(&banner);
        body.append(&question_head);
        body.append(&question);
        body.append(&question_error);
        body.append(&heading("Answers"));
        body.append(&answers);
        body.append(&add);
        body.append(&duration_line);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&body)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(560)
            .build();

        let multiselect = gtk::CheckButton::with_label("Allow Multiple Answers");
        let post = gtk::Button::builder().label("Post").css_classes(["suggested-action"]).build();
        let schedule = gtk::Button::builder().icon_name("alarm-symbolic").tooltip_text("Schedule Poll").build();
        let actions = gtk::ActionBar::new();
        actions.pack_start(&multiselect);
        actions.pack_end(&post);
        actions.pack_end(&schedule);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scroller));
        toolbar.add_bottom_bar(&actions);
        dialog.set_child(Some(&toolbar));

        let editor = Rc::new(Editor {
            channel,
            dialog: dialog.downgrade(),
            draft: RefCell::new(Draft { answers: Vec::new(), ..Draft::default() }),
            banner,
            question_count,
            question_error,
            answers,
            rows: RefCell::default(),
            add: add.clone(),
            post: post.clone(),
            sending: Cell::new(false),
            schedule: schedule.clone(),
            at: Cell::new(None),
        });
        for _ in 0..Draft::default().answers.len() {
            editor.add_answer(self);
        }

        question.connect_changed({
            let editor = Rc::downgrade(&editor);
            move |question| {
                let Some(editor) = editor.upgrade() else { return };
                let text = question.text();
                editor.question_count.set_label(&format!("{}/{QUESTION_MAX}", text.encode_utf16().count()));
                editor.draft.borrow_mut().question = text.into();
                show_error(&editor.question_error, None);
            }
        });
        add.connect_clicked({
            let (app, editor) = (Rc::downgrade(self), Rc::downgrade(&editor));
            move |_| {
                if let (Some(app), Some(editor)) = (app.upgrade(), editor.upgrade())
                    && let Some(row) = editor.add_answer(&app)
                {
                    row.entry.grab_focus();
                }
            }
        });
        duration.connect_selected_notify({
            let editor = Rc::downgrade(&editor);
            move |duration| {
                if let (Some(editor), Some(&chosen)) =
                    (editor.upgrade(), Duration::ALL.get(duration.selected() as usize))
                {
                    editor.draft.borrow_mut().duration = chosen;
                }
            }
        });
        multiselect.connect_toggled({
            let editor = Rc::downgrade(&editor);
            move |check| {
                if let Some(editor) = editor.upgrade() {
                    editor.draft.borrow_mut().multiselect = check.is_active();
                }
            }
        });
        schedule.connect_clicked({
            let (app, editor) = (Rc::downgrade(self), Rc::downgrade(&editor));
            move |_| {
                if let (Some(app), Some(editor)) = (app.upgrade(), editor.upgrade()) {
                    editor.pick_time(&app);
                }
            }
        });
        let submit = {
            let (app, editor, question) = (Rc::downgrade(self), Rc::downgrade(&editor), question.downgrade());
            move || {
                if let (Some(app), Some(editor), Some(question)) = (app.upgrade(), editor.upgrade(), question.upgrade())
                {
                    editor.submit(&app, &question);
                }
            }
        };
        post.connect_clicked({
            let submit = submit.clone();
            move |_| submit()
        });
        // Ctrl+Enter posts from anywhere in the editor.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                && !modifiers.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SHIFT_MASK);
            if ctrl && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
                submit();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        toolbar.add_controller(keys);
        dialog.connect_close_attempt({
            let editor = Rc::downgrade(&editor);
            move |dialog| {
                let Some(editor) = editor.upgrade() else { return dialog.force_close() };
                if editor.sending.get() {
                    return;
                }
                if editor.draft.borrow().is_blank() {
                    return dialog.force_close();
                }
                let alert = adw::AlertDialog::new(
                    Some("Are you sure?"),
                    Some("Are you sure you want to exit? Any changes made will be lost if you exit now."),
                );
                alert.add_response("back", "_Back to editing");
                alert.add_response("exit", "E_xit");
                alert.set_response_appearance("exit", adw::ResponseAppearance::Destructive);
                alert.set_default_response(Some("back"));
                alert.set_close_response("back");
                alert.connect_response(None, {
                    let dialog = dialog.clone();
                    move |_, response| {
                        if response == "exit" {
                            dialog.force_close();
                        }
                    }
                });
                alert.present(Some(dialog));
            }
        });
        // The editor lives as long as its dialog does.
        dialog.connect_closed(move |_| {
            let _ = &editor;
        });
        dialog.present(Some(&self.window));
        question.grab_focus();
    }
}
