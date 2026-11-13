//! A command's file options: the boxes that take the place of the message's
//! files above the box while a command is in it (102077 `L`), the file each
//! option holds (the client's `SlashCommand` uploads, 522602), and running
//! a command that has files (`B` in 545152, whose staging only waits on
//! uploads started as each file was picked).
//!
//! An option's file is held by the command, not by its pill: removing the
//! pill hides its box and leaves it out of the run, and adding the pill
//! again brings it back, as the client's list does. Ending the command
//! stops its uploads and asks nothing of Discord (Q-AO1).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;
use discord::commands::{App as CommandApp, Command, CommandOption, Run, RunFrom, Values, option_type};
use discord::uploads::{self, File, Staging};
use discord::{Snowflake, images};
use gtk::{gdk, gio, glib};

use super::App;
use super::slash::{Active, Running};
use super::uploads::{Details, Upload, file_at, staged};

/// The remove button's tooltip on a command's file (`vN7REz`, 914905).
const REMOVE: &str = "Remove Attachment";

/// The boxes above the message box, and what they were last drawn from.
pub(super) struct FileBoxes {
    pub(super) holder: gtk::Box,
    /// Each option drawn, and its file's upload, if it held one.
    shown: RefCell<Vec<(String, Option<*const Upload>)>>,
}

impl FileBoxes {
    pub(super) fn new() -> Self {
        let holder = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).visible(false).build();
        FileBoxes { holder, shown: RefCell::default() }
    }
}

impl Active {
    /// The name of each option's file (`em` in 84046, `g` in 317681).
    pub(super) fn staged(&self) -> Values {
        self.files.iter().map(|(option, upload)| (option.clone(), upload.name())).collect()
    }

    /// Stop every file's upload, asking nothing of Discord: the client's
    /// stay listed (and go on), so this only sends less.
    pub(super) fn drop_files(&mut self) {
        for upload in std::mem::take(&mut self.files).into_values() {
            upload.cancel(None);
        }
    }
}

/// A run whose files are still uploading: what its body is made of. The
/// session goes in once they are up, as `handleCommand` (870801) reads it
/// when the queue sends.
pub(super) struct FileRun {
    pub(super) application: Snowflake,
    pub(super) guild: Option<Snowflake>,
    pub(super) channel: Snowflake,
    pub(super) data: serde_json::Value,
    pub(super) nonce: Snowflake,
    pub(super) from: RunFrom,
    /// In the order the run numbers them ([`discord::commands::file_options`]).
    pub(super) files: Vec<Rc<Upload>>,
}

impl App {
    pub(super) fn build_command_files(&self, area: &gtk::Box) {
        area.append(&self.slash.file_boxes.holder);
    }

    /// Draw a box per file option with a pill, in the command's order
    /// (`T.map(j)` in 102077), when what they show changed.
    pub(super) fn refresh_command_files(self: &Rc<Self>) {
        let (options, wanted) = {
            let slot = self.slash.active.borrow();
            let pills = self.read_line().map(|(line, _)| line.pills).unwrap_or_default();
            match slot.as_ref() {
                Some(active) => {
                    let options: Vec<CommandOption> = active
                        .command
                        .options
                        .iter()
                        .filter(|o| o.kind == option_type::ATTACHMENT && pills.iter().any(|p| p.option == o.name))
                        .cloned()
                        .collect();
                    let wanted: Vec<(String, Option<*const Upload>)> =
                        options.iter().map(|o| (o.name.clone(), active.files.get(&o.name).map(Rc::as_ptr))).collect();
                    let files: BTreeMap<String, Rc<Upload>> = active.files.clone();
                    (options.into_iter().map(|o| (files.get(&o.name).cloned(), o)).collect::<Vec<_>>(), wanted)
                }
                None => (Vec::new(), Vec::new()),
            }
        };
        let boxes = &self.slash.file_boxes;
        if *boxes.shown.borrow() == wanted {
            return;
        }
        boxes.shown.replace(wanted);
        while let Some(child) = boxes.holder.first_child() {
            boxes.holder.remove(&child);
        }
        boxes.holder.set_visible(!options.is_empty());
        for (upload, option) in options {
            let shown = match upload {
                Some(upload) => staged_box(&option, &upload),
                None => self.empty_box(&option),
            };
            boxes.holder.append(&shown);
        }
    }

