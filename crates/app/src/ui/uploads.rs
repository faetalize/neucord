//! Attaching files: what the attach menu, a drop and the clipboard add to a
//! draft, the tray that shows it above the message box, and sending it. The
//! protocol is `discord::uploads`; as in the official client, an upload
//! starts when its file is added, not when the message is sent.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use discord::images;
use discord::uploads::{self, Data, File, Pasted, Refusal, Sent, pasted};
use discord::{Channel, Rest, Snowflake};
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

/// The message box's menu: the clipboard's text alone, where Paste would
/// attach what also holds text (a file manager's copy). This stands for
/// the desktop's `clipboardHasMixedContent` (194400), which no web build
/// defines.
pub(super) const PASTE_PLAIN: Entry<()> = Entry::Item(Item {
    name: "paste-plain-text",
    label: "Paste as P_lain Text",
    shows: |app, _| app.composer_paste().is_some_and(|(pasted, text)| text && pasted != Pasted::Text),
    kind: Kind::Action(|app, _| app.paste_plain_text()),
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
    /// A message's files only: a command's or a form's have neither
    /// (`canEdit: !1` in 914905).
    spoiler: Option<gtk::ToggleButton>,
    edit: Option<gtk::Button>,
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
    pub(super) async fn ended(&self) -> Result<String, String> {
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
    pub(super) fn cancel(&self, rest: Option<Rest>) {
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
        for button in row.edit.iter().chain([&row.remove]) {
            button.set_sensitive(!locked);
        }
        if let Some(spoiler) = &row.spoiler {
            spoiler.set_sensitive(!locked);
        }
    }

    /// Its row, to put where it shows.
    pub(super) fn row(&self) -> &gtk::Box {
        &self.row.root
    }
}

/// Wait for every one of `files` to be uploaded, and say what Discord keeps
/// each by, or fail as soon as one has failed or been taken back, without
/// trying any again: 358579's `A` without its second argument, under a
/// `Promise.all`. The others go on.
pub(super) async fn staged(files: &[Rc<Upload>]) -> Result<Vec<String>, ()> {
    loop {
        let mut uploaded = Vec::new();
        for upload in files {
            match &*upload.status.borrow() {
                Status::Done(name) => uploaded.push(name.clone()),
                Status::Failed(..) | Status::Canceled => return Err(()),
                Status::Uploading(_) => {}
            }
        }
        if uploaded.len() == files.len() {
            return Ok(uploaded);
        }
        let (tx, rx) = async_channel::bounded(files.len());
        for upload in files.iter().filter(|u| matches!(*u.status.borrow(), Status::Uploading(_))) {
            upload.waiting.borrow_mut().push(tx.clone());
        }
        drop(tx);
        let _ = rx.recv().await;
    }
}

/// The file at `path`, as a browser's `File` would give it: its name, size
/// and the type its name says. A folder is no file: like an empty one, it
/// has nothing to send.
pub(super) async fn file_at(path: gio::File) -> Option<File> {
    let attributes = "standard::name,standard::size,standard::type";
    let info = path.query_info_future(attributes, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await.ok()?;
    let name = info.name().to_string_lossy().into_owned();
    let size = if info.file_type() == gio::FileType::Regular { info.size().max(0) as u64 } else { 0 };
    Some(File {
        content_type: uploads::content_type(&name),
        filename: name,
        size,
        data: Data::File(path),
        original_md5: None,
    })
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

/// A file added to an app's form: its channel's `InteractionModal` draft
/// in the client, under the form's `custom_id` (257249 `R`).
pub(super) struct FormUpload {
    pub(super) channel: Snowflake,
    pub(super) custom_id: String,
    /// What the field's state names it by.
    pub(super) id: String,
    pub(super) upload: Rc<Upload>,
}

/// What the message box has attached, by channel, and the tray showing it.
pub(super) struct Drafts {
    files: RefCell<HashMap<Snowflake, Vec<Rc<Upload>>>>,
    /// Forms' files, kept across closing and reopening until a submit
    /// succeeds, in the order they were added.
    pub(super) forms: RefCell<Vec<FormUpload>>,
    /// Channels with a message waiting on its files.
    sending: RefCell<HashMap<Snowflake, Rc<Sending>>>,
    tray: gtk::Box,
    attach: gtk::Button,
    /// The same button on the notice that takes the message box's place,
    /// where only Create Thread is left in its menu.
    pub(super) notice_attach: gtk::Button,
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
        let attach_button = || {
            gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("Attach")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build()
        };
        Drafts {
            files: RefCell::default(),
            forms: RefCell::default(),
            sending: RefCell::default(),
            tray: gtk::Box::builder().orientation(gtk::Orientation::Vertical).visible(false).build(),
            attach: attach_button(),
            notice_attach: attach_button(),
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
    /// Read aloud (`/tts`). A scheduled message carries no `tts`.
    pub(super) tts: bool,
}

/// Post the message with `sent`, uploaded already, or have Discord post it later.
async fn post(rest: &Rest, channel: Snowflake, message: &Outgoing<'_>, sent: &[Sent<'_>]) -> Result<(), Stopped> {
    let Outgoing { text, reply, sticker, at, tts } = *message;
    match at {
        // The gateway echoes a message back, which is what adds it to the list; a scheduled one isn't posted yet.
        None => rest.send_message(channel, text, reply, sent, sticker, tts).await.map(drop),
        Some(at) => rest.schedule_message(channel, at, text, reply, sent, sticker, None).await,
    }
    .map_err(Stopped::Send)
}

/// What a message calls its files, which the edit dialog may have changed:
/// [`Sent`] borrows it.
pub(super) struct Details {
    names: Vec<String>,
    descriptions: Vec<Option<String>>,
    types: Vec<String>,
}

impl Details {
    pub(super) fn of(files: &[Rc<Upload>]) -> Self {
        let (names, descriptions) =
            files.iter().map(|u| (u.name.borrow().clone(), u.description.borrow().clone())).unzip();
        let types = files.iter().map(|u| u.file.borrow().content_type.clone()).collect();
        Details { names, descriptions, types }
    }

    /// `files` as the message names them, by what Discord keeps each as.
    pub(super) fn sent<'a>(&'a self, files: &[Rc<Upload>], uploaded: &'a [String]) -> Vec<Sent<'a>> {
        files
            .iter()
            .enumerate()
            .map(|(i, u)| Sent {
                filename: &self.names[i],
                uploaded_filename: &uploaded[i],
                description: self.descriptions[i].as_deref(),
                spoiler: u.spoiler.get(),
                content_type: &self.types[i],
                voice: None,
            })
            .collect()
    }
}

/// An upload of `file` and its row, not started.
fn upload_of(
    channel: Snowflake,
    file: File,
    spoiler: Option<gtk::ToggleButton>,
    edit: Option<gtk::Button>,
    remove: &str,
) -> Rc<Upload> {
    let root = gtk::Box::builder().spacing(8).margin_start(12).margin_top(4).margin_bottom(4).build();
    let name = gtk::Label::builder()
        .label(&file.filename)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    let state = gtk::Label::builder().css_classes(["dim-label"]).ellipsize(gtk::pango::EllipsizeMode::End).build();
    let remove =
        gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text(remove).css_classes(["flat"]).build();
    root.append(&name);
    root.append(&state);
    if let Some(spoiler) = &spoiler {
        root.append(spoiler);
    }
    if let Some(edit) = &edit {
        root.append(edit);
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
    upload
}

impl App {
    /// The open channel, if files may be attached to it now: the message box
    /// shows, no edit has it, and the client would offer attaching there.
    pub(super) fn attach_channel(&self) -> Option<Snowflake> {
        let channel = self.account.state.borrow().channel?;
        let allowed = self.composer_box.is_visible()
            && self.editing().is_none()
            && !self.has_voice_draft(channel)
            && self.account.state.borrow().can_attach(channel);
        allowed.then_some(channel)
    }

    /// Whether `channel`'s draft takes files now: from the message box, or
    /// from the form that starts a post there.
    fn takes_files(&self, channel: Snowflake) -> bool {
        self.attach_channel() == Some(channel) || self.post_form_channel() == Some(channel)
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
                || self.schedule_channel().is_some()
                || self.thread_channel().is_some()
                || self.apps_channel().is_some(),
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
        self.refresh_post_files();
    }

    /// Put `channel`'s files in `tray`, as the message box's tray shows them,
    /// and say whether a message waiting on them may still be called off.
    pub(super) fn fill_tray(&self, tray: &gtk::Box, channel: Snowflake) -> bool {
        while let Some(child) = tray.first_child() {
            tray.remove(&child);
        }
        let sending = self.drafts.sending.borrow().get(&channel).cloned();
        for upload in self.drafts.of(channel) {
            upload.lock(sending.as_ref().is_some_and(|s| s.has(&upload)));
            tray.append(&upload.row.root);
        }
        sending.is_some_and(|s| !s.posting.get())
    }

    /// Take `channel`'s files out of the draft, unless a post is on its way
    /// that names them.
    pub(super) fn drop_post_files(self: &Rc<Self>, channel: Snowflake) {
        if self.drafts.sending.borrow().contains_key(&channel) {
            return;
        }
        for upload in self.drafts.of(channel) {
            self.remove_upload(channel, &upload);
        }
    }

    pub(super) fn build_attach(self: &Rc<Self>, area: &gtk::Box) {
        let drafts = &self.drafts;
        area.append(&drafts.tray);
        self.composer_box.append(&drafts.attach);
        for (button, parent) in [(&drafts.attach, &self.composer_box), (&drafts.notice_attach, &self.notice_bar)] {
            button.connect_clicked({
                let (this, parent) = (Rc::downgrade(self), parent.downgrade());
                move |button| {
                    let (Some(this), Some(parent)) = (this.upgrade(), parent.upgrade()) else { return };
                    let Some(bounds) = button.compute_bounds(&parent) else { return };
                    let rect = gdk::Rectangle::new(
                        bounds.x() as i32,
                        bounds.y() as i32,
                        bounds.width() as i32,
                        bounds.height() as i32,
                    );
                    super::menus::popup(&this, super::menus::ATTACH, (), &parent, super::menus::At::Below(rect));
                }
            });
        }
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
                let Some((pasted, _)) = this.composer_paste() else { return };
                if pasted != Pasted::Text {
                    view.stop_signal_emission_by_name("paste-clipboard");
                    this.paste_files(pasted == Pasted::Files);
                }
            }
        });
    }

    /// What pasting the clipboard in the message box does now, and whether
    /// the clipboard holds text: `None` where the box takes no files, which
    /// pastes text alone.
    fn composer_paste(&self) -> Option<(Pasted, bool)> {
        self.attach_channel()?;
        let formats = self.composer.clipboard().formats();
        let text = formats.contains_type(glib::Type::STRING);
        let files = formats.contains_type(gdk::FileList::static_type());
        Some((pasted(files, formats.contains_type(gdk::Texture::static_type()), text), text))
    }

    /// Let the menu's Paste attach the image or files on the clipboard, as
    /// Ctrl+V does: GTK greys it without text, once it has looked as the
    /// menu opens.
    pub(super) fn paste_opening(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(this) = this.upgrade() else { return };
            if this.composer_paste().is_some_and(|(pasted, _)| pasted != Pasted::Text) {
                this.composer.action_set_enabled("clipboard.paste", true);
            }
        });
    }

    /// Paste only the clipboard's text, as typed.
    fn paste_plain_text(self: &Rc<Self>) {
        let clipboard = self.composer.clipboard();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Ok(Some(text)) = clipboard.read_text_future().await else { return };
            if text.is_empty() {
                return;
            }
            let (view, buffer) = (&this.composer, this.composer.buffer());
            buffer.begin_user_action();
            buffer.delete_selection(true, view.is_editable());
            buffer.insert_interactive_at_cursor(&text, view.is_editable());
            buffer.end_user_action();
            view.grab_focus();
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
        let single = self.account.state.borrow().find_channel(channel).is_some_and(|(_, c)| c.rate_limit_per_user > 0);
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
    pub(super) async fn add_paths(self: &Rc<Self>, channel: Snowflake, paths: Vec<gio::File>) {
        let mut files = Vec::new();
        for path in paths {
            files.extend(file_at(path).await);
        }
        self.add_files(channel, files).await;
    }

    /// Add files to `channel`'s draft as the client's `R` does (518960): an
    /// empty file is turned back first, then HEIC and JPEG XR files become
    /// JPEGs (all at once, unseen), and only then are the limits checked, on
    /// what will be uploaded.
    async fn add_files(self: &Rc<Self>, channel: Snowflake, files: Vec<File>) {
        if files.is_empty() || !self.takes_files(channel) {
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
        if files.is_empty() || !self.takes_files(channel) {
            return;
        }
        let limits = {
            let state = self.account.state.borrow();
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
        let upload = upload_of(channel, file, Some(spoiler), Some(edit), "Remove");
        if let Some(spoiler) = &upload.row.spoiler {
            spoiler.connect_toggled({
                let upload = Rc::downgrade(&upload);
                move |button| {
                    if let Some(upload) = upload.upgrade() {
                        upload.spoiler.set(button.is_active());
                    }
                }
            });
        }
        upload.row.remove.connect_clicked({
            let (this, upload) = (Rc::downgrade(self), Rc::downgrade(&upload));
            move |_| {
                if let (Some(this), Some(upload)) = (this.upgrade(), upload.upgrade()) {
                    this.remove_upload(channel, &upload);
                }
            }
        });
        if let Some(edit) = &upload.row.edit {
            edit.connect_clicked({
                let (this, upload) = (Rc::downgrade(self), Rc::downgrade(&upload));
                move |_| {
                    if let (Some(this), Some(upload)) = (this.upgrade(), upload.upgrade()) {
                        this.edit_upload(&upload);
                    }
                }
            });
        }
        upload
    }

    /// A command's or a form's file (`setFile` with `allowOptimization: !1`,
    /// 102077 and 232430): never converted, with no spoiler or edit button,
    /// and a remove button (tooltip `remove`) that calls `removed`, which
    /// decides what taking it back does. Its upload starts at once, unless
    /// `CloudUpload.upload` (48325) would turn it down before any request:
    /// then it shows why and never asks. The client draws nothing for that;
    /// the row's words are Neucord's and change no request.
    pub(super) fn plain_upload(
        self: &Rc<Self>,
        channel: Snowflake,
        file: File,
        remove: &str,
        removed: impl Fn() + 'static,
    ) -> Rc<Upload> {
        let limits = {
            let state = self.account.state.borrow();
            state.limits().uploads(state.find_channel(channel).and_then(|(guild, _)| guild))
        };
        let checked = uploads::check_upload(&limits, file.size);
        let upload = upload_of(channel, file, None, None, remove);
        upload.optimized.set(true);
        upload.row.remove.connect_clicked(move |_| removed());
        match checked {
            Ok(()) => self.start_upload(&upload),
            Err(refusal) => upload.set_status(Status::Failed(refusal.body(), false)),
        }
        upload
    }

    /// Start (or start again) uploading, as the client does when a file is
    /// added and, for one that failed, when the message is sent.
    fn start_upload(self: &Rc<Self>, upload: &Rc<Upload>) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let channel = upload.channel;
        upload.set_status(Status::Uploading(0));
        let weak = Rc::downgrade(upload);
        let attempts = upload.attempts.clone();
        // Decided here, from the account's experiments, for the file's turn.
        let webp = self.account.state.borrow().limits().webp_conversion();
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
        upload.cancel(self.account.state.borrow().rest.clone());
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
        let uploaded = self.finish_uploads(sending).await?;
        let details = Details::of(&sending.files);
        post(rest, channel, message, &details.sent(&sending.files, &uploaded)).await
    }

    /// Wait for `sending`'s uploads, starting again those that failed in a
    /// way that restarts, and say what Discord keeps each by. Once they are
    /// all up, what follows is the post itself, which can't be taken back.
    async fn finish_uploads(self: &Rc<Self>, sending: &Sending) -> Result<Vec<String>, Stopped> {
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
        Ok(uploaded)
    }

    /// Start the post `make` builds in the forum or media channel `forum`,
    /// with `files` as its first message's attachments once they are all
    /// uploaded.
    pub(super) async fn post_with_uploads(
        self: &Rc<Self>,
        rest: &Rest,
        forum: Snowflake,
        files: Vec<Rc<Upload>>,
        make: impl FnOnce(Vec<serde_json::Value>) -> serde_json::Value,
    ) -> Result<Channel, Stopped> {
        if files.is_empty() {
            return rest.create_post(forum, make(Vec::new())).await.map_err(Stopped::Send);
        }
        let sending = Rc::new(Sending { files, posting: Cell::new(false) });
        self.drafts.sending.borrow_mut().insert(forum, sending.clone());
        self.refresh_attach();
        let result = match self.finish_uploads(&sending).await {
            Ok(uploaded) => {
                let details = Details::of(&sending.files);
                let sent = details.sent(&sending.files, &uploaded);
                let attachments = sent.iter().enumerate().map(|(at, s)| s.body(at)).collect();
                rest.create_post(forum, make(attachments)).await.map_err(Stopped::Send)
            }
            Err(stopped) => Err(stopped),
        };
        self.drafts.sending.borrow_mut().remove(&forum);
        if result.is_ok() {
            self.forget(forum, &sending);
        }
        self.refresh_attach();
        result
    }

    /// Cancel a send that is still uploading: the files are taken back, and
    /// the uploads that finished are deleted.
    pub(super) fn cancel_sending(self: &Rc<Self>) {
        let Some(channel) = self.account.state.borrow().channel else { return };
        let sending = self.drafts.sending.borrow().get(&channel).cloned();
        let Some(sending) = sending.filter(|s| !s.posting.get()) else { return };
        let rest = self.account.state.borrow().rest.clone();
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
        for form in self.drafts.forms.take() {
            form.upload.cancel(None);
        }
        self.drafts.files.borrow_mut().clear();
        self.drafts.sending.borrow_mut().clear();
        self.scheduling.clear();
        self.drop_voice();
    }

    /// What a message sent now from `channel` takes along: nothing where the
    /// client wouldn't offer attaching, which also hides the draft.
    pub(super) fn files_for_message(&self, channel: Snowflake) -> Vec<Rc<Upload>> {
        if !self.account.state.borrow().can_attach(channel) {
            return Vec::new();
        }
        self.drafts.for_new_message(channel)
    }

    /// The sizes of `files`, for the checks before sending.
    pub(super) fn sizes(files: &[Rc<Upload>]) -> Vec<u64> {
        files.iter().map(|u| u.file.borrow().size).collect()
    }
}
