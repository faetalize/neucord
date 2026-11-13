//! An app's form (a modal): opened by any `INTERACTION_MODAL_CREATE`
//! (764056), drawn as 121338 and 232430 draw it, checked on every change and
//! on Submit, and sent as MODAL_SUBMIT. What it holds and sends is
//! [`discord::modal`]. A dialog like Create Thread's: X and Escape cancel,
//! Submit ends the header.
//!
//! Menus of users, roles, mentionables and channels show and send the
//! defaults that resolve, but can't be picked from (they need the member
//! search). A file field uploads what is added at once, unconverted, and
//! the files stay with the form's `custom_id` in that channel until a
//! submit succeeds, which deletes them.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use discord::gateway::Outcome;
use discord::modal::{
    self, DefaultKind, FieldState, FileUpload, Modal, Node, NodeKind, SUBMIT_ERROR, SUBMIT_TIMEOUT, Submit,
    UPLOAD_TIMEOUT,
};
use discord::{Snowflake, uploads};
use gtk::{gdk, gio, glib, pango};

use super::App;
use super::components::{Choices, choices_button, components};
use super::uploads::{Details, FormUpload, Upload, file_at, staged};
use crate::render::{self, Context};

/// What a field that can't be used yet says.
const NOT_YET: &str = "Neucord can't use this yet";

/// One open form.
pub(super) struct Form {
    modal: Modal,
    dialog: adw::Dialog,
    submit: gtk::Button,
    /// "Submit", or a spinner while ours is in flight.
    submit_face: gtk::Stack,
    error: gtk::Label,
    fields: Vec<Field>,
    /// The last submit this form sent.
    nonce: Cell<Option<Snowflake>>,
    /// The request on its way, which closing the form aborts.
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

/// A field, and the title that says what is wrong with it.
struct Field {
    node: Node,
    title: Rc<Title>,
    /// What takes the focus when it is wrong.
    focus: gtk::Widget,
    /// A file field's own parts, held here for its widgets, which reach
    /// them weakly.
    _files: Option<Rc<FileField>>,
}

/// A field's title (742158 `FormTitle`): "Label *" when required, "Label -
/// error" in red when wrong. Without a label of its own only an error shows.
struct Title {
    label: gtk::Label,
    name: Option<String>,
    required: bool,
}

impl Title {
    fn new(name: Option<String>, required: bool) -> Rc<Title> {
        let label = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["heading"]).build();
        let title = Rc::new(Title { label, name, required });
        title.show(None);
        title
    }

    fn show(&self, error: Option<&str>) {
        let text = match (&self.name, error) {
            (Some(name), Some(error)) => format!("{name} - {error}"),
            (None, Some(error)) => error.to_owned(),
            (Some(name), None) if self.required => format!("{name} *"),
            (Some(name), None) => name.clone(),
            (None, None) => String::new(),
        };
        self.label.set_label(&text);
        self.label.set_visible(!text.is_empty());
        if error.is_some() {
            self.label.add_css_class("error");
        } else {
            self.label.remove_css_class("error");
        }
    }
}

/// Keep what is typed or pasted into an input within `max` UTF-16 units, as
/// HTML's `maxlength` does, cutting it short rather than refusing it.
fn cap_entry(entry: &gtk::Entry, max: Option<usize>) {
    let Some(editable) = entry.delegate().filter(|_| max.is_some()) else { return };
    editable.connect_insert_text(move |editable, text, position| {
        let kept = modal::fit(&editable.text(), text, max);
        if kept.len() != text.len() {
            editable.stop_signal_emission_by_name("insert-text");
            // What is kept fits, so this insertion passes untouched.
            if !kept.is_empty() {
                editable.insert_text(kept, position);
            }
        }
    });
}

fn cap_buffer(buffer: &gtk::TextBuffer, max: Option<usize>) {
    if max.is_none() {
        return;
    }
    buffer.connect_insert_text(move |buffer, iter, text| {
        let current = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let kept = modal::fit(&current, text, max);
        if kept.len() != text.len() {
            buffer.stop_signal_emission_by_name("insert-text");
            if !kept.is_empty() {
                buffer.insert(iter, kept);
            }
        }
    });
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["caption", "dimmed"]).build()
}

/// A file field (232430 `eK`): drawn again on every change, from the
/// field's state and the form's files that are still there.
struct FileField {
    channel: Snowflake,
    custom_id: String,
    node: Node,
    field: FileUpload,
    holder: gtk::Box,
    set: Box<dyn Fn(FieldState)>,
    /// "Drop here to upload", shown while files are dragged over the area.
    dropping: RefCell<Option<gtk::Label>>,
}

/// What an option list's entry says: its label, and its description under it.
fn option_label(option: &discord::component::SelectOption) -> gtk::Widget {
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    text.append(&gtk::Label::builder().label(&option.label).xalign(0.0).wrap(true).build());
    if let Some(description) = &option.description {
        text.append(&caption(description));
    }
    text.upcast()
}

