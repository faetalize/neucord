//! Attaching files: what the attach menu, a drop and the clipboard add to a
//! draft, the tray that shows it above the message box, and sending it. The
//! protocol is `discord::uploads`; as in the official client, an upload
//! starts when its file is added, not when the message is sent.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use discord::images;
use discord::uploads::{self, Data, File, Refusal, Sent};
use discord::{Rest, Snowflake};
use gtk::{gdk, gio, glib};

use super::App;
use super::menus::{Entry, Item, Kind};

/// Upload a File, in the attach menu.
pub(super) const UPLOAD_FILE: Entry<()> = Entry::Item(Item {
    name: "upload-file",
    label: "_Upload a File",
    shows: |app, _| app.attach_channel().is_some(),
    kind: Kind::Action(|app, _| app.choose_files()),
});

/// Shown only with something in the box, as in the client.
pub(super) const UPLOAD_TEXT: Entry<()> = Entry::Item(Item {
    name: "upload-text",
    label: "Upload your _message as a file",
    shows: |app, _| app.attach_channel().is_some() && app.composer.buffer().char_count() > 0,
    kind: Kind::Action(|app, _| app.upload_composer_text()),
});

/// Where an upload stands.
#[derive(Clone)]
enum Status {
    /// With the bytes sent so far.
    Uploading(u64),
    /// With the words for it, and whether sending the message tries again.
    Failed(String, bool),
    /// With the name Discord keeps the file by.
    Done(String),
    Canceled,
}

/// A file in a channel's draft, and the upload of it.
pub(super) struct Upload {
    channel: Snowflake,
    /// What goes up: the file as added, or what it was converted to.
    file: RefCell<File>,
    /// Whether the WebP conversion has had its turn, which the client gives
    /// a file once as well (its result replaces the file).
    optimized: Cell<bool>,
    /// What the message calls it, which the edit dialog may change.
    name: RefCell<String>,
    spoiler: Cell<bool>,
    description: RefCell<Option<String>>,
    status: RefCell<Status>,
    /// How many times it has been tried, which a send doesn't reset.
    attempts: Rc<Cell<u32>>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    waiting: RefCell<Vec<async_channel::Sender<()>>>,
    row: Row,
}

struct Row {
    root: gtk::Box,
    name: gtk::Label,
    state: gtk::Label,
    spoiler: gtk::ToggleButton,
    edit: gtk::Button,
    remove: gtk::Button,
}

impl Upload {
    fn set_status(&self, status: Status) {
        let size = self.file.borrow().size;
        let words = match &status {
            Status::Uploading(sent) if size > 0 => format!("{}%", (sent * 100 / size).min(100)),
            Status::Uploading(_) => String::new(),
            Status::Done(_) => "Uploaded".to_owned(),
            Status::Failed(why, _) => why.clone(),
            Status::Canceled => "Canceled".to_owned(),
        };
        let failed = matches!(status, Status::Failed(..));
        let finished = !matches!(status, Status::Uploading(_));
        let size = uploads::size_words(size);
        let shown = if words.is_empty() { size } else { format!("{size} · {words}") };
        // Progress comes in small steps; only a change of the words is drawn.
        if self.row.state.label() != shown {
            self.row.state.set_label(&shown);
        }
        if failed {
            self.row.state.add_css_class("error");
        } else {
            self.row.state.remove_css_class("error");
        }
        self.status.replace(status);
        if finished {
            for waiter in self.waiting.take() {
                let _ = waiter.try_send(());
            }
        }
    }

    /// Wait until the upload has ended, and say with what.
    async fn ended(&self) -> Result<String, String> {
        loop {
            match &*self.status.borrow() {
                Status::Done(uploaded) => return Ok(uploaded.clone()),
                Status::Failed(why, _) => return Err(why.clone()),
                Status::Canceled => return Err(String::new()),
                Status::Uploading(_) => {}
            }
            let (tx, rx) = async_channel::bounded(1);
            self.waiting.borrow_mut().push(tx);
            let _ = rx.recv().await;
        }
    }

