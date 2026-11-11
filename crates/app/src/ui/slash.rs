//! Slash commands: the suggestions after a `/`, the form for a command's
//! options, and running it. What each sends and when comes from the client's
//! command code; see `discord::commands`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use discord::commands::{
    self, App as CommandApp, Autocomplete, Command, CommandOption, Names, Run, command_type, option_type,
};
use discord::{Snowflake, interaction::Failure};
use gtk::glib;

use super::App;
use super::composer::Completion;

/// How long typing in an option waits before asking its app for choices
/// again (`debounce`, 500 ms leading and trailing, in 496533).
const AUTOCOMPLETE_WAIT: Duration = Duration::from_millis(500);

/// A command we ran that its app hasn't answered.
#[derive(Clone, Debug)]
pub(super) struct Running {
    pub(super) channel: Snowflake,
    /// The app's name, for "is thinking…".
    pub(super) app: String,
    /// Discord took it (`INTERACTION_CREATE`).
    pub(super) created: bool,
}

/// A command in the suggestions.
pub(super) struct Suggestion {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) app: String,
}

/// The widgets for slash commands, around the message box.
pub(super) struct Slash {
    /// Above the message box while a command's options are filled in.
    bar: gtk::Box,
    title: gtk::Label,
    description: gtk::Label,
    fields: gtk::Box,
    run: gtk::Button,
    /// What running commands are doing, in the open channel.
    status: gtk::Label,
    active: RefCell<Option<Active>>,
}

/// The command being filled in.
struct Active {
    channel: Snowflake,
    command: Command,
    app: CommandApp,
    fields: Vec<Field>,
    autocomplete: Autocomplete,
}

struct Field {
    option: CommandOption,
    widget: gtk::Widget,
    read: Rc<dyn Fn() -> String>,
    error: gtk::Label,
    choices: Option<Choices>,
}

/// An option with an app's choices to pick from as it is typed in.
struct Choices {
    popover: gtk::Popover,
    list: gtk::ListBox,
    /// The names shown, row-aligned.
    shown: RefCell<Vec<String>>,
    entry: gtk::Entry,
    /// The entry has the keyboard focus.
    focused: Rc<Cell<bool>>,
}

impl Slash {
    pub(super) fn new() -> Self {
        Slash {
            bar: gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).visible(false).build(),
            title: gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["heading"]).build(),
            description: gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label", "caption"]).build(),
            fields: gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build(),
            run: gtk::Button::builder().label("_Run").use_underline(true).css_classes(["suggested-action"]).build(),
            status: gtk::Label::builder().xalign(0.0).visible(false).css_classes(["dim-label"]).build(),
            active: RefCell::default(),
        }
    }

    /// Whether a command is being filled in.
    pub(super) fn is_active(&self) -> bool {
        self.active.borrow().is_some()
    }
}

/// Who and what a name in an option can mean, from what the app knows.
struct Resolver<'a> {
    state: &'a super::state::State,
    channel: Snowflake,
}

impl Resolver<'_> {
    fn server(&self) -> Option<&discord::Guild> {
        self.state.find_channel(self.channel).and_then(|(guild, _)| guild)
    }
}

impl Names for Resolver<'_> {
    fn user(&self, name: &str) -> Option<Snowflake> {
        let mut found = self.state.users.iter().filter(|(_, known)| known.as_str() == name);
        let first = found.next()?;
        found.next().is_none().then_some(Snowflake(*first.0))
    }

    fn role(&self, name: &str) -> Option<Snowflake> {
        self.server()?.roles.iter().find(|r| r.name == name).map(|r| r.id)
    }

    fn channel(&self, name: &str) -> Option<Snowflake> {
        let guild = self.server()?;
        let named = |c: &&discord::Channel| c.name.as_deref() == Some(name);
        let listed = guild.channels.iter().filter(|c| c.kind != discord::ChannelKind::Category);
        listed.chain(&guild.threads).find(named).map(|c| c.id)
    }

    fn guild(&self) -> Option<Snowflake> {
        self.server().map(|g| g.id)
    }
}

