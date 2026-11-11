//! The form that starts a post in a forum or media channel: a title, tags,
//! the first message and its files. The official client keeps it above the
//! post list; here it is a dialog the list's New Post button opens. What it
//! asks of Discord, and when it refuses, is in [`discord::post_creation`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::cooldowns::wait_words;
use discord::post_creation::{self, AUTOMOD_REASON, Draft, Problems, TAGS_MAX};
use discord::thread_creation::{self, NAME_MAX, Refused};
use discord::{ChannelKind, Snowflake};
use gtk::glib;

use super::state::now_ms;
use super::uploads::Stopped;
use super::{App, polls};

/// The form's widgets, and what it has picked.
pub(super) struct PostForm {
    forum: Snowflake,
    dialog: adw::Dialog,
    name: gtk::Entry,
    name_error: gtk::Label,
    chips: Vec<(Snowflake, gtk::ToggleButton)>,
    tags: RefCell<Vec<Snowflake>>,
    tag_error: gtk::Label,
    message: gtk::TextView,
    message_error: gtk::Label,
    /// The files' rows, which `uploads.rs` fills.
    files: gtk::Box,
    attach: Option<gtk::Button>,
    slowmode: gtk::Label,
    post: gtk::Button,
    /// While the files go up, which the message waits for.
    stop: gtk::Button,
    /// The request is out: nothing changes until it answers.
    submitting: Cell<bool>,
    /// The post was made, so what is in the form is no longer ours to keep.
    posted: Cell<bool>,
}

impl PostForm {
    fn text(&self) -> String {
        let buffer = self.message.buffer();
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned()
    }