    /// Stop the upload, and take back what it has made: Discord holds a
    /// finished upload until a message names it, and the client says it's
    /// no longer wanted.
    fn cancel(&self, rest: Option<Rest>) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let before = self.status.borrow().clone();
        if let (Status::Done(uploaded), Some(rest)) = (before, rest) {
            glib::spawn_future_local(async move { uploads::delete(&rest, &uploaded).await });
        }
        self.set_status(Status::Canceled);
    }

    fn lock(&self, locked: bool) {
        let row = &self.row;
        for button in [&row.edit, &row.remove] {
            button.set_sensitive(!locked);
        }
        row.spoiler.set_sensitive(!locked);
    }
}

/// A message waiting on its files.
struct Sending {
    /// The files it takes. Others may be added meanwhile, for the next one.
    files: Vec<Rc<Upload>>,
    /// Whether the message itself has gone out, after which there is no
    /// taking the files back.
    posting: Cell<bool>,
}

impl Sending {
    fn has(&self, upload: &Rc<Upload>) -> bool {
        self.files.iter().any(|u| Rc::ptr_eq(u, upload))
    }
}

/// What the message box has attached, by channel, and the tray showing it.
pub(super) struct Drafts {
    files: RefCell<HashMap<Snowflake, Vec<Rc<Upload>>>>,
    /// Channels with a message waiting on its files.
    sending: RefCell<HashMap<Snowflake, Rc<Sending>>>,
    tray: gtk::Box,
    attach: gtk::Button,
    /// The tray's top row while sending.
    sending_bar: gtk::Box,
    cancel: gtk::Button,
}

impl Drafts {
    pub(super) fn new() -> Self {
        let sending_bar = gtk::Box::builder().spacing(8).margin_start(12).margin_top(4).build();
        sending_bar.append(&gtk::Label::builder().label("Sending…").xalign(0.0).hexpand(true).build());
        let cancel = gtk::Button::builder().label("Cancel").css_classes(["flat"]).build();
        sending_bar.append(&cancel);
        Drafts {
            files: RefCell::default(),
            sending: RefCell::default(),
            tray: gtk::Box::builder().orientation(gtk::Orientation::Vertical).visible(false).build(),
            attach: gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("Attach")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build(),
            sending_bar,
            cancel,
        }
    }

    pub(super) fn of(&self, channel: Snowflake) -> Vec<Rc<Upload>> {
        self.files.borrow().get(&channel).cloned().unwrap_or_default()
    }

    /// What a new message would take: not the files on their way in another.
    fn for_new_message(&self, channel: Snowflake) -> Vec<Rc<Upload>> {
        let sending = self.sending.borrow().get(&channel).cloned();
        let mut files = self.of(channel);
        files.retain(|u| sending.as_ref().is_none_or(|s| !s.has(u)));
        files
    }
}

/// Why a message with files didn't go out.
pub(super) enum Stopped {
    /// The user took the files back.
    Canceled,
    Upload(String),
    Send(discord::Error),
}

/// What a message carries besides its files.
pub(super) struct Outgoing<'a> {
    pub(super) text: &'a str,
    pub(super) reply: Option<discord::rest::Reply>,
    pub(super) sticker: Option<Snowflake>,
    /// An ISO 8601 time, for a message Discord posts then, not now.
    pub(super) at: Option<&'a str>,
}

/// Post the message with `sent`, uploaded already, or have Discord post it later.
async fn post(rest: &Rest, channel: Snowflake, message: &Outgoing<'_>, sent: &[Sent<'_>]) -> Result<(), Stopped> {
    let Outgoing { text, reply, sticker, at } = *message;
    match at {
        // The gateway echoes a message back, which is what adds it to the list; a scheduled one isn't posted yet.
        None => rest.send_message(channel, text, reply, sent, sticker).await.map(drop),
        Some(at) => rest.schedule_message(channel, at, text, reply, sent, sticker, None).await,
    }
    .map_err(Stopped::Send)
}