impl App {
    /// The suggestions area and the command's form, in the message area.
    pub(super) fn build_slash(self: &Rc<Self>, area: &gtk::Box) {
        let slash = &self.slash;
        area.append(&slash.status);
        let header = gtk::Box::builder().spacing(8).build();
        header.append(&slash.title);
        let cancel = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Cancel command")
            .css_classes(["flat"])
            .build();
        cancel.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.cancel_command();
                }
            }
        });
        header.append(&cancel);
        slash.bar.append(&header);
        slash.bar.append(&slash.description);
        slash.bar.append(&slash.fields);
        slash.run.set_halign(gtk::Align::End);
        slash.run.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.run_command();
                }
            }
        });
        slash.bar.append(&slash.run);
        // Escape puts the form away from any option.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let this = Rc::downgrade(self);
            move |_, key, _, _| match this.upgrade() {
                Some(this) if key == gtk::gdk::Key::Escape && this.slash.is_active() => {
                    this.cancel_command();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        slash.bar.add_controller(keys);
        area.append(&slash.bar);
    }

    /// Show the suggestions for the command being typed after a `/` at the
    /// start of the message box, fetching the apps and commands first if the
    /// client would. Returns whether the box holds a command.
    pub(super) fn update_slash_popup(self: &Rc<Self>) -> bool {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let Some(typed) = text.strip_prefix('/') else { return false };
        if self.editing().is_some() || self.slash.is_active() {
            return false;
        }
        let Some(channel) = self.state.borrow().channel else { return false };
        let Some(place) = self.state.borrow().command_place(channel) else { return false };
        self.fetch_command_indexes(&place);
        let (query, terminated) = commands::query_of(typed);
        let suggestions: Vec<Suggestion> = {
            let state = self.state.borrow();
            let found = state.commands.find(&place, command_type::CHAT, &query);
            commands::settled(found, &query, terminated)
                .into_iter()
                .take(commands::SUGGESTIONS)
                .map(|f| Suggestion {
                    id: f.command.id.clone(),
                    name: format!("/{}", f.command.display_name),
                    description: f.command.display_description.clone(),
                    app: f.app.name.clone(),
                })
                .collect()
        };
        self.show_completions(0, suggestions.into_iter().map(Completion::Command).collect());
        true
    }

    /// Ask for the indexes the suggestions need (`query` with `allowFetch`
    /// in 264322): each once, until Discord says it changed.
    fn fetch_command_indexes(self: &Rc<Self>, place: &commands::Place) {
        let (rest, me) = {
            let state = self.state.borrow();
            (state.rest.clone(), state.me())
        };
        let (Some(rest), Some(me)) = (rest, me) else { return };
        let targets = self.state.borrow_mut().commands.to_fetch(place, Instant::now());
        for target in targets {
            let (this, rest) = (self.clone(), rest.clone());
            glib::spawn_future_local(async move {
                let index = rest.command_index(target).await;
                {
                    let mut state = this.state.borrow_mut();
                    match &index {
                        Some(index) => state.commands.loaded(target, index, me),
                        None => state.commands.failed(target, Instant::now()),
                    }
                }
                // The box may have moved on while the index came.
                this.update_slash_popup();
            });
        }
    }

    /// Fill in `id`, a suggestion, in the form.
    pub(super) fn start_command(self: &Rc<Self>, id: &str) {
        let Some(channel) = self.state.borrow().channel else { return };
        let found = {
            let state = self.state.borrow();
            let place = state.command_place(channel);
            place.and_then(|p| state.commands.get(&p, command_type::CHAT, id).map(|(c, a)| (c.clone(), a.clone())))
        };
        let Some((command, app)) = found else { return };
        self.set_composer_text("");
        self.mention_popover.popdown();
        let slash = &self.slash;
        slash.title.set_label(&format!("/{}", command.display_name));
        slash.description.set_label(&format!("{} — {}", app.name, command.display_description));
        while let Some(child) = slash.fields.first_child() {
            slash.fields.remove(&child);
        }
        let mut options: Vec<&CommandOption> = command
            .options
            .iter()
            .filter(|o| !matches!(o.kind, option_type::SUB_COMMAND | option_type::SUB_COMMAND_GROUP))
            .collect();
        // What the command needs comes first.
        options.sort_by_key(|o| !o.required);
        let fields: Vec<Field> = options.into_iter().map(|o| self.build_field(o)).collect();
        for field in &fields {
            let row = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
            let required = if field.option.required { " *" } else { "" };
            let label = gtk::Label::builder()
                .label(format!("{}{required}", field.option.display_name))
                .xalign(0.0)
                .tooltip_text(field.option.description.as_str())
                .build();
            row.append(&label);
            row.append(&field.widget);
            row.append(&field.error);
            slash.fields.append(&row);
        }
        let attachments = command.needs_attachment();
        slash.run.set_sensitive(!attachments);
        if attachments {
            slash.description.set_label(&format!(
                "{} — This command needs a file, which Neucord can't attach to a command yet.",
                app.name
            ));
        }
        let first = fields.first().map(|f| f.widget.clone());
        slash.active.replace(Some(Active { channel, command, app, fields, autocomplete: Autocomplete::default() }));
        slash.bar.set_visible(true);
        self.composer.set_sensitive(false);
        if let Some(first) = first {
            first.grab_focus();
        } else {
            slash.run.grab_focus();
        }
    }

    /// Put the form away without running the command.
    pub(super) fn cancel_command(self: &Rc<Self>) {
        self.slash.active.take();
        self.slash.bar.set_visible(false);
        self.composer.set_sensitive(true);
        self.composer.grab_focus();
    }

    /// A widget for one option.
    fn build_field(self: &Rc<Self>, option: &CommandOption) -> Field {
        let error = gtk::Label::builder().xalign(0.0).visible(false).css_classes(["error", "caption"]).build();
        let weak = Rc::downgrade(self);
        let choose = |names: Vec<String>| {
            let mut all = vec![String::new()];
            all.extend(names);
            let refs: Vec<&str> = all.iter().map(String::as_str).collect();
            let dropdown = gtk::DropDown::from_strings(&refs);
            let read: Rc<dyn Fn() -> String> = {
                let dropdown = dropdown.clone();
                Rc::new(move || all.get(dropdown.selected() as usize).cloned().unwrap_or_default())
            };
            (dropdown.upcast::<gtk::Widget>(), read)
        };
        if let Some(choices) = &option.choices {
            let (widget, read) = choose(choices.iter().map(|c| c.name.clone()).collect());
            return Field { option: option.clone(), widget, read, error, choices: None };
        }
        if option.kind == option_type::BOOLEAN {
            let (widget, read) = choose(vec!["True".into(), "False".into()]);
            return Field { option: option.clone(), widget, read, error, choices: None };
        }
        let entry = gtk::Entry::builder().placeholder_text(placeholder(option)).activates_default(false).build();
        entry.connect_activate({
            let weak = weak.clone();
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.run_command();
                }
            }
        });
        let read: Rc<dyn Fn() -> String> = {
            let entry = entry.clone();
            Rc::new(move || entry.text().to_string())
        };
        let choices = option.autocomplete.then(|| {
            let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
            let popover = gtk::Popover::builder().autohide(false).has_arrow(false).child(&list).build();
            popover.set_parent(&entry);
            let choices =
                Choices { popover, list, shown: RefCell::default(), entry: entry.clone(), focused: Rc::default() };
            self.autocomplete_wiring(option, &choices);
            choices
        });
        Field { option: option.clone(), widget: entry.upcast(), read, error, choices }
    }

    /// Ask the app for choices as the entry is typed in, and pick one from
    /// the list.
    fn autocomplete_wiring(self: &Rc<Self>, option: &CommandOption, choices: &Choices) {
        let (name, entry) = (option.name.clone(), choices.entry.clone());
        let debounce = Rc::new(Debounce::default());
        let ask: Rc<dyn Fn()> = Rc::new({
            let (weak, name) = (Rc::downgrade(self), name.clone());
            move || {
                if let Some(this) = weak.upgrade() {
                    this.ask_autocomplete(&name);
                }
            }
        });
        entry.connect_changed({
            let (debounce, ask) = (debounce.clone(), ask.clone());
            move |_| debounce.poke(&ask)
        });
        // An option being filled in asks for its choices at once, for what
        // it holds so far.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter({
            let (debounce, ask, focused) = (debounce.clone(), ask.clone(), choices.focused.clone());
            move |_| {
                focused.set(true);
                debounce.poke(&ask);
            }
        });
        focus.connect_leave({
            let (popover, focused) = (choices.popover.clone(), choices.focused.clone());
            move |_| {
                focused.set(false);
                popover.popdown();
            }
        });
        entry.add_controller(focus);
        choices.list.connect_row_activated({
            let weak = Rc::downgrade(self);
            let name = name.clone();
            move |_, row| {
                if let Some(this) = weak.upgrade() {
                    this.pick_choice(&name, row.index());
                }
            }
        });
    }

    /// Ask the app for the choices to `option` for what it holds, if they
    /// aren't known (`WL` in 721768).
    fn ask_autocomplete(self: &Rc<Self>, option: &str) {
        enum Ask {
            Nothing,
            Known,
            Send(discord::Rest, serde_json::Value, Snowflake),
        }
        let ask = {
            let mut slot = self.slash.active.borrow_mut();
            let Some(active) = slot.as_mut() else { return };
            let values = active.values();
            let state = self.state.borrow();
            let names = Resolver { state: &state, channel: active.channel };
            let options =
                commands::run_options(&active.command, &values, &names, active.autocomplete.answers(), Some(option));
            match (options, state.rest.clone()) {
                (Ok(options), Some(rest)) => {
                    let query = values.get(option).map(|v| v.trim().to_owned()).unwrap_or_default();
                    let nonce = Snowflake::now();
                    if active.autocomplete.request(nonce, option, &query, &commands::context_key(&options)) {
                        let guild = state.find_channel(active.channel).and_then(|(guild, _)| guild.map(|g| g.id));
                        let run = Run {
                            application: active.command.application,
                            guild,
                            channel: active.channel,
                            session: &state.session_id,
                            data: active.command.data(options, None),
                            nonce,
                        };
                        Ask::Send(rest, run.autocomplete_body(), nonce)
                    } else {
                        Ask::Known
                    }
                }
                _ => Ask::Nothing,
            }
        };
        match ask {
            Ask::Nothing => {}
            Ask::Known => self.show_choices(option),
            Ask::Send(rest, body, nonce) => {
                let (this, option) = (self.clone(), option.to_owned());
                glib::spawn_future_local(async move {
                    // The request failing is the app failing, as the client's `INTERACTION_FAILURE` has it.
                    if rest.autocomplete(body).await.is_err() {
                        this.autocomplete_failed(nonce);
                    }
                    this.show_choices(&option);
                });
            }
        }
    }

    /// The request `nonce` for choices failed; the option it was for, if it
    /// was ours.
    fn autocomplete_failed(self: &Rc<Self>, nonce: Snowflake) -> Option<String> {
        self.slash.active.borrow_mut().as_mut()?.autocomplete.failed(nonce)
    }

    /// An app's choices arrived.
    pub(super) fn on_autocomplete(self: &Rc<Self>, nonce: Snowflake, choices: &serde_json::Value) {
        let option = {
            let mut slot = self.slash.active.borrow_mut();
            let Some(active) = slot.as_mut() else { return };
            let integer: Vec<(String, bool)> =
                active.command.options.iter().map(|o| (o.name.clone(), o.kind == option_type::INTEGER)).collect();
            let whole = move |name: &str| integer.iter().any(|(n, whole)| n == name && *whole);
            active.autocomplete.answer(nonce, choices, whole)
        };
        if let Some(option) = option {
            self.show_choices(&option);
        }
    }

    /// Draw the list of choices under the option, from what the app last
    /// answered.
    fn show_choices(self: &Rc<Self>, option: &str) {
        // Widgets can call back into the form, so none is touched while it is borrowed.
        let (list, popover, focused, shown, errored) = {
            let slot = self.slash.active.borrow();
            let Some(active) = slot.as_ref() else { return };
            let Some(field) = active.fields.iter().find(|f| f.option.name == option) else { return };
            let Some(choices) = &field.choices else { return };
            let query = (field.read)().trim().to_owned();
            let answers = active.autocomplete.answers();
            let list = answers.choices(option, &query).unwrap_or_else(|| answers.latest(option));
            let shown: Vec<String> = list.iter().map(|c| c.name.clone()).collect();
            *choices.shown.borrow_mut() = shown.clone();
            (choices.list.clone(), choices.popover.clone(), choices.focused.get(), shown, active.autocomplete.errored)
        };
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        if errored && shown.is_empty() {
            list.append(&gtk::Label::builder().label("Couldn't load choices").sensitive(false).build());
            popover.popup();
            return;
        }
        for name in &shown {
            list.append(&gtk::Label::builder().label(name.as_str()).xalign(0.0).build());
        }
        if shown.is_empty() || !focused {
            popover.popdown();
        } else {
            popover.popup();
        }
    }

    /// The user picked a row of the choices list.
    fn pick_choice(self: &Rc<Self>, option: &str, row: i32) {
        let picked = {
            let slot = self.slash.active.borrow();
            let Some(active) = slot.as_ref() else { return };
            let Some(field) = active.fields.iter().find(|f| f.option.name == option) else { return };
            let Some(choices) = &field.choices else { return };
            let name = choices.shown.borrow().get(row.max(0) as usize).cloned();
            (choices.entry.clone(), choices.popover.clone(), name)
        };
        let (entry, popover, name) = picked;
        if let Some(name) = name {
            entry.set_text(&name);
            entry.set_position(-1);
        }
        popover.popdown();
    }

    /// Check the form and run the command (`A` in 545152 and `handleCommand`
    /// in 870801).
    pub(super) fn run_command(self: &Rc<Self>) {
        enum Prepared {
            Nothing,
            /// The first option that isn't right.
            Invalid(gtk::Widget),
            Error(String),
            NoticeBar,
            Go {
                channel: Snowflake,
                command: Box<Command>,
                app: CommandApp,
                body: serde_json::Value,
                nonce: Snowflake,
            },
        }
        let prepared = {
            let slot = self.slash.active.borrow();
            let Some(active) = slot.as_ref() else { return };
            let state = self.state.borrow();
            let names = Resolver { state: &state, channel: active.channel };
            let values = active.values();
            let mut invalid: Option<gtk::Widget> = None;
            for field in &active.fields {
                let typed = values.get(&field.option.name).map(String::as_str).unwrap_or_default();
                match commands::validate(&field.option, typed, &names, active.autocomplete.answers()) {
                    Ok(()) => field.error.set_visible(false),
                    Err(why) => {
                        invalid.get_or_insert_with(|| field.widget.clone());
                        field.error.set_label(&why);
                        field.error.set_visible(true);
                    }
                }
            }
            let options = commands::run_options(&active.command, &values, &names, active.autocomplete.answers(), None);
            if let Some(first) = invalid {
                Prepared::Invalid(first)
            } else if active.command.needs_attachment() || state.session_id.is_empty() {
                Prepared::Nothing
            } else if !state.can_send(active.channel) {
                Prepared::NoticeBar
            } else {
                match options {
                    Err(why) => Prepared::Error(why),
                    Ok(options) => {
                        let guild = state.find_channel(active.channel).and_then(|(guild, _)| guild.map(|g| g.id));
                        let nonce = Snowflake::now();
                        let run = Run {
                            application: active.command.application,
                            guild,
                            channel: active.channel,
                            session: &state.session_id,
                            data: active.command.data(options, None),
                            nonce,
                        };
                        Prepared::Go {
                            channel: active.channel,
                            command: Box::new(active.command.clone()),
                            app: active.app.clone(),
                            body: run.body(),
                            nonce,
                        }
                    }
                }
            }
        };
        let (channel, command, app, body, nonce) = match prepared {
            Prepared::Nothing => return,
            // Focus moves outside the borrow: an option asks its app for choices when it gets it.
            Prepared::Invalid(first) => {
                first.grab_focus();
                return;
            }
            Prepared::Error(why) => return self.show_composer_error(&why),
            Prepared::NoticeBar => return self.update_notice_bar(),
            Prepared::Go { channel, command, app, body, nonce } => (channel, command, app, body, nonce),
        };
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest else { return };
        self.cancel_command();
        self.state.borrow_mut().running.insert(nonce, Running { channel, app: app.name.clone(), created: false });
        self.refresh_command_status();
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(why) = this.reopen(&rest, channel).await {
                this.state.borrow_mut().running.remove(&nonce);
                this.refresh_command_status();
                return this.show_composer_error(&format!("Not sent: {why}"));
            }
            if let Err(failure) = rest.run_command(body).await {
                this.command_refused(nonce, &command, &failure);
            }
        });
        true
    }

    /// Discord turned the request down.
    fn command_refused(self: &Rc<Self>, nonce: Snowflake, command: &Command, failure: &Failure) {
        let Some(running) = self.state.borrow_mut().running.remove(&nonce) else { return };
        if failure.stale_command {
            self.state.borrow_mut().commands_out_of_date(command.application, running.channel);
        }
        self.refresh_command_status();
        let text = match (&failure.message, failure.is_announced()) {
            (Some(message), _) => message.clone(),
            (None, true) => "Your message could not be delivered.".to_owned(),
            (None, false) => "This interaction failed".to_owned(),
        };
        self.show_composer_error(&text);
    }

    /// Say what the commands running in the open channel are doing.
    pub(super) fn refresh_command_status(&self) {
        let state = self.state.borrow();
        let open = state.channel;
        let latest = state.running.iter().filter(|(_, r)| Some(r.channel) == open).max_by_key(|(nonce, _)| **nonce);
        let text =
            latest.map(|(_, r)| if r.created { format!("{} is thinking…", r.app) } else { "Sending command…".into() });
        self.slash.status.set_visible(text.is_some());
        self.slash.status.set_label(text.as_deref().unwrap_or_default());
    }

    /// An interaction of ours is over, answered or not. Whether it was a
    /// command's.
    pub(super) fn command_answered(
        self: &Rc<Self>,
        nonce: Snowflake,
        outcome: discord::gateway::Outcome,
        reason: Option<u64>,
    ) -> bool {
        use discord::gateway::Outcome;
        if outcome == Outcome::Failure
            && let Some(option) = self.autocomplete_failed(nonce)
        {
            self.show_choices(&option);
            return true;
        }
        let Some(running) = self.state.borrow_mut().running.remove(&nonce) else { return false };
        self.refresh_command_status();
        match outcome {
            Outcome::Success => {}
            Outcome::Form => self.show_composer_error("This app wants a form filled in, which Neucord can't show yet."),
            Outcome::Failure => {
                let text = commands::reason_text(reason, Some(&running.app))
                    .unwrap_or_else(|| "This interaction failed".to_owned());
                self.show_composer_error(&text);
            }
        }
        true
    }

    /// Discord took the command (`INTERACTION_CREATE`).
    pub(super) fn command_created(self: &Rc<Self>, nonce: Snowflake) {
        if let Some(running) = self.state.borrow_mut().running.get_mut(&nonce) {
            running.created = true;
        }
        self.refresh_command_status();
    }

    /// The server's apps or commands changed.
    pub(super) fn command_index_changed(self: &Rc<Self>, guild: Snowflake, version: Option<String>) {
        let mut state = self.state.borrow_mut();
        let bots = state.commands.guild_changed(guild, version);
        let channels: Vec<Snowflake> = state
            .private_channels
            .iter()
            .filter(|c| {
                c.kind == discord::ChannelKind::Dm && c.recipients.first().is_some_and(|u| bots.contains(&u.id))
            })
            .map(|c| c.id)
            .collect();
        for channel in channels {
            state.commands.channel_stale(channel);
        }
    }
}