    fn set_busy(&self, busy: bool) {
        self.submitting.set(busy);
        self.name.set_sensitive(!busy);
        self.message.set_sensitive(!busy);
        for (_, chip) in &self.chips {
            chip.set_sensitive(!busy);
        }
        if let Some(attach) = &self.attach {
            attach.set_sensitive(!busy);
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
    /// The forum whose post form is open and takes files now.
    pub(super) fn post_form_channel(&self) -> Option<Snowflake> {
        let forum = self.post_form.borrow().as_ref()?.forum;
        self.account.state.borrow().can_attach_to_post(forum).then_some(forum)
    }

    /// Show the open form's files as the draft is now.
    pub(super) fn refresh_post_files(self: &Rc<Self>) {
        let Some(form) = self.post_form.borrow().clone() else { return };
        let may_stop = self.fill_tray(&form.files, form.forum);
        form.stop.set_sensitive(may_stop);
        self.refresh_post_form(&form);
    }

    /// Open the form for a post in `forum`, where the client shows it.
    pub(super) fn open_post_form(self: &Rc<Self>, forum: Snowflake) {
        if self.post_form.borrow().is_some() || !self.account.state.borrow().can_post(forum) {
            return;
        }
        let form = Rc::new(self.build_post_form(forum));
        self.post_form.replace(Some(form.clone()));
        self.connect_post_form(&form);
        self.refresh_post_files();
        self.guard_post_form(&form);
        form.dialog.present(Some(&self.window));
        form.name.grab_focus();
    }

    fn build_post_form(self: &Rc<Self>, forum: Snowflake) -> PostForm {
        let (media, tags, attach) = {
            let state = self.account.state.borrow();
            let media = state.find_channel(forum).is_some_and(|(_, c)| c.kind == ChannelKind::Media);
            (media, state.post_tags(forum), state.can_attach_to_post(forum))
        };
        let name = gtk::Entry::builder().placeholder_text("Title").max_length(0).activates_default(false).build();
        // The limit is in UTF-16 units, and a title is a single line.
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

        let chips = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let wrap = adw::WrapBox::builder().child_spacing(6).line_spacing(6).build();
        let mut toggles = Vec::new();
        for tag in &tags {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            if let Some(emoji) = super::forum::tag_emoji(tag) {
                content.append(&polls::emoji_widget(&emoji, &self.images));
            }
            content.append(&gtk::Label::new(Some(&tag.name)));
            let chip = gtk::ToggleButton::builder().child(&content).css_classes(["pill"]).build();
            wrap.append(&chip);
            toggles.push((tag.id, chip));
        }
        chips.append(&wrap);

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
        let error = || gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["error"]).visible(false).build();
        let (name_error, message_error, tag_error) = (error(), error(), error());
        tag_error.set_label("A tag is required to create a post in this channel");
        let slowmode = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label"]).visible(false).build();
        let label = |text: &str| gtk::Label::builder().label(text).xalign(0.0).css_classes(["heading"]).build();

        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.append(&label("Title"));
        body.append(&name);
        body.append(&name_error);
        if !tags.is_empty() {
            body.append(&label("Tags"));
            body.append(&chips);
            body.append(&tag_error);
        }
        body.append(&label(if media { "Description" } else { "Message" }));
        body.append(&scroller);
        body.append(&message_error);
        let files = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let add = attach
            .then(|| gtk::Button::builder().label("Add Media").halign(gtk::Align::Start).css_classes(["flat"]).build());
        if media || add.is_some() {
            body.append(&label(if media { "Upload your media" } else { "Files" }));
        }
        if media {
            body.append(
                &gtk::Label::builder()
                    .label("Add any video, photo, and files to share with members.")
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        body.append(&files);
        if let Some(add) = &add {
            body.append(add);
        }
        body.append(&slowmode);

        let post = gtk::Button::builder().label("Post").css_classes(["suggested-action"]).build();
        let stop = gtk::Button::builder().label("Cancel").visible(false).build();
        let header = adw::HeaderBar::new();
        header.pack_end(&post);
        header.pack_end(&stop);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .child(&body)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .build(),
        ));
        let dialog =
            adw::Dialog::builder().title("Create Post").content_width(520).content_height(640).child(&toolbar).build();
        PostForm {
            forum,
            dialog,
            name,
            name_error,
            chips: toggles,
            tags: RefCell::default(),
            tag_error,
            message,
            message_error,
            files,
            attach: add,
            slowmode,
            post,
            stop,
            submitting: Cell::new(false),
            posted: Cell::new(false),
        }
    }

    /// What the form does on its own: the Post button follows what is
    /// filled in, a refusal is said until its field is changed, and Enter in
    /// the message posts, as in the message box.
    fn connect_post_form(self: &Rc<Self>, form: &Rc<PostForm>) {
        let changed = {
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            Rc::new(move || {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.refresh_post_form(&form);
                }
            })
        };
        form.name.connect_changed({
            let (changed, error) = (changed.clone(), form.name_error.downgrade());
            move |_| {
                if let Some(error) = error.upgrade() {
                    error.set_visible(false);
                }
                changed();
            }
        });
        form.message.buffer().connect_changed({
            let (changed, error) = (changed.clone(), form.message_error.downgrade());
            move |_| {
                if let Some(error) = error.upgrade() {
                    error.set_visible(false);
                }
                changed();
            }
        });
        for (id, chip) in &form.chips {
            chip.connect_clicked({
                let (changed, form, id) = (changed.clone(), Rc::downgrade(form), *id);
                move |_| {
                    if let Some(form) = form.upgrade() {
                        post_creation::toggle_tag(&mut form.tags.borrow_mut(), id);
                        changed();
                    }
                }
            });
        }
        form.name.connect_activate({
            let form = Rc::downgrade(form);
            move |_| {
                if let Some(form) = form.upgrade() {
                    form.message.grab_focus();
                }
            }
        });
        form.post.connect_clicked({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.submit_post_form(&form);
                }
            }
        });
        form.stop.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.cancel_sending();
                }
            }
        });
        if let Some(attach) = &form.attach {
            attach.connect_clicked({
                let (this, forum) = (Rc::downgrade(self), form.forum);
                move |_| {
                    if let Some(this) = this.upgrade() {
                        this.choose_post_files(forum);
                    }
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_, key, _, modifiers| {
                use gtk::gdk::{Key, ModifierType};
                let enter = matches!(key, Key::Return | Key::KP_Enter);
                let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) else {
                    return glib::Propagation::Proceed;
                };
                if !enter || modifiers.contains(ModifierType::SHIFT_MASK) {
                    return glib::Propagation::Proceed;
                }
                this.submit_post_form(&form);
                glib::Propagation::Stop
            }
        });
        form.message.add_controller(keys);
        // Files dropped on the form join the draft.
        let target = gtk::DropTarget::new(gtk::gdk::FileList::static_type(), gtk::gdk::DragAction::COPY);
        target.connect_drop({
            let (this, forum) = (Rc::downgrade(self), form.forum);
            move |_, value, _, _| {
                let (Some(this), Ok(list)) = (this.upgrade(), value.get::<gtk::gdk::FileList>()) else { return false };
                let files = list.files();
                glib::spawn_future_local(async move { this.add_paths(forum, files).await });
                true
            }
        });
        form.dialog.add_controller(target);
        // The wait slowmode asks counts down while the form is open.
        glib::timeout_add_local(Duration::from_secs(1), {
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move || match (this.upgrade(), form.upgrade()) {
                (Some(this), Some(form)) if this.post_form.borrow().as_ref().is_some_and(|f| Rc::ptr_eq(f, &form)) => {
                    this.refresh_post_form(&form);
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
        form.dialog.connect_closed({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.post_form_closed(&form);
                }
            }
        });
        self.refresh_post_form(form);
    }

    /// What the draft is, for [`post_creation`]'s checks.
    fn post_draft(&self, form: &PostForm, name: &str, text: &str) -> Problems {
        let state = self.account.state.borrow();
        let Some((_, forum)) = state.find_channel(form.forum) else { return Problems::default() };
        let draft = Draft { name, text, files: self.drafts.of(form.forum).len(), tags: form.tags.borrow().len() };
        Problems::of(forum, &draft)
    }

    /// Bring the Post button, the tags and the slowmode notice up to date.
    fn refresh_post_form(&self, form: &PostForm) {
        let (name, text) = (form.name.text().to_string(), form.text());
        let problems = self.post_draft(form, &name, &text);
        let wait = self.slowmode_left(form.forum);
        let postable = problems.none() && !name.trim().is_empty() && wait == 0;
        form.post.set_sensitive(postable && !form.submitting.get());
        form.tag_error.set_visible(problems.tag);
        let full = form.tags.borrow().len() >= TAGS_MAX;
        for (id, chip) in &form.chips {
            let picked = form.tags.borrow().contains(id);
            chip.set_active(picked);
            chip.set_sensitive(!form.submitting.get() && (picked || !full));
        }
        form.slowmode.set_visible(wait > 0);
        if wait > 0 {
            form.slowmode.set_label(&format!(
                "Slowmode is enabled. You can post again in this channel in {}.",
                wait_words((wait + 999) / 1000)
            ));
        }
    }

    fn choose_post_files(self: &Rc<Self>, forum: Snowflake) {
        // The client's picker takes one file at a time where slowmode is on (316576).
        let single = self.account.state.borrow().find_channel(forum).is_some_and(|(_, c)| c.rate_limit_per_user > 0);
        let dialog = gtk::FileDialog::builder().title("Upload a File").build();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let files = if single {
                dialog.open_future(Some(&this.window)).await.map(|file| vec![file])
            } else {
                dialog
                    .open_multiple_future(Some(&this.window))
                    .await
                    .map(|list| list.iter::<gtk::gio::File>().flatten().collect())
            };
            if let Ok(files) = files {
                this.add_paths(forum, files).await;
            }
        });
    }

    /// Closing a form with something in it asks first, as the client does
    /// for a thread's.
    fn guard_post_form(self: &Rc<Self>, form: &Rc<PostForm>) {
        let update = {
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move || {
                let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) else { return };
                let empty = form.name.text().is_empty()
                    && form.text().is_empty()
                    && form.tags.borrow().is_empty()
                    && this.drafts.of(form.forum).is_empty();
                form.dialog.set_can_close(empty && !form.submitting.get());
            }
        };
        let update = Rc::new(update);
        update();
        form.name.connect_changed({
            let update = update.clone();
            move |_| update()
        });
        form.message.buffer().connect_changed({
            let update = update.clone();
            move |_| update()
        });
        for (_, chip) in &form.chips {
            chip.connect_clicked({
                let update = update.clone();
                move |_| update()
            });
        }
        glib::timeout_add_local(Duration::from_millis(250), {
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move || match (this.upgrade(), form.upgrade()) {
                (Some(_), Some(form)) if !form.posted.get() && form.dialog.is_mapped() => {
                    update();
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
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

    /// The form is gone: the files it had, if it didn't post them, go too.
    fn post_form_closed(self: &Rc<Self>, form: &Rc<PostForm>) {
        if self.post_form.borrow().as_ref().is_some_and(|f| Rc::ptr_eq(f, form)) {
            self.post_form.replace(None);
        }
        if !form.posted.get() {
            self.drop_post_files(form.forum);
        }
    }

    fn submit_post_form(self: &Rc<Self>, form: &Rc<PostForm>) {
        if form.submitting.get() {
            return;
        }
        let name = thread_creation::single_line(&form.name.text(), true);
        let text = form.text();
        let problems = self.post_draft(form, &name, &text);
        form.show_errors(
            problems.name.then_some("A title is required"),
            problems.message.then_some("A message is required"),
        );
        // A tag that is missing is said all along; slowmode shows in the form.
        if !problems.none() || self.slowmode_left(form.forum) > 0 {
            return;
        }
        // `:name:`s go out as emoji, and the length is of what goes out.
        let library = self.emoji_library(form.forum);
        let translated = library.translate(&text);
        let text = translated.text;
        let files = self.drafts.of(form.forum);
        let (rest, forum, limits) = {
            let state = self.account.state.borrow();
            let found = state.find_channel(form.forum);
            let limits = state.limits().uploads(found.and_then(|(guild, _)| guild));
            (state.rest.clone(), found.map(|(_, c)| c.clone()), limits)
        };
        let (Some(rest), Some(forum_channel)) = (rest, forum) else { return };
        if let Err(too_long) = self.account.state.borrow().limits().check_length(&text) {
            let dialog = adw::AlertDialog::new(Some(too_long.heading()), Some(&too_long.body()));
            dialog.add_response("okay", "_Okay");
            return dialog.present(Some(&form.dialog));
        }
        if !files.is_empty()
            && let Err(refusal) = discord::uploads::check_send(&limits, &Self::sizes(&files))
        {
            return self.show_refusal(&refusal);
        }
        let tags = form.tags.borrow().clone();
        form.set_busy(true);
        form.stop.set_visible(!files.is_empty());
        form.show_errors(None, None);
        self.refresh_post_form(form);
        let (this, form) = (self.clone(), form.clone());
        glib::spawn_future_local(async move {
            let make = |attachments| post_creation::body(&forum_channel, &name, &tags, &text, attachments);
            let post = match this.post_with_uploads(&rest, form.forum, files, make).await {
                Ok(post) => post,
                Err(stopped) => {
                    form.set_busy(false);
                    this.refresh_post_form(&form);
                    return this.post_stopped(&form, &stopped);
                }
            };
            // Discord's wait starts when the post is made.
            this.record_slowmode(form.forum, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
            form.posted.set(true);
            // The client waits for the gateway to bring the post, which is
            // what opens it. Past the wait we give it, the post is Discord's
            // and the form has no more to do with it.
            let arrived = this.thread_arrived(post.id).await;
            form.dialog.force_close();
            if arrived {
                // The list shows the post when the forum is next open, as the
                // client resorts it before it opens the post (`RESORT_THREADS`).
                this.forum_resort_after_post(form.forum);
                if this.account.state.borrow().channel == Some(form.forum) {
                    this.select_channel(post.id);
                }
            }
        });
    }

    /// Say why a post wasn't made, as the client does: an alert for most,
    /// next to its field for AutoMod and the title.
    fn post_stopped(self: &Rc<Self>, form: &PostForm, stopped: &Stopped) {
        let alert = |heading: &str, body: &str| {
            let dialog = adw::AlertDialog::new(Some(heading), Some(body));
            dialog.add_response("okay", "_Okay");
            dialog.present(Some(&form.dialog));
        };
        let error = match stopped {
            Stopped::Canceled => return,
            Stopped::Upload(why) => return alert("Upload Failed", why),
            Stopped::Send(error) => error,
        };
        const CANNOT: &str = "Cannot create post";
        match Refused::from(error) {
            Refused::TooManyThreads => alert(
                CANNOT,
                "This server has reached the maximum number of active posts. Free up space by archiving a post.",
            ),
            Refused::TooManyAnnouncementThreads => alert(
                "Cannot create thread",
                "This server has reached the maximum number of active announcement threads. Free up space by archiving a thread.",
            ),
            // The client records the wait and says nothing.
            Refused::Slowmode(_) => {
                let wait = error.slowmode_wait().unwrap_or_default();
                self.record_slowmode(form.forum, |slowmode, c, _| slowmode.refused(c, wait, now_ms()));
            }
            Refused::RateLimited => alert(CANNOT, "You are being rate limited."),
            Refused::AutoMod { title, reason } => {
                let reason = reason.unwrap_or_else(|| AUTOMOD_REASON.to_owned());
                match title {
                    true => form.show_errors(Some(&reason), None),
                    false => form.show_errors(None, Some(&reason)),
                }
            }
            Refused::Name => form.show_errors(Some("A title is required"), None),
            Refused::Other => match error {
                discord::Error::Http { .. } => alert("Error", "An error occurred, please try again."),
                other => alert("Error", &other.to_string()),
            },
        }
    }

    /// Show the list as the client does once its post exists.
    fn forum_resort_after_post(self: &Rc<Self>, forum: Snowflake) {
        if self.forum_shown() == Some(forum) {
            self.forum_resort();
        }
    }
}