impl App {
    /// An app answered with a form. The interaction that asked for it ends
    /// as answered (436283 `p`): a pressed button or a running command's
    /// status goes back to normal. Its users and members are learnt (287809,
    /// 696451, for the modal channel's server), and the form opens whatever
    /// the nonce and whichever channel is open (764056 checks neither).
    pub(super) fn on_modal(self: &Rc<Self>, modal: Modal) {
        if let Some(nonce) = modal.nonce {
            self.on_interaction(nonce, Outcome::Success, None);
        }
        self.learn_users(modal.resolved_users.iter());
        {
            let mut state = self.account.state.borrow_mut();
            let guild = state.find_channel(modal.channel).and_then(|(guild, _)| guild.map(|g| g.id));
            if let Some(guild) = guild {
                state.members.add(guild, modal.resolved_members.clone(), &[]);
            }
        }
        self.open_modal(modal);
    }

    /// The name each default of the form's entity selects goes by, for those
    /// that resolve (`er` in 232430): a known user, by their nickname in the
    /// modal channel's server or their name; a role of that server; one of
    /// its channels of an allowed type. The rest are dropped.
    fn modal_defaults(&self, modal: &Modal) -> Vec<(Snowflake, String)> {
        let state = self.account.state.borrow();
        let guild = state.find_channel(modal.channel).and_then(|(guild, _)| guild);
        let mut names = Vec::new();
        for node in modal.fields() {
            let NodeKind::EntitySelect(select) = &node.kind else { continue };
            for &(kind, id) in &select.defaults {
                let name = match kind {
                    DefaultKind::User => state.users.get(&id.0).map(|name| {
                        guild.and_then(|g| state.members.nick(g.id, id)).map_or_else(|| name.clone(), str::to_owned)
                    }),
                    DefaultKind::Role => {
                        guild.and_then(|g| g.roles.iter().find(|r| r.id == id)).map(|r| r.name.clone())
                    }
                    DefaultKind::Channel => guild
                        .and_then(|g| g.channels.iter().chain(&g.threads).find(|c| c.id == id))
                        .filter(|c| select.channel_types.is_empty() || select.channel_types.contains(&c.kind.raw()))
                        .map(|c| c.shown_name().unwrap_or_default().to_owned()),
                };
                if let Some(name) = name {
                    names.push((id, name));
                }
            }
        }
        names
    }

    fn open_modal(self: &Rc<Self>, modal: Modal) {
        let defaults = self.modal_defaults(&modal);
        let started = {
            let mut state = self.account.state.borrow_mut();
            let draft = state.modal_drafts.entry(modal.custom_id.clone()).or_default();
            draft.start(&modal, |_, id| defaults.iter().any(|(known, _)| *known == id))
        };
        let form = self.build_modal(modal, &defaults);
        // What a field starts from is checked at once; an empty one waits.
        for field in form.fields.iter().filter(|f| started.contains(&f.node.path)) {
            self.check_modal_field(&form.modal.custom_id, field);
        }
        self.modal_forms.borrow_mut().push(form.clone());
        self.refresh_modal_forms();
        form.dialog.present(Some(&self.window));
        if let Some(path) = form.modal.autofocus()
            && let Some(field) = form.fields.iter().find(|f| f.node.path == path)
        {
            field.focus.grab_focus();
        }
    }

    /// Check one field against what the draft holds, and say so on it.
    fn check_modal_field(&self, custom_id: &str, field: &Field) -> bool {
        let error = {
            let state = self.account.state.borrow();
            let draft = state.modal_drafts.get(custom_id);
            modal::validate(&field.node, draft.and_then(|d| d.state(&field.node)))
        };
        field.title.show(error.as_deref());
        error.is_none()
    }

    fn build_modal(self: &Rc<Self>, modal: Modal, defaults: &[(Snowflake, String)]) -> Rc<Form> {
        let (nick, member) = {
            let state = self.account.state.borrow();
            let bot = modal.application.bot.as_ref().map(|b| b.id);
            match (state.guild, bot) {
                (Some(guild), Some(bot)) if state.members.roles(guild, bot).is_some() => (
                    state.members.nick(guild, bot).map(str::to_owned),
                    Some((guild, state.members.avatar(guild, bot).map(str::to_owned))),
                ),
                _ => (None, None),
            }
        };
        let name = modal.app_name(nick.as_deref()).to_owned();
        let size = discord::cdn::asset_size(128 * self.images.scale().max(1) as u32);
        let icon = modal.app_icon(member.as_ref().map(|(guild, avatar)| (*guild, avatar.as_deref())), size);

        let avatar = adw::Avatar::builder().size(24).text(&name).show_initials(false).build();
        avatar.set_icon_name(Some("application-x-addon-symbolic"));
        if let Some(url) = icon {
            let avatar = avatar.downgrade();
            self.images.load(&url, move |paintable| {
                if let Some(avatar) = avatar.upgrade() {
                    avatar.set_custom_image(Some(&paintable));
                }
            });
        }
        let heading = gtk::Box::builder().spacing(8).build();
        heading.append(&avatar);
        heading.append(
            &gtk::Label::builder()
                .label(&modal.title)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["heading"])
                .build(),
        );