/// Asks at the first change and again at the end of the wait if there were
/// more (500 ms, leading and trailing).
#[derive(Default)]
struct Debounce {
    waiting: Cell<bool>,
    pending: Cell<bool>,
}

impl Debounce {
    fn poke(self: &Rc<Self>, ask: &Rc<dyn Fn()>) {
        if self.waiting.get() {
            self.pending.set(true);
            return;
        }
        ask();
        self.waiting.set(true);
        let (this, ask) = (self.clone(), ask.clone());
        glib::timeout_add_local_once(AUTOCOMPLETE_WAIT, move || {
            this.waiting.set(false);
            if this.pending.take() {
                this.poke(&ask);
            }
        });
    }
}

impl Active {
    /// What the form holds, by option, leaving out what is empty.
    fn values(&self) -> BTreeMap<String, String> {
        self.fields
            .iter()
            .filter_map(|f| {
                let text = (f.read)();
                (!text.trim().is_empty()).then(|| (f.option.name.clone(), text))
            })
            .collect()
    }
}

/// What an option's entry says before anything is typed.
fn placeholder(option: &CommandOption) -> &'static str {
    match option.kind {
        option_type::USER => "@name, or an id",
        option_type::ROLE => "@role, or an id",
        option_type::MENTIONABLE => "@name or @role, or an id",
        option_type::CHANNEL => "#channel, or an id",
        option_type::INTEGER => "A whole number",
        option_type::NUMBER => "A number",
        _ => "",
    }
}