impl App {
    /// The open channel, if files may be attached to it now: the message box
    /// shows, no edit has it, and the client would offer attaching there.
    pub(super) fn attach_channel(&self) -> Option<Snowflake> {
        let channel = self.state.borrow().channel?;
        let allowed = self.composer_box.is_visible()
            && self.editing().is_none()
            && !self.has_voice_draft(channel)
            && self.state.borrow().can_attach(channel);
        allowed.then_some(channel)
    }

    /// Show the attach button and the tray as they are now for the open
    /// channel.
    pub(super) fn refresh_attach(self: &Rc<Self>) {
        let drafts = &self.drafts;
        let channel = self.attach_channel();
        // The menu holds more than files: it shows wherever anything in it does.
        drafts.attach.set_visible(
            channel.is_some()
                || self.voice_channel().is_some()
                || self.poll_channel().is_some()
                || self.schedule_channel().is_some(),
        );
        self.refresh_schedule_bar();
        while let Some(child) = drafts.tray.first_child() {
            drafts.tray.remove(&child);
        }
        let files = channel.map(|c| drafts.of(c)).unwrap_or_default();
        let sending = channel.and_then(|c| drafts.sending.borrow().get(&c).cloned());
        if let Some(sending) = &sending {
            drafts.tray.append(&drafts.sending_bar);
            drafts.cancel.set_sensitive(!sending.posting.get());
        }
        for upload in &files {
            upload.lock(sending.as_ref().is_some_and(|s| s.has(upload)));
            drafts.tray.append(&upload.row.root);
        }
        drafts.tray.set_visible(!files.is_empty());
        self.refresh_stickers();
        self.refresh_voice();
    }