        let submit_face = gtk::Stack::builder().hhomogeneous(true).vhomogeneous(true).build();
        submit_face.add_named(&gtk::Label::new(Some("Submit")), Some("label"));
        submit_face.add_named(&gtk::Spinner::builder().spinning(true).build(), Some("spinner"));
        let submit = gtk::Button::builder().child(&submit_face).css_classes(["suggested-action"]).build();
        let header = adw::HeaderBar::builder().title_widget(&heading).build();
        header.pack_end(&submit);

        let error = gtk::Label::builder()
            .label(SUBMIT_ERROR)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();
        let notice = gtk::Label::builder()
            .use_markup(true)
            .label(format!(
                "This form will be submitted to <b>{}</b>. Do not share passwords or other sensitive information.",
                glib::markup_escape_text(&name)
            ))
            .xalign(0.0)
            .wrap(true)
            .build();
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.append(&error);
        body.append(&notice);

        let mut fields = Vec::new();
        for node in &modal.components {
            body.append(&self.modal_node(&modal, node, None, defaults, &mut fields));
        }
        let scroller = gtk::ScrolledWindow::builder()
            .child(&body)
            .propagate_natural_height(true)
            .max_content_height(560)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));
        let dialog = adw::Dialog::builder().title(&modal.title).content_width(480).child(&toolbar).build();

        let form = Rc::new(Form {
            modal,
            dialog,
            submit,
            submit_face,
            error,
            fields,
            nonce: Cell::new(None),
            task: RefCell::default(),
        });
        let this = Rc::downgrade(self);
        form.submit.connect_clicked({
            let (this, form) = (this.clone(), Rc::downgrade(&form));
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.submit_modal(&form);
                }
            }
        });
        // HTML's implicit submission, with no submit button in the form.
        if form.modal.enter_submits() {
            for field in &form.fields {
                if let Some(entry) = field.focus.downcast_ref::<gtk::Entry>() {
                    let (this, form) = (this.clone(), Rc::downgrade(&form));
                    entry.connect_activate(move |_| {
                        if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                            this.submit_modal(&form);
                        }
                    });
                }
            }
        }
        // X and Escape keep what was typed, and abort a request on its way.
        form.dialog.connect_closed({
            let form = Rc::downgrade(&form);
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.modal_closed(&form);
                }
            }
        });
        form
    }

    /// The widget of one component, its fields added to `fields`. `title`
    /// is the label around it, if any.
    fn modal_node(
        self: &Rc<Self>,
        modal: &Modal,
        node: &Node,
        title: Option<(&str, Option<&str>)>,
        defaults: &[(Snowflake, String)],
        fields: &mut Vec<Field>,
    ) -> gtk::Widget {
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        match &node.kind {
            NodeKind::Row(children) => {
                for child in children {
                    column.append(&self.modal_node(modal, child, None, defaults, fields));
                }
                return column.upcast();
            }
            NodeKind::Label { label, description, child } => {
                return self.modal_node(modal, child, Some((label, description.as_deref())), defaults, fields);
            }
            NodeKind::Text(content) => {
                let names = self.names();
                let cx = Context::new(&names, Some(&self.images));
                return render::render(&discord::markup::parse(content), &cx, false);
            }
            NodeKind::Unsubmittable(component) => {
                let names = self.names();
                let cx = Context::new(&names, Some(&self.images));
                return components(std::slice::from_ref(component), &cx, None);
            }
            _ => {}
        }
        // A legacy row's text input is titled by its own label.
        let own = match &node.kind {
            NodeKind::TextInput(input) => input.label.as_deref(),
            _ => None,
        };
        let name = title.map(|(label, _)| label).or(own).map(str::to_owned);
        let checkbox = matches!(node.kind, NodeKind::Checkbox { .. });
        let field_title = Title::new(name.filter(|_| !checkbox), node.is_required());
        column.append(&field_title.label);
        if let Some(description) = title.and_then(|(_, d)| d).filter(|_| !checkbox) {
            column.append(&caption(description));
        }
        let state = {
            let state = self.account.state.borrow();
            state.modal_drafts.get(&modal.custom_id).and_then(|d| d.state(node)).cloned()
        };
        let set = self.modal_setter(&modal.custom_id, node, &field_title);
        let mut file_field = None;
        let focus: gtk::Widget = match &node.kind {
            NodeKind::TextInput(input) if input.paragraph => {
                let view = gtk::TextView::builder()
                    .wrap_mode(gtk::WrapMode::WordChar)
                    .accepts_tab(false)
                    .top_margin(8)
                    .bottom_margin(8)
                    .left_margin(8)
                    .right_margin(8)
                    .build();
                let buffer = view.buffer();
                if let Some(FieldState::Text(text)) = &state {
                    buffer.set_text(text);
                }
                cap_buffer(&buffer, input.max_length);
                buffer.connect_changed(move |buffer| {
                    set(FieldState::Text(buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).into()));
                });
                let scroller = gtk::ScrolledWindow::builder()
                    .child(&view)
                    .min_content_height(72)
                    .max_content_height(240)
                    .propagate_natural_height(true)
                    .css_classes(["card"])
                    .build();
                if let Some(placeholder) = &input.placeholder {
                    view.set_tooltip_text(Some(placeholder));
                }
                column.append(&scroller);
                view.upcast()
            }
            NodeKind::TextInput(input) => {
                let entry =
                    gtk::Entry::builder().placeholder_text(input.placeholder.as_deref().unwrap_or_default()).build();
                if let Some(FieldState::Text(text)) = &state {
                    entry.set_text(text);
                }
                cap_entry(&entry, input.max_length);
                entry.connect_changed(move |entry| set(FieldState::Text(entry.text().into())));
                column.append(&entry);
                entry.upcast()
            }
            NodeKind::StringSelect(select) => {
                let picked = match &state {
                    Some(FieldState::Picks(values)) => values.clone(),
                    _ => Vec::new(),
                };
                let max = select.max.unwrap_or(1);
                let choices = Choices {
                    placeholder: select.placeholder.as_deref(),
                    options: &select.options,
                    max,
                    clearable: max <= 1 && !select.required,
                };
                let button = choices_button(&choices, picked, Some(&self.images), false, move |now| {
                    set(FieldState::Picks(now.to_vec()));
                    true
                });
                button.set_sensitive(!select.disabled);
                column.append(&button);
                button.upcast()
            }
            NodeKind::EntitySelect(select) => {
                let picked = match &state {
                    Some(FieldState::Entities(ids)) => ids.clone(),
                    _ => Vec::new(),
                };
                let names: Vec<&str> = picked
                    .iter()
                    .filter_map(|id| defaults.iter().find(|(known, _)| known == id).map(|(_, name)| name.as_str()))
                    .collect();
                let face = match (names.is_empty(), &select.placeholder) {
                    (false, _) => names.join(", "),
                    (true, Some(placeholder)) => placeholder.clone(),
                    (true, None) => modal::SELECT_PLACEHOLDER.to_owned(),
                };
                let button = gtk::MenuButton::builder()
                    .child(&gtk::Label::builder().label(face).ellipsize(pango::EllipsizeMode::End).build())
                    .sensitive(false)
                    .tooltip_text(NOT_YET)
                    .build();
                column.append(&button);
                button.upcast()
            }
            NodeKind::FileUpload(upload) => {
                let holder = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
                let files = Rc::new(FileField {
                    channel: modal.channel,
                    custom_id: modal.custom_id.clone(),
                    node: node.clone(),
                    field: upload.clone(),
                    holder: holder.clone(),
                    set: Box::new(set),
                    dropping: RefCell::default(),
                });
                self.fill_files(&files);
                holder.add_controller(self.files_drop(&files));
                column.append(&holder);
                file_field = Some(files);
                holder.upcast()
            }
            NodeKind::RadioGroup { options, required, .. } => {
                let picked = match &state {
                    Some(FieldState::Radio(value)) => value.clone(),
                    _ => None,
                };
                let clear = gtk::Button::builder()
                    .label("Clear selection")
                    .halign(gtk::Align::Start)
                    .css_classes(["flat", "caption"])
                    .visible(picked.is_some() && !required)
                    .build();
                let set = Rc::new(set);
                let mut group: Option<gtk::CheckButton> = None;
                let mut radios = Vec::new();
                for option in options {
                    let radio = gtk::CheckButton::builder()
                        .child(&option_label(option))
                        .active(picked.as_deref() == Some(option.value.as_str()))
                        .build();
                    radio.set_group(group.as_ref());
                    group.get_or_insert_with(|| radio.clone());
                    let (set, value, clear, optional) = (set.clone(), option.value.clone(), clear.clone(), !required);
                    radio.connect_toggled(move |radio| {
                        if radio.is_active() {
                            clear.set_visible(optional);
                            set(FieldState::Radio(Some(value.clone())));
                        }
                    });
                    column.append(&radio);
                    radios.push(radio);
                }
                clear.connect_clicked(move |clear| {
                    for radio in &radios {
                        radio.set_active(false);
                    }
                    clear.set_visible(false);
                    set(FieldState::Radio(None));
                });
                column.append(&clear);
                group.map_or_else(|| column.clone().upcast(), |radio| radio.upcast())
            }
            NodeKind::CheckboxGroup { options, max, .. } => {
                let picked = Rc::new(RefCell::new(match &state {
                    Some(FieldState::Checks(values)) => values.clone(),
                    _ => Vec::new(),
                }));
                let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> = Rc::default();
                // At the limit the rest wait for one to be put back.
                let fill = {
                    let (picked, checks, max) = (picked.clone(), checks.clone(), *max);
                    move || {
                        let full = max.is_some_and(|max| picked.borrow().len() >= max);
                        for (value, check) in checks.borrow().iter() {
                            check.set_sensitive(!full || picked.borrow().contains(value));
                        }
                    }
                };
                let (set, fill) = (Rc::new(set), Rc::new(fill));
                for option in options {
                    let check = gtk::CheckButton::builder()
                        .child(&option_label(option))
                        .active(picked.borrow().contains(&option.value))
                        .build();
                    checks.borrow_mut().push((option.value.clone(), check.clone()));
                    let (set, fill, picked, value) = (set.clone(), fill.clone(), picked.clone(), option.value.clone());
                    check.connect_toggled(move |check| {
                        {
                            let mut picked = picked.borrow_mut();
                            picked.retain(|v| *v != value);
                            if check.is_active() {
                                picked.push(value.clone());
                            }
                        }
                        fill();
                        set(FieldState::Checks(picked.borrow().clone()));
                    });
                    column.append(&check);
                }
                fill();
                let first = checks.borrow().first().map(|(_, check)| check.clone().upcast());
                first.unwrap_or_else(|| column.clone().upcast())
            }
            NodeKind::Checkbox { .. } => {
                let (label, description) = title.unwrap_or_default();
                let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
                text.append(&gtk::Label::builder().label(label).xalign(0.0).wrap(true).build());
                if let Some(description) = description {
                    text.append(&caption(description));
                }
                let check = gtk::CheckButton::builder()
                    .child(&text)
                    .active(matches!(state, Some(FieldState::Check(true))))
                    .build();
                check.connect_toggled(move |check| set(FieldState::Check(check.is_active())));
                column.append(&check);
                check.upcast()
            }
            _ => unreachable!("only fields are left"),
        };
        fields.push(Field { node: node.clone(), title: field_title, focus, _files: file_field });
        column.upcast()
    }

    /// The form's files in its channel, in the order they were added: what
    /// a submit names in `attachments` (969508 `M`).
    fn form_uploads(&self, channel: Snowflake, custom_id: &str) -> Vec<(String, Rc<Upload>)> {
        let forms = self.drafts.forms.borrow();
        let ours = forms.iter().filter(|f| f.channel == channel && f.custom_id == custom_id);
        ours.map(|f| (f.id.clone(), f.upload.clone())).collect()
    }

    /// The files a field holds, without those that have gone (taken back,
    /// or added in another channel), which its state then forgets too.
    fn file_ids(&self, files: &FileField) -> Vec<String> {
        let held = {
            let state = self.account.state.borrow();
            match state.modal_drafts.get(&files.custom_id).and_then(|d| d.state(&files.node)) {
                Some(FieldState::Files(ids)) => ids.clone(),
                _ => Vec::new(),
            }
        };
        let there = self.form_uploads(files.channel, &files.custom_id);
        let ids: Vec<String> = held.iter().filter(|id| there.iter().any(|(t, _)| t == *id)).cloned().collect();
        if ids.len() < held.len() {
            (files.set)(FieldState::Files(ids.clone()));
        }
        ids
    }

    /// Draw a file field: the area to add files (or that it is full), and a
    /// row per file. A field of one file that has it shows only that row.
    fn fill_files(self: &Rc<Self>, files: &Rc<FileField>) {
        let ids = self.file_ids(files);
        let there = self.form_uploads(files.channel, &files.custom_id);
        let uploads: Vec<Rc<Upload>> =
            ids.iter().filter_map(|id| there.iter().find(|(t, _)| t == id).map(|(_, u)| u.clone())).collect();
        let holder = &files.holder;
        while let Some(child) = holder.first_child() {
            holder.remove(&child);
        }
        files.dropping.take();
        if !(files.field.max == Some(1) && uploads.len() == 1) {
            if files.field.full(ids.len()) {
                holder.append(&gtk::Label::builder().label(modal::FILES_FULL).xalign(0.0).wrap(true).build());
                holder.append(&caption(modal::FILES_FULL_HINT));
            } else {
                let (words, link) = files.field.drop_words();
                let area = gtk::Label::builder()
                    .use_markup(true)
                    .label(format!("{}<a href=\"browse\">{link}</a>", glib::markup_escape_text(words)))
                    .xalign(0.0)
                    .wrap(true)
                    .build();
                area.connect_activate_link({
                    let (this, files) = (Rc::downgrade(self), Rc::downgrade(files));
                    move |_, _| {
                        if let (Some(this), Some(files)) = (this.upgrade(), files.upgrade()) {
                            this.browse_files(&files);
                        }
                        glib::Propagation::Stop
                    }
                });
                let max_size = {
                    let state = self.account.state.borrow();
                    state.limits().uploads(state.find_channel(files.channel).and_then(|(guild, _)| guild)).file_size
                };
                let dropping = gtk::Label::builder().label("Drop here to upload").xalign(0.0).visible(false).build();
                holder.append(&area);
                holder.append(&caption(&files.field.hint(max_size)));
                holder.append(&dropping);
                files.dropping.replace(Some(dropping));
            }
        }
        for upload in uploads {
            let row = upload.row();
            if row.parent().is_some() {
                row.unparent();
            }
            holder.append(row);
        }
    }

    /// The area's drop target: files dropped on it are added, except on a
    /// field of one file that has it, which has no area.
    fn files_drop(self: &Rc<Self>, files: &Rc<FileField>) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let (this, weak) = (Rc::downgrade(self), Rc::downgrade(files));
        target.connect_accept({
            let (this, weak) = (this.clone(), weak.clone());
            move |_, _| match (this.upgrade(), weak.upgrade()) {
                (Some(this), Some(files)) => !(files.field.max == Some(1) && this.file_ids(&files).len() == 1),
                _ => false,
            }
        });
        let shown = |weak: &Weak<FileField>, visible: bool| {
            if let Some(label) = weak.upgrade().and_then(|f| f.dropping.borrow().clone()) {
                label.set_visible(visible);
            }
        };
        target.connect_enter({
            let weak = weak.clone();
            move |_, _, _| {
                shown(&weak, true);
                gdk::DragAction::COPY
            }
        });
        target.connect_leave({
            let weak = weak.clone();
            move |_| shown(&weak, false)
        });
        target.connect_drop(move |_, value, _, _| {
            shown(&weak, false);
            let (Some(this), Some(files), Ok(list)) = (this.upgrade(), weak.upgrade(), value.get::<gdk::FileList>())
            else {
                return false;
            };
            let paths = list.files();
            glib::spawn_future_local(async move { this.add_form_files(&files, paths).await });
            true
        });
        target
    }

    /// The file picker, filtered to what the field takes, for one file or
    /// several as its `max_values` allows.
    fn browse_files(self: &Rc<Self>, files: &Rc<FileField>) {
        let dialog = gtk::FileDialog::builder().title("Upload a File").build();
        let extensions = uploads::extensions(&files.field.file_types);
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
        let (this, files) = (self.clone(), files.clone());
        glib::spawn_future_local(async move {
            let picked = if files.field.picks_several() {
                dialog
                    .open_multiple_future(Some(&this.window))
                    .await
                    .map(|list| list.iter::<gio::File>().flatten().collect())
            } else {
                dialog.open_future(Some(&this.window)).await.map(|file| vec![file])
            };
            if let Ok(paths) = picked {
                this.add_form_files(&files, paths).await;
            }
        });
    }

    /// Add files to a field as `T` in 232430's `eK` does: refused all
    /// together for the count, a size or a type, else each starts
    /// uploading at once, never converted.
    async fn add_form_files(self: &Rc<Self>, files: &Rc<FileField>, paths: Vec<gio::File>) {
        let mut added = Vec::new();
        for path in paths {
            added.extend(file_at(path).await);
        }
        if added.is_empty() {
            return;
        }
        let mut ids = self.file_ids(files);
        let limits = {
            let state = self.account.state.borrow();
            state.limits().uploads(state.find_channel(files.channel).and_then(|(guild, _)| guild))
        };
        let named: Vec<(&str, u64)> = added.iter().map(|f| (f.filename.as_str(), f.size)).collect();
        if let Err(refusal) = files.field.check(&limits, ids.len(), &named) {
            let alert = adw::AlertDialog::new(Some(refusal.heading()), Some(&refusal.body()));
            alert.add_response("okay", "_Okay");
            alert.present(Some(&files.holder));
            return;
        }
        for file in added {
            let id = uploads::unique_id();
            let removed = {
                let (this, weak, id) = (Rc::downgrade(self), Rc::downgrade(files), id.clone());
                move || {
                    if let (Some(this), Some(files)) = (this.upgrade(), weak.upgrade()) {
                        this.remove_form_file(&files, &id);
                    }
                }
            };
            let upload = self.plain_upload(files.channel, file, modal::REMOVE_FILE, removed);
            let channel = files.channel;
            let entry = FormUpload { channel, custom_id: files.custom_id.clone(), id: id.clone(), upload };
            self.drafts.forms.borrow_mut().push(entry);
            ids.push(id);
        }
        (files.set)(FieldState::Files(ids));
        self.fill_files(files);
    }

    /// Take a file back (`removeFromMsgDraft`): its upload stops, or is
    /// deleted if it had finished.
    fn remove_form_file(self: &Rc<Self>, files: &Rc<FileField>, id: &str) {
        let gone = {
            let mut forms = self.drafts.forms.borrow_mut();
            let at = forms.iter().position(|f| f.id == id);
            at.map(|at| forms.remove(at))
        };
        if let Some(gone) = gone {
            gone.upload.cancel(self.account.state.borrow().rest.clone());
        }
        self.fill_files(files);
    }

    /// What a field calls with its new state: the draft keeps it (even
    /// when wrong) and the field is checked (911408 `executeStateUpdate`).
    fn modal_setter(self: &Rc<Self>, custom_id: &str, node: &Node, title: &Rc<Title>) -> impl Fn(FieldState) + 'static {
        let (this, custom_id, node, title) = (Rc::downgrade(self), custom_id.to_owned(), node.clone(), title.clone());
        move |state| {
            let Some(this) = this.upgrade() else { return };
            let error = {
                let mut account = this.account.state.borrow_mut();
                let draft = account.modal_drafts.entry(custom_id.clone()).or_default();
                draft.set(&node, state);
                modal::validate(&node, draft.state(&node))
            };
            title.show(error.as_deref());
        }
    }

    /// Submit (969508 `x`): every field is checked, then the form is sent
    /// as `nonce` and waits for Discord's answer, at most 10 s. Nothing goes
    /// while another form's submit is in flight.
    fn submit_modal(self: &Rc<Self>, form: &Rc<Form>) {
        form.error.set_visible(false);
        let mut first_wrong = None;
        for field in &form.fields {
            if !self.check_modal_field(&form.modal.custom_id, field) && first_wrong.is_none() {
                first_wrong = Some(field.focus.clone());
            }
        }
        if let Some(widget) = first_wrong {
            widget.grab_focus();
            return;
        }
        let modal = &form.modal;
        let (ids, files): (Vec<String>, Vec<Rc<Upload>>) =
            self.form_uploads(modal.channel, &modal.custom_id).into_iter().unzip();
        let (rest, session, guild, components, free) = {
            let state = self.account.state.borrow();
            let draft = state.modal_drafts.get(&modal.custom_id).cloned().unwrap_or_default();
            let channel = state.find_channel(modal.channel);
            (
                state.rest.clone(),
                state.session_id.clone(),
                channel.map(|(guild, _)| guild.map(|g| g.id)),
                modal::components(&modal.components, &draft, &ids),
                state.modal_submission.in_flight().is_none(),
            )
        };
        if !free {
            return;
        }
        // A part it can't send, a channel we don't know or no session: the
        // client throws before posting, and so nothing goes.
        let (Some(rest), Some(guild), Ok(components)) = (rest, guild, components) else {
            form.error.set_visible(true);
            return;
        };
        if session.is_empty() {
            form.error.set_visible(true);
            return;
        }
        let nonce = Snowflake::now();
        self.account.state.borrow_mut().modal_submission.start(nonce);
        form.nonce.set(Some(nonce));
        self.refresh_modal_forms();
        let this = Rc::downgrade(self);
        self.modal_timer(nonce, if files.is_empty() { SUBMIT_TIMEOUT } else { UPLOAD_TIMEOUT });
        let (application, modal_id, custom_id, channel) =
            (modal.application.id, modal.id, modal.custom_id.clone(), modal.channel);
        let modal_components = modal.components.clone();
        let task = glib::spawn_future_local({
            let form = Rc::downgrade(form);
            async move {
                // With files, the form waits for them first (969508's
                // preflight): one failing fails the submit, and nothing is
                // posted. What is sent is read once they are up.
                let (components, attachments, session) = if files.is_empty() {
                    (components, Vec::new(), session)
                } else {
                    let uploaded = staged(&files).await;
                    let Some(app) = this.upgrade() else { return };
                    let Ok(uploaded) = uploaded else {
                        if let Some(form) = form.upgrade() {
                            form.task.take();
                        }
                        if app.account.state.borrow_mut().modal_submission.answered(nonce, false) {
                            app.modal_ended(nonce, false);
                        }
                        return;
                    };
                    app.modal_timer(nonce, SUBMIT_TIMEOUT);
                    let details = Details::of(&files);
                    let attachments =
                        details.sent(&files, &uploaded).iter().enumerate().map(|(at, s)| s.body(at)).collect();
                    let state = app.account.state.borrow();
                    let draft = state.modal_drafts.get(&custom_id).cloned().unwrap_or_default();
                    let components = modal::components(&modal_components, &draft, &ids).unwrap_or(components);
                    (components, attachments, state.session_id.clone())
                };
                let body = Submit {
                    application,
                    channel,
                    guild,
                    session: &session,
                    modal: modal_id,
                    custom_id: &custom_id,
                    components,
                    attachments,
                }
                .body(nonce);
                let in_flight = {
                    let this = this.clone();
                    move || {
                        this.upgrade()
                            .is_some_and(|this| this.account.state.borrow().modal_submission.in_flight() == Some(nonce))
                    }
                };
                let sent = rest.submit_modal(body, in_flight).await;
                if let Some(form) = form.upgrade() {
                    form.task.take();
                }
                if sent.is_err()
                    && let Some(this) = this.upgrade()
                    && this.account.state.borrow_mut().modal_submission.answered(nonce, false)
                {
                    this.modal_ended(nonce, false);
                }
            }
        });
        *form.task.borrow_mut() = Some(task);
    }

    /// Fail the submit sent as `nonce` after `after`, if it is still in
    /// flight then (750128's timers).
    fn modal_timer(self: &Rc<Self>, nonce: Snowflake, after: Duration) {
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(after, move || {
            if let Some(this) = this.upgrade()
                && this.account.state.borrow_mut().modal_submission.timed_out(nonce)
            {
                this.modal_ended(nonce, false);
            }
        });
    }

    /// Discord answered `nonce`. Whether it was the form submit in flight:
    /// then nothing else of ours waits on it. Only `INTERACTION_SUCCESS` and
    /// `INTERACTION_FAILURE` end it, never the app's reply message.
    pub(super) fn modal_answered(self: &Rc<Self>, nonce: Snowflake, outcome: Outcome) -> bool {
        if outcome == Outcome::Form {
            return false;
        }
        let ok = outcome == Outcome::Success;
        if !self.account.state.borrow_mut().modal_submission.answered(nonce, ok) {
            return false;
        }
        self.modal_ended(nonce, ok);
        true
    }

    /// The submit sent as `nonce` ended. Its form, if still open, closes on
    /// a success, forgets what was typed (`CLEAR_INTERACTION_MODAL_STATE`)
    /// and deletes its files (`removeFiles`), or says it went wrong and may
    /// be sent again. A success with the form closed keeps both, as the
    /// client clears them only from the form.
    fn modal_ended(self: &Rc<Self>, nonce: Snowflake, ok: bool) {
        let form = self.modal_forms.borrow().iter().find(|f| f.nonce.get() == Some(nonce)).cloned();
        if let Some(form) = form {
            if ok {
                let modal = &form.modal;
                self.account.state.borrow_mut().modal_drafts.remove(&modal.custom_id);
                let gone: Vec<FormUpload> = {
                    let mut forms = self.drafts.forms.borrow_mut();
                    let (gone, kept) =
                        forms.drain(..).partition(|f| f.channel == modal.channel && f.custom_id == modal.custom_id);
                    *forms = kept;
                    gone
                };
                let rest = self.account.state.borrow().rest.clone();
                for file in gone {
                    file.upload.cancel(rest.clone());
                }
                form.dialog.force_close();
            } else {
                form.error.set_visible(true);
            }
        }
        self.refresh_modal_forms();
    }

    /// The form closed: a request still on its way is aborted, which fails
    /// its submit at once (the client's abort lands in its `.catch`).
    fn modal_closed(self: &Rc<Self>, form: &Rc<Form>) {
        self.modal_forms.borrow_mut().retain(|f| !Rc::ptr_eq(f, form));
        if let Some(task) = form.task.take() {
            task.abort();
            if let Some(nonce) = form.nonce.get()
                && self.account.state.borrow_mut().modal_submission.answered(nonce, false)
            {
                self.modal_ended(nonce, false);
            }
        }
        self.refresh_modal_forms();
    }

    /// Submit waits in every form while one is in flight, and spins in the
    /// form that sent it.
    fn refresh_modal_forms(&self) {
        let in_flight = self.account.state.borrow().modal_submission.in_flight();
        for form in self.modal_forms.borrow().iter() {
            let ours = in_flight.is_some() && form.nonce.get() == in_flight;
            form.submit.set_sensitive(in_flight.is_none());
            form.submit_face.set_visible_child_name(if ours { "spinner" } else { "label" });
        }
    }

    /// Leaving the account closes its forms; what they held goes with it.
    pub(super) fn close_modal_forms(&self) {
        let forms: Vec<Rc<Form>> = self.modal_forms.borrow_mut().drain(..).collect();
        for form in forms {
            if let Some(task) = form.task.take() {
                task.abort();
            }
            form.dialog.force_close();
        }
    }
}