    /// An option with no file (`j` in 102077): its name, and a button that
    /// opens the file picker, which a file dropped on it stands for.
    fn empty_box(self: &Rc<Self>, option: &CommandOption) -> gtk::Widget {
        let root = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).margin_start(12).build();
        root.append(
            &gtk::Label::builder().label(&option.display_name).xalign(0.0).css_classes(["caption-heading"]).build(),
        );
        let face = gtk::Box::builder().spacing(8).build();
        face.append(&gtk::Image::from_icon_name("document-open-symbolic"));
        face.append(&gtk::Label::builder().label(option.file_prompt()).wrap(true).xalign(0.0).build());
        let button = gtk::Button::builder().child(&face).halign(gtk::Align::Start).build();
        let name = option.name.clone();
        button.connect_clicked({
            let (this, name) = (Rc::downgrade(self), name.clone());
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.browse_command_file(&name);
                }
            }
        });
        // The first file dropped, highlighted while files are over it.
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_enter({
            let button = button.downgrade();
            move |_, _, _| {
                if let Some(button) = button.upgrade() {
                    button.add_css_class("suggested-action");
                }
                gdk::DragAction::COPY
            }
        });
        target.connect_leave({
            let button = button.downgrade();
            move |_| {
                if let Some(button) = button.upgrade() {
                    button.remove_css_class("suggested-action");
                }
            }
        });
        target.connect_drop({
            let (this, button) = (Rc::downgrade(self), button.downgrade());
            move |_, value, _, _| {
                if let Some(button) = button.upgrade() {
                    button.remove_css_class("suggested-action");
                }
                let (Some(this), Ok(list)) = (this.upgrade(), value.get::<gdk::FileList>()) else { return false };
                let Some(first) = list.files().into_iter().next() else { return false };
                this.pick_command_file(&name, first);
                true
            }
        });
        button.add_controller(target);
        root.append(&button);
        root.upcast()
    }

    /// The file picker for `option`: one file, filtered to the extensions it
    /// takes (`activateUploadDialogue`, `multiple: !1`), matched without
    /// case as a browser's `accept` is.
    fn browse_command_file(self: &Rc<Self>, option: &str) {
        let extensions = {
            let slot = self.slash.active.borrow();
            let Some(found) = slot.as_ref().and_then(|a| a.command.options.iter().find(|o| o.name == option)) else {
                return;
            };
            found.file_extensions()
        };
        let dialog = gtk::FileDialog::builder().title("Upload a File").build();
        if !extensions.is_empty() {
            let filter = gtk::FileFilter::new();
            for extension in &extensions {
                filter.add_suffix(extension);
            }
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            dialog.set_default_filter(Some(&filter));
        }
        let (this, option) = (self.clone(), option.to_owned());
        glib::spawn_future_local(async move {
            if let Ok(path) = dialog.open_future(Some(&this.window)).await {
                this.pick_command_file(&option, path);
            }
        });
    }

    /// A file picked or dropped for `option` (`O` in 102077): refused with
    /// an alert when its name isn't of the types the option takes, else it
    /// starts uploading at once, never converted (`allowOptimization: !1`).
    /// Only a paste replaces a file: one picked while the option holds a
    /// file is ignored.
    pub(super) fn pick_command_file(self: &Rc<Self>, option: &str, path: gio::File) {
        let Some(asked) = self.command_key_of() else { return };
        let (this, option) = (self.clone(), option.to_owned());
        glib::spawn_future_local(async move {
            let Some(file) = file_at(path).await else { return };
            // The command may have ended or changed while the file was looked at.
            let found = {
                let slot = this.slash.active.borrow();
                slot.as_ref()
                    .filter(|a| (a.channel, a.command.id.as_str()) == (asked.0, asked.1.as_str()))
                    .filter(|a| !a.files.contains_key(&option))
                    .and_then(|a| a.command.options.iter().find(|o| o.name == option).cloned())
            };
            let Some(found) = found else { return };
            if !found.takes_file(&file.filename) {
                let words = uploads::invalid_type_words(&found.file_types_words().unwrap_or_default());
                let alert = adw::AlertDialog::new(Some(uploads::INVALID_TYPE), Some(&words));
                alert.add_response("okay", "_Okay");
                alert.present(Some(&this.window));
                return;
            }
            let upload = this.plain_upload(asked.0, file, REMOVE, this.file_remover(&option));
            this.stage_command_file(&option, upload);
        });
    }

    /// The first file pasted with a command in the box (84046's paste
    /// handler): to the file option whose pill holds the caret, else the
    /// first with no file, pill or not. Its type isn't checked, and it may
    /// be converted as a message's file is (no `allowOptimization`, so
    /// 565150's default). It replaces a file the option held.
    pub(super) fn paste_command_file(self: &Rc<Self>, file: File) {
        let holding = self.read_line().and_then(|(line, _)| {
            let caret = self.composer.buffer().property::<i32>("cursor-position").max(0) as usize;
            discord::command_line::active(&line, caret).map(|p| p.option.clone())
        });
        let target = {
            let slot = self.slash.active.borrow();
            let Some(active) = slot.as_ref() else { return };
            let staged = active.staged();
            discord::command_line::paste_option(&active.command, holding.as_deref(), &staged).map(|o| o.name.clone())
        };
        let (Some(option), Some(asked)) = (target, self.command_key_of()) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let file = images::prepare(file).await;
            if this.command_key_of().as_ref() != Some(&asked) {
                return;
            }
            let upload = this.pasted_upload(asked.0, file, REMOVE, this.file_remover(&option));
            this.stage_command_file(&option, upload);
        });
    }

    /// The command in the box, by its channel and id.
    fn command_key_of(&self) -> Option<(Snowflake, String)> {
        self.slash.active.borrow().as_ref().map(|a| (a.channel, a.command.id.clone()))
    }

    /// What `option`'s remove button does.
    fn file_remover(self: &Rc<Self>, option: &str) -> impl Fn() + 'static {
        let (this, option) = (Rc::downgrade(self), option.to_owned());
        move || {
            if let Some(this) = this.upgrade() {
                this.remove_command_file(&option);
            }
        }
    }

    /// Give `option` its file, stopping the upload of one it held (the
    /// client drops it from the list and lets it run on).
    fn stage_command_file(self: &Rc<Self>, option: &str, upload: Rc<Upload>) {
        let old = match self.slash.active.borrow_mut().as_mut() {
            Some(active) => active.files.insert(option.to_owned(), upload),
            None => return upload.cancel(None),
        };
        if let Some(old) = old {
            old.cancel(None);
        }
        self.sync_command_line();
        self.refresh_command_files();
    }

    /// Remove Attachment (`removeFromMsgDraft`, 48325): the upload stops,
    /// and Discord is asked to delete it if it had finished.
    pub(super) fn remove_command_file(self: &Rc<Self>, option: &str) {
        let removed = self.slash.active.borrow_mut().as_mut().and_then(|a| a.files.remove(option));
        let Some(upload) = removed else { return };
        upload.cancel(self.account.state.borrow().rest.clone());
        self.sync_command_line();
        self.refresh_command_files();
    }

    /// Run a command with files (`B` in 545152), after the client's order:
    /// pending at once, the thread reopened, then the sizes checked
    /// (`stageAttachments`), every upload waited on without starting a
    /// failed one again, the first failure failing the run, and only then
    /// the request. A run that doesn't go aborts what is still uploading,
    /// with no `DELETE`. Whether it started.
    pub(super) fn send_command_files(self: &Rc<Self>, command: Command, app: &CommandApp, run: FileRun) -> bool {
        let rest = self.account.state.borrow().rest.clone();
        let Some(rest) = rest else {
            for upload in &run.files {
                upload.cancel(None);
            }
            return false;
        };
        let (channel, nonce) = (run.channel, run.nonce);
        self.command_used(&command, run.guild);
        self.account
            .state
            .borrow_mut()
            .running
            .insert(nonce, Running { channel, app: app.name.clone(), created: false });
        self.refresh_command_status();
        self.drafts.commands.borrow_mut().extend(run.files.iter().cloned());
        let this = self.clone();
        let epoch = self.account.epoch.get();
        glib::spawn_future_local(async move {
            let files = run.files.clone();
            let stop = |words: Option<String>| {
                for upload in &files {
                    upload.cancel(None);
                }
                this.drafts.commands.borrow_mut().retain(|u| !files.iter().any(|f| Rc::ptr_eq(f, u)));
                this.account.state.borrow_mut().running.remove(&nonce);
                this.refresh_command_status();
                if let Some(words) = words {
                    this.show_composer_error(&words);
                }
            };
            let reopened = this.reopen(&rest, channel).await;
            if this.account.epoch.get() != epoch {
                return;
            }
            if let Err(why) = reopened {
                return stop(Some(format!("Not sent: {why}")));
            }
            let limits = {
                let state = this.account.state.borrow();
                state.limits().uploads(state.find_channel(channel).and_then(|(guild, _)| guild))
            };
            if let Err(refused) = uploads::check_staging(&limits, &App::sizes(&files)) {
                return stop(Some(refused.words()));
            }
            let uploaded = staged(&files).await;
            if this.account.epoch.get() != epoch {
                return;
            }
            let Ok(uploaded) = uploaded else {
                return stop(Some(Staging::Failed { files: files.len() }.words()));
            };
            let details = Details::of(&files);
            let sent = details.sent(&files, &uploaded);
            let body = {
                let state = this.account.state.borrow();
                (!state.session_id.is_empty()).then(|| {
                    let FileRun { application, guild, channel, ref data, nonce, from, .. } = run;
                    let data = data.clone();
                    Run { application, guild, channel, session: &state.session_id, data, nonce, from }.body(&sent)
                })
            };
            // Without a session nothing is sent, as Enter sends nothing then.
            let Some(body) = body else { return stop(None) };
            this.drafts.commands.borrow_mut().retain(|u| !files.iter().any(|f| Rc::ptr_eq(f, u)));
            if let Err(failure) = rest.run_command(body).await {
                this.command_refused(nonce, &command, &failure);
            }
        });
        true
    }
}

/// An option holding a file (914905's item with `canEdit: !1`): its name,
/// then the file's row, whose only button removes it.
fn staged_box(option: &CommandOption, upload: &Rc<Upload>) -> gtk::Widget {
    let root = gtk::Box::builder().spacing(4).margin_start(12).build();
    root.append(
        &gtk::Label::builder()
            .label(format!("{}:", option.display_name))
            .valign(gtk::Align::Center)
            .css_classes(["caption-heading"])
            .build(),
    );
    let row = upload.row();
    if row.parent().is_some() {
        row.unparent();
    }
    row.set_hexpand(true);
    root.append(row);
    root.upcast()
}