    pub(super) fn build_attach(self: &Rc<Self>, area: &gtk::Box) {
        let drafts = &self.drafts;
        area.append(&drafts.tray);
        self.composer_box.append(&drafts.attach);
        drafts.attach.connect_clicked({
            let this = Rc::downgrade(self);
            move |button| {
                let Some(this) = this.upgrade() else { return };
                let Some(bounds) = button.compute_bounds(&this.composer_box) else { return };
                let rect = gdk::Rectangle::new(
                    bounds.x() as i32,
                    bounds.y() as i32,
                    bounds.width() as i32,
                    bounds.height() as i32,
                );
                super::menus::popup(&this, super::menus::ATTACH, (), &this.composer_box, super::menus::At::Below(rect));
            }
        });
        drafts.cancel.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.cancel_sending();
                }
            }
        });
        // A pasted image or file list becomes a file; text pastes as ever.
        self.composer.connect_paste_clipboard({
            let this = Rc::downgrade(self);
            move |view| {
                let Some(this) = this.upgrade() else { return };
                if this.attach_channel().is_none() {
                    return;
                }
                let formats = view.clipboard().formats();
                let files = formats.contains_type(gdk::FileList::static_type());
                let image =
                    !formats.contains_type(glib::Type::STRING) && formats.contains_type(gdk::Texture::static_type());
                if files || image {
                    view.stop_signal_emission_by_name("paste-clipboard");
                    this.paste_files(files);
                }
            }
        });
    }

    /// Files dropped anywhere on the chat.
    pub(super) fn drop_target(self: &Rc<Self>) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_accept({
            let this = Rc::downgrade(self);
            move |_, _| this.upgrade().is_some_and(|this| this.attach_channel().is_some())
        });
        target.connect_drop({
            let this = Rc::downgrade(self);
            move |_, value, _, _| {
                let (Some(this), Ok(list)) = (this.upgrade(), value.get::<gdk::FileList>()) else { return false };
                let Some(channel) = this.attach_channel() else { return false };
                let files = list.files();
                glib::spawn_future_local(async move { this.add_paths(channel, files).await });
                true
            }
        });
        target
    }

    /// Ask for files to attach. The client's picker takes several unless the
    /// channel has slowmode (316576).
    pub(super) fn choose_files(self: &Rc<Self>) {
        let Some(channel) = self.attach_channel() else { return };
        let single = self.state.borrow().find_channel(channel).is_some_and(|(_, c)| c.rate_limit_per_user > 0);
        let dialog = gtk::FileDialog::builder().title("Upload a File").build();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let files = if single {
                dialog.open_future(Some(&this.window)).await.map(|file| vec![file])
            } else {
                dialog
                    .open_multiple_future(Some(&this.window))
                    .await
                    .map(|list| list.iter::<gio::File>().flatten().collect())
            };
            if let Ok(files) = files {
                this.add_paths(channel, files).await;
            }
            this.composer.grab_focus();
        });
    }

    fn paste_files(self: &Rc<Self>, files: bool) {
        let Some(channel) = self.attach_channel() else { return };
        let clipboard = self.composer.clipboard();
        let this = self.clone();
        glib::spawn_future_local(async move {
            if files {
                let Ok(value) =
                    clipboard.read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT).await
                else {
                    return;
                };
                if let Ok(list) = value.get::<gdk::FileList>() {
                    this.add_paths(channel, list.files()).await;
                }
            } else if let Ok(Some(texture)) = clipboard.read_texture_future().await {
                let bytes = texture.save_to_png_bytes();
                let image = File {
                    filename: "image.png".to_owned(),
                    size: bytes.len() as u64,
                    content_type: "image/png".to_owned(),
                    data: Data::Memory(bytes),
                    original_md5: None,
                };
                this.add_files(channel, vec![image]).await;
            }
        });
    }

    /// Add what `paths` hold.
    async fn add_paths(self: &Rc<Self>, channel: Snowflake, paths: Vec<gio::File>) {
        let mut files = Vec::new();
        for path in paths {
            let attributes = "standard::name,standard::size,standard::type";
            let info = path.query_info_future(attributes, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await;
            let Ok(info) = info else { continue };
            let name = info.name().to_string_lossy().into_owned();
            // A folder is no file; like an empty one, there is nothing to send.
            let size = if info.file_type() == gio::FileType::Regular { info.size().max(0) as u64 } else { 0 };
            files.push(File {
                content_type: uploads::content_type(&name),
                filename: name,
                size,
                data: Data::File(path),
                original_md5: None,
            });
        }
        self.add_files(channel, files).await;
    }

    /// Add files to `channel`'s draft as the client's `R` does (518960): an
    /// empty file is turned back first, then HEIC and JPEG XR files become
    /// JPEGs (all at once, unseen), and only then are the limits checked, on
    /// what will be uploaded.
    async fn add_files(self: &Rc<Self>, channel: Snowflake, files: Vec<File>) {
        if files.is_empty() || self.attach_channel() != Some(channel) {
            return;
        }
        if files.iter().any(|f| f.size == 0) {
            return self.show_refusal(&Refusal::Empty);
        }
        let tasks: Vec<_> = files.into_iter().map(|file| glib::spawn_future_local(images::prepare(file))).collect();
        let mut prepared = Vec::new();
        for task in tasks {
            prepared.extend(task.await);
        }
        self.add_prepared(channel, prepared);
    }

    /// Put files in `channel`'s draft and start uploading them, unless
    /// they're over what a message may carry, in which case none are added.
    fn add_prepared(self: &Rc<Self>, channel: Snowflake, files: Vec<File>) {
        if files.is_empty() || self.attach_channel() != Some(channel) {
            return;
        }
        let limits = {
            let state = self.state.borrow();
            state.limits().uploads(state.find_channel(channel).and_then(|(guild, _)| guild))
        };
        let queued: Vec<u64> = self.drafts.of(channel).iter().map(|u| u.file.borrow().size).collect();
        let added: Vec<u64> = files.iter().map(|f| f.size).collect();
        if let Err(refusal) = uploads::check_added(&limits, &queued, &added) {
            return self.show_refusal(&refusal);
        }
        for file in files {
            let upload = self.new_upload(channel, file);
            self.start_upload(&upload);
            self.drafts.files.borrow_mut().entry(channel).or_default().push(upload);
        }
        self.refresh_attach();
    }

    pub(super) fn show_refusal(self: &Rc<Self>, refusal: &Refusal) {
        let dialog = adw::AlertDialog::new(Some(refusal.heading()), Some(&refusal.body()));
        dialog.add_response("okay", "_Okay");
        let this = self.clone();
        glib::spawn_future_local(async move {
            dialog.choose_future(Some(&this.window)).await;
            this.composer.grab_focus();
        });
    }

    fn new_upload(self: &Rc<Self>, channel: Snowflake, file: File) -> Rc<Upload> {
        let root = gtk::Box::builder().spacing(8).margin_start(12).margin_top(4).margin_bottom(4).build();
        let name = gtk::Label::builder()
            .label(&file.filename)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        let state = gtk::Label::builder().css_classes(["dim-label"]).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let spoiler = gtk::ToggleButton::builder()
            .icon_name("view-conceal-symbolic")
            .tooltip_text("Mark as spoiler")
            .css_classes(["flat"])
            .build();
        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Edit")
            .css_classes(["flat"])
            .build();
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove")
            .css_classes(["flat"])
            .build();
        for widget in [name.upcast_ref::<gtk::Widget>(), state.upcast_ref(), spoiler.upcast_ref(), edit.upcast_ref()] {
            root.append(widget);
        }
        root.append(&remove);
        let upload = Rc::new(Upload {
            channel,
            name: RefCell::new(file.filename.clone()),
            file: RefCell::new(file),
            optimized: Cell::new(false),
            spoiler: Cell::new(false),
            description: RefCell::default(),
            status: RefCell::new(Status::Uploading(0)),
            attempts: Rc::default(),
            task: RefCell::default(),
            waiting: RefCell::default(),
            row: Row { root, name, state, spoiler, edit, remove },
        });
        upload.set_status(Status::Uploading(0));
        upload.row.spoiler.connect_toggled({
            let upload = Rc::downgrade(&upload);
            move |button| {
                if let Some(upload) = upload.upgrade() {
                    upload.spoiler.set(button.is_active());
                }
            }
        });
        upload.row.remove.connect_clicked({
            let (this, upload) = (Rc::downgrade(self), Rc::downgrade(&upload));
            move |_| {
                if let (Some(this), Some(upload)) = (this.upgrade(), upload.upgrade()) {
                    this.remove_upload(channel, &upload);
                }
            }
        });
        upload.row.edit.connect_clicked({
            let (this, upload) = (Rc::downgrade(self), Rc::downgrade(&upload));
            move |_| {
                if let (Some(this), Some(upload)) = (this.upgrade(), upload.upgrade()) {
                    this.edit_upload(&upload);
                }
            }
        });
        upload
    }

    /// Start (or start again) uploading, as the client does when a file is
    /// added and, for one that failed, when the message is sent.
    fn start_upload(self: &Rc<Self>, upload: &Rc<Upload>) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let channel = upload.channel;
        upload.set_status(Status::Uploading(0));
        let weak = Rc::downgrade(upload);
        let attempts = upload.attempts.clone();
        // Decided here, from the account's experiments, for the file's turn.
        let webp = self.state.borrow().limits().webp_conversion();
        let task = glib::spawn_future_local(async move {
            let Some(upload) = weak.upgrade() else { return };
            let file = upload.file.borrow().clone();
            if let (false, Some(max)) = (upload.optimized.replace(true), webp)
                && let Some(smaller) = images::optimize(&file, max).await
            {
                upload.file.replace(smaller);
                upload.set_status(Status::Uploading(0));
            }
            let file = upload.file.borrow().clone();
            drop(upload);
            let progress: Rc<dyn Fn(u64)> = Rc::new({
                let weak = weak.clone();
                move |sent| {
                    if let Some(upload) = weak.upgrade() {
                        upload.set_status(Status::Uploading(sent));
                    }
                }
            });
            let result = uploads::upload(&rest, channel, &file, &attempts, progress).await;
            if let Some(upload) = weak.upgrade() {
                upload.task.take();
                upload.set_status(match result {
                    Ok(uploaded) => Status::Done(uploaded),
                    Err(failure) => Status::Failed(failure.words(), failure.restarts()),
                });
            }
        });
        upload.task.replace(Some(task));
    }

    /// Take a file out of the draft: its upload stops, or is deleted.
    fn remove_upload(self: &Rc<Self>, channel: Snowflake, upload: &Rc<Upload>) {
        upload.cancel(self.state.borrow().rest.clone());
        if let Some(files) = self.drafts.files.borrow_mut().get_mut(&channel) {
            files.retain(|u| !Rc::ptr_eq(u, upload));
        }
        self.refresh_attach();
    }

    /// Change what the message calls a file, and its alt text, which only
    /// images and videos take (507536).
    fn edit_upload(self: &Rc<Self>, upload: &Rc<Upload>) {
        let name = gtk::Entry::builder().text(upload.name.borrow().as_str()).build();
        let description = gtk::Entry::builder()
            .placeholder_text("Alt text")
            .text(upload.description.borrow().as_deref().unwrap_or_default())
            .max_length(1024)
            .build();
        let fields = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        fields.append(&name);
        let alt = uploads::is_image(&upload.file.borrow().content_type)
            || uploads::is_video(&upload.file.borrow().content_type);
        if alt {
            fields.append(&description);
        }
        let dialog = adw::AlertDialog::new(Some("Edit Attachment"), None);
        dialog.set_extra_child(Some(&fields));
        dialog.add_response("cancel", "_Cancel");
        dialog.add_response("save", "_Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        let (this, upload) = (self.clone(), upload.clone());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "save" {
                // A file with no name would be turned down.
                if !name.text().is_empty() {
                    upload.name.replace(name.text().into());
                    upload.row.name.set_label(&name.text());
                }
                // The client's dialog hands over its alt text for any file,
                // empty when the field isn't there (507536).
                upload.description.replace(Some(description.text().into()));
            }
            this.composer.grab_focus();
        });
    }

    /// Upload Text as File: the box's text, or the code in it, becomes a file.
    pub(super) fn upload_composer_text(self: &Rc<Self>) {
        let Some(channel) = self.attach_channel() else { return };
        let buffer = self.composer.buffer();
        let text: String = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).into();
        if text.is_empty() {
            return;
        }
        let split = uploads::text_file(&text);
        let bytes = glib::Bytes::from_owned(split.content.as_bytes().to_vec());
        let file = File {
            filename: split.filename,
            size: bytes.len() as u64,
            content_type: "text/plain".to_owned(),
            data: Data::Memory(bytes),
            original_md5: None,
        };
        let before = self.drafts.of(channel).len();
        // Text is never converted.
        self.add_prepared(channel, vec![file]);
        // The text leaves the box only once it has become a file.
        if self.drafts.of(channel).len() > before {
            self.set_composer_text(&split.rest);
        }
    }

    /// Send `text` with the draft's files, once they are all uploaded: the
    /// message waits for them, and goes out at the end.
    pub(super) async fn send_with_uploads(
        self: &Rc<Self>,
        rest: &Rest,
        channel: Snowflake,
        message: &Outgoing<'_>,
        files: Vec<Rc<Upload>>,
    ) -> Result<(), Stopped> {
        if files.is_empty() {
            return post(rest, channel, message, &[]).await;
        }
        let sending = Rc::new(Sending { files: files.clone(), posting: Cell::new(false) });
        self.drafts.sending.borrow_mut().insert(channel, sending.clone());
        self.refresh_attach();
        let result = self.upload_and_post(rest, channel, message, &sending).await;
        self.drafts.sending.borrow_mut().remove(&channel);
        if result.is_ok() {
            self.forget(channel, &sending);
        }
        self.refresh_attach();
        result
    }

    async fn upload_and_post(
        self: &Rc<Self>,
        rest: &Rest,
        channel: Snowflake,
        message: &Outgoing<'_>,
        sending: &Sending,
    ) -> Result<(), Stopped> {
        let files = &sending.files;
        for upload in files {
            let failed = match &*upload.status.borrow() {
                Status::Failed(why, restarts) => Some((why.clone(), *restarts)),
                _ => None,
            };
            match failed {
                Some((why, false)) => return Err(Stopped::Upload(format!("{}: {why}", upload.name.borrow()))),
                Some((_, true)) => self.start_upload(upload),
                None => {}
            }
        }
        let mut uploaded = Vec::new();
        for upload in files {
            match upload.ended().await {
                Ok(name) => uploaded.push(name),
                Err(why) if why.is_empty() => return Err(Stopped::Canceled),
                Err(why) => return Err(Stopped::Upload(format!("{}: {why}", upload.name.borrow()))),
            }
        }
        sending.posting.set(true);
        self.refresh_attach();
        let (names, descriptions): (Vec<String>, Vec<Option<String>>) =
            files.iter().map(|u| (u.name.borrow().clone(), u.description.borrow().clone())).unzip();
        let types: Vec<String> = files.iter().map(|u| u.file.borrow().content_type.clone()).collect();
        let sent: Vec<Sent> = files
            .iter()
            .enumerate()
            .map(|(i, u)| Sent {
                filename: &names[i],
                uploaded_filename: &uploaded[i],
                description: descriptions[i].as_deref(),
                spoiler: u.spoiler.get(),
                content_type: &types[i],
                voice: None,
            })
            .collect();
        post(rest, channel, message, &sent).await
    }

    /// Cancel a send that is still uploading: the files are taken back, and
    /// the uploads that finished are deleted.
    fn cancel_sending(self: &Rc<Self>) {
        let Some(channel) = self.state.borrow().channel else { return };
        let sending = self.drafts.sending.borrow().get(&channel).cloned();
        let Some(sending) = sending.filter(|s| !s.posting.get()) else { return };
        let rest = self.state.borrow().rest.clone();
        for upload in &sending.files {
            upload.cancel(rest.clone());
        }
        self.forget(channel, &sending);
        self.refresh_attach();
    }

    /// Drop the files a message took from the draft, and keep any added
    /// since.
    fn forget(&self, channel: Snowflake, sending: &Sending) {
        if let Some(files) = self.drafts.files.borrow_mut().get_mut(&channel) {
            files.retain(|u| !sending.has(u));
        }
    }

    /// Drop every draft without telling Discord, once the token that made the
    /// uploads is gone.
    pub(super) fn discard_drafts(&self) {
        for upload in self.drafts.files.borrow().values().flatten() {
            upload.cancel(None);
        }
        self.drafts.files.borrow_mut().clear();
        self.drafts.sending.borrow_mut().clear();
        self.scheduling.clear();
        self.drop_voice();
    }

    /// What a message sent now from `channel` takes along: nothing where the
    /// client wouldn't offer attaching, which also hides the draft.
    pub(super) fn files_for_message(&self, channel: Snowflake) -> Vec<Rc<Upload>> {
        if !self.state.borrow().can_attach(channel) {
            return Vec::new();
        }
        self.drafts.for_new_message(channel)
    }

    /// The sizes of `files`, for the checks before sending.
    pub(super) fn sizes(files: &[Rc<Upload>]) -> Vec<u64> {
        files.iter().map(|u| u.file.borrow().size).collect()
    }
}
