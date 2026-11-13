//! A command in the message box, as the client holds it: its name, then a
//! pill for each option given (870748, 84046 `em`). The box's text is the
//! line's plain form, `/roll count:3 size:D6`, with tags for what
//! `discord::command_line` needs to read it back: the name, each key
//! (`count:`, which can't be edited) and the space that ends each pill.
//! Everything that decides is there; this reads the tags, applies what
//! comes back, and draws the rest: the look of the pills, the bar above the
//! box, "+2 options" after it, and the lists of the box's popup inside a
//! pill or between them.
//!
//! The line is read again at the end of each user action (typing, a
//! deletion, a paste), and after a change made outside one (an undo, the
//! box's text set) once the main loop is idle; the caret moving redraws
//! what depends on the pill it is in.

use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use adw::prelude::*;
use discord::Snowflake;
use discord::command_line::{self, Caret, Change, Edit, Header, Line, Marked, OptionStates, Part};
use discord::commands::{self, CommandOption, Focus, Pick, RunFrom, Selection, Values, option_type};
use discord::members::{PILL_SEARCH_LIMIT, SEARCH_WAIT};
use gtk::{glib, pango};

use super::App;
use super::composer::{Completion, MentionCtx};
use super::slash::{Active, Keep, Resolver};

/// The tags: the line's structure, then its look.
const NAME: &str = "command-name";
const KEY: &str = "pill-key";
const END: &str = "pill-end";
const VALUE: &str = "pill-value";
const ACTIVE: &str = "pill-active";
const ERROR: &str = "pill-error";

/// The most a pill's member, role or channel list shows (`queryMentionResults`
/// and `queryApplicationCommandChannelResults` in 997625).
const LISTED: usize = 10;

/// People, roles or channels, by id, with the names they show.
type Named = Vec<(Snowflake, String)>;

/// What the popup shows inside the line, and how its rows are picked.
struct Listing {
    /// Where the text a pick replaces starts.
    start: i32,
    candidates: Vec<Completion>,
    headers: Vec<(usize, Header)>,
    /// The list's type, for the selection (`OPTIONS`, `CHOICES`, …).
    kind: &'static str,
    focus: Focus,
    query: String,
    loading: bool,
    /// The pill a pick sets whole, if it does.
    pill: Option<String>,
}

fn char_offset(iter: &gtk::TextIter) -> usize {
    iter.offset().max(0) as usize
}

/// Where `tag` is in `buffer`.
fn ranges(buffer: &gtk::TextBuffer, tag: &gtk::TextTag) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut iter = buffer.start_iter();
    loop {
        if !iter.starts_tag(Some(tag)) && !iter.forward_to_tag_toggle(Some(tag)) {
            break;
        }
        let start = char_offset(&iter);
        let more = iter.forward_to_tag_toggle(Some(tag));
        found.push(start..char_offset(&iter));
        if !more {
            break;
        }
    }
    found
}

fn tag_of(part: Part) -> Option<&'static str> {
    match part {
        Part::Plain => None,
        Part::Name => Some(NAME),
        Part::Key => Some(KEY),
        Part::End => Some(END),
    }
}

/// The word being typed at `caret`, from `from` at the earliest: its start
/// and text.
fn word_at(chars: &[char], from: usize, caret: usize) -> (usize, String) {
    let caret = caret.min(chars.len());
    let from = from.min(caret);
    let start = chars[from..caret].iter().rposition(|c| c.is_whitespace()).map_or(from, |at| from + at + 1);
    (start, chars[start..caret].iter().collect())
}

/// Whether a pill's value is picked from a list as a whole (choices, the
/// app's choices, true or false, a mention or a channel), rather than typed
/// as text with the box's `@` and `:` lists.
fn picked_whole(option: &CommandOption) -> bool {
    use option_type::*;
    option.choices.as_ref().is_some_and(|c| !c.is_empty())
        || option.autocomplete
        || matches!(option.kind, BOOLEAN | USER | ROLE | MENTIONABLE | CHANNEL)
}

impl App {
    /// The tags, the look's colours, and what the box's buffer tells the
    /// line.
    pub(super) fn build_pills(self: &Rc<Self>) {
        let buffer = self.composer.buffer();
        let table = buffer.tag_table();
        let tag = |builder: gtk::builders::TextTagBuilder| {
            let tag = builder.build();
            table.add(&tag);
            tag
        };
        tag(gtk::TextTag::builder().name(NAME).weight(700));
        let key = tag(gtk::TextTag::builder().name(KEY).editable(false).weight(700));
        tag(gtk::TextTag::builder().name(END));
        let value = tag(gtk::TextTag::builder().name(VALUE));
        let active = tag(gtk::TextTag::builder().name(ACTIVE));
        let error = tag(gtk::TextTag::builder().name(ERROR).underline(pango::Underline::Error));
        let colours = {
            let error_label = self.composer_error.downgrade();
            move |style: &adw::StyleManager| {
                let soft = |alpha| {
                    let mut colour = style.accent_color_rgba();
                    colour.set_alpha(alpha);
                    colour
                };
                value.set_background_rgba(Some(&soft(0.15)));
                active.set_background_rgba(Some(&soft(0.3)));
                key.set_foreground_rgba(Some(&style.accent_color().to_standalone_rgba(style.is_dark())));
                if let Some(label) = error_label.upgrade() {
                    error.set_foreground_rgba(Some(&label.color()));
                }
            }
        };
        let style = adw::StyleManager::default();
        colours(&style);
        style.connect_accent_color_rgba_notify(colours.clone());
        style.connect_dark_notify(colours);

        buffer.connect_begin_user_action({
            let this = Rc::downgrade(self);
            move |buffer| {
                let Some(this) = this.upgrade() else { return };
                this.slash.in_action.set(true);
                if !this.slash.is_active() {
                    return;
                }
                // What deleting the selection takes whole, before GTK leaves the keys behind.
                let covered = match (buffer.selection_bounds(), this.read_line()) {
                    (Some((a, b)), Some((line, _))) => command_line::covered(&line, char_offset(&a)..char_offset(&b)),
                    _ => Vec::new(),
                };
                if let Some(active) = this.slash.active.borrow_mut().as_mut() {
                    active.covered = covered;
                }
            }
        });
        buffer.connect_end_user_action({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                this.slash.in_action.set(false);
                this.sync_command_line();
            }
        });
        buffer.connect_notify_local(Some("cursor-position"), {
            let this = Rc::downgrade(self);
            move |_, _| {
                if let Some(this) = this.upgrade()
                    && !this.slash.syncing.get()
                    && !this.slash.in_action.get()
                {
                    this.caret_moved();
                }
            }
        });
        self.mention_list.set_header_func({
            let this = Rc::downgrade(self);
            move |row, _| {
                let Some(this) = this.upgrade() else { return };
                let index = row.index().max(0) as usize;
                let header = this.mention.borrow().headers.iter().find(|(at, _)| *at == index).map(|(_, h)| h.clone());
                row.set_header(header.map(|h| header_label(&h)).as_ref());
            }
        });
    }

    fn cursor(&self) -> usize {
        let buffer = self.composer.buffer();
        buffer.property::<i32>("cursor-position").max(0) as usize
    }

    /// The box's text and where its keys and ends are.
    fn marked(&self) -> (String, Vec<Range<usize>>, Vec<usize>) {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let table = buffer.tag_table();
        let keys = table.lookup(KEY).map(|tag| ranges(&buffer, &tag)).unwrap_or_default();
        let ends = table.lookup(END).map(|tag| ranges(&buffer, &tag)).unwrap_or_default();
        (text, keys, ends.into_iter().flatten().collect())
    }

    /// The line the box holds, and its text, while a command is in it.
    pub(super) fn read_line(&self) -> Option<(Line, String)> {
        let slot = self.slash.active.borrow();
        let active = slot.as_ref()?;
        let (text, keys, ends) = self.marked();
        let line = command_line::read(&active.command, &Marked { text: &text, keys, ends });
        Some((line, text))
    }

    /// What each pill holds, as written.
    pub(super) fn line_values(&self) -> Option<Values> {
        let (line, text) = self.read_line()?;
        let active = self.slash.active.borrow();
        Some(command_line::values(&active.as_ref()?.command, &line, &text))
    }

    /// Make `change`, the box's handlers left out of it.
    fn apply_change(&self, change: &Change) {
        let buffer = self.composer.buffer();
        let was = self.slash.syncing.replace(true);
        for edit in &change.edits {
            match edit {
                Edit::Delete(range) => {
                    let mut from = buffer.iter_at_offset(range.start as i32);
                    let mut to = buffer.iter_at_offset(range.end as i32);
                    buffer.delete(&mut from, &mut to);
                }
                Edit::Insert { at, text, part } => {
                    let mut end = buffer.iter_at_offset(*at as i32);
                    buffer.insert(&mut end, text);
                    let start = buffer.iter_at_offset(*at as i32);
                    // Text inserted inside a tagged stretch takes its tags; it is only what it says it is.
                    for name in [NAME, KEY, END] {
                        buffer.remove_tag_by_name(name, &start, &end);
                    }
                    if let Some(name) = tag_of(*part) {
                        buffer.apply_tag_by_name(name, &start, &end);
                    }
                }
            }
        }
        if let Some(caret) = &change.caret {
            self.place_caret(caret);
        }
        self.slash.syncing.set(was);
    }

    fn place_caret(&self, caret: &Caret) {
        let buffer = self.composer.buffer();
        let was = self.slash.syncing.replace(true);
        match caret {
            Caret::At(at) => buffer.place_cursor(&buffer.iter_at_offset(*at as i32)),
            Caret::Select(range) => buffer
                .select_range(&buffer.iter_at_offset(range.end as i32), &buffer.iter_at_offset(range.start as i32)),
        }
        self.slash.syncing.set(was);
    }

    /// Put the command `id` in the box (the client's
    /// `APPLICATION_COMMAND_SET_ACTIVE_COMMAND`), started from `from`: a
    /// picked suggestion or a typed name ([`RunFrom::CHAT`]), or the
    /// launcher's fill-in, whose origin and source its run says. The box's
    /// text becomes its line ([`command_line::activate`]), which can't be
    /// undone, and every option is checked with empty ones passing. Setting
    /// the active command closes the app launcher (`AppLauncherStore`,
    /// 989837).
    pub(super) fn activate_command(self: &Rc<Self>, id: &str, from: RunFrom) {
        self.close_launcher();
        let Some(channel) = self.account.state.borrow().channel else { return };
        let found = {
            let state = self.account.state.borrow();
            let place = state.command_place(channel);
            place.and_then(|p| {
                state.commands.get(&p, commands::command_type::CHAT, id).map(|(c, a)| (c.clone(), a.clone()))
            })
        };
        let Some((command, app)) = found else { return };
        self.end_command(Keep::Text);
        let buffer = self.composer.buffer();
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let change = command_line::activate(&command, &typed, &Values::new());
        self.mention_popover.popdown();
        buffer.begin_irreversible_action();
        self.apply_change(&change);
        buffer.end_irreversible_action();
        let states = OptionStates::new(&command);
        self.slash.active.replace(Some(Active {
            channel,
            command,
            app,
            from,
            autocomplete: Default::default(),
            debounce: Rc::default(),
            states,
            covered: Vec::new(),
            asked: None,
            caret: self.cursor(),
            selection: Selection::default(),
        }));
        if let Some((line, text)) = self.read_line() {
            self.check_options(&line, &text, true, true);
        }
        self.show_command_icon();
        self.slash.bar.set_visible(true);
        self.refresh_attach();
        self.composer.grab_focus();
        self.refresh_line();
    }

    /// A command typed whole: it becomes active once the change that named
    /// it is over, if the box still names it then.
    pub(super) fn activate_typed(self: &Rc<Self>, id: String) {
        let this = Rc::downgrade(self);
        glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
            if let Some(this) = this.upgrade()
                && this.typed_command_id().as_ref() == Some(&id)
            {
                this.activate_command(&id, RunFrom::CHAT);
            }
            glib::ControlFlow::Break
        });
    }

    /// Take the command out of the box, leaving its line as plain text or
    /// nothing. Its bar, icon and lists go, and the attach button is back.
    /// Clearing the active command closes the app launcher too.
    pub(super) fn end_command(self: &Rc<Self>, keep: Keep) {
        if self.slash.active.take().is_none() {
            return;
        }
        self.close_launcher();
        let buffer = self.composer.buffer();
        let was = self.slash.syncing.replace(true);
        match keep {
            Keep::Text => {
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                let kept = command_line::plain(&text).chars().count() as i32;
                buffer.remove_all_tags(&buffer.start_iter(), &buffer.end_iter());
                buffer.delete(&mut buffer.iter_at_offset(kept), &mut buffer.end_iter());
            }
            Keep::Nothing => self.set_composer_text(""),
        }
        self.slash.syncing.set(was);
        self.slash.bar.set_visible(false);
        self.slash.more.set_visible(false);
        self.mention_popover.popdown();
        self.refresh_attach();
        // The text may name a command here, or be plain text with its own lists.
        if keep == Keep::Text {
            self.update_mention_popup();
        }
    }

    /// A change outside a user action: read the line once the main loop is
    /// idle, before it draws.
    pub(super) fn queue_command_sync(self: &Rc<Self>) {
        if self.slash.queued.replace(true) {
            return;
        }
        let this = Rc::downgrade(self);
        glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
            if let Some(this) = this.upgrade() {
                this.slash.queued.set(false);
                this.sync_command_line();
            }
            glib::ControlFlow::Break
        });
    }

    /// Read the line after a change (870748's `onChange`): the keys of the
    /// pills a deletion took whole go; an empty box ends the command, a
    /// broken name ends it keeping the text; what a deletion, an undo or a
    /// paste left wrong is repaired; typed keys become pills, else the
    /// one-option rule applies. Then everything drawn follows.
    pub(super) fn sync_command_line(self: &Rc<Self>) {
        if !self.slash.is_active() || self.slash.syncing.get() {
            return;
        }
        let covered =
            self.slash.active.borrow_mut().as_mut().map(|a| std::mem::take(&mut a.covered)).unwrap_or_default();
        if !covered.is_empty()
            && let Some((line, _)) = self.read_line()
        {
            let keys: Vec<Edit> = line
                .pills
                .iter()
                .rev()
                .filter(|p| covered.contains(&p.option))
                .map(|p| Edit::Delete(p.key.clone()))
                .collect();
            self.apply_change(&Change { edits: keys, caret: None });
        }
        // Each step settles what the last left; a few always do.
        for _ in 0..4 {
            let command = match self.slash.active.borrow().as_ref() {
                Some(active) => active.command.clone(),
                None => return,
            };
            let (text, keys, ends) = self.marked();
            if text.is_empty() {
                return self.end_command(Keep::Nothing);
            }
            let marked = Marked { text: &text, keys, ends };
            let line = command_line::read(&command, &marked);
            if !command_line::still_command(&command, &line, &text) {
                return self.end_command(Keep::Text);
            }
            let caret = self.cursor();
            let repaired = command_line::repair(&command, &marked, caret);
            let change = if !repaired.is_empty() {
                Some(repaired)
            } else {
                command_line::typed_keys(&command, &marked, caret)
                    .or_else(|| command_line::single_option(&command, &line, &text, caret))
            };
            match change {
                Some(change) => self.apply_change(&change),
                None => break,
            }
        }
        self.refresh_line();
    }

    /// Enter found options that fail (`eX` in 84046): required ones without
    /// a pill get an empty one at the end (`insertCommandOption` in 35277),
    /// `first` is selected, and the box shakes (`SHAKE_APP`).
    pub(super) fn command_failed(self: &Rc<Self>, first: &str) {
        let Some(command) = self.slash.active.borrow().as_ref().map(|a| a.command.clone()) else { return };
        let (text, keys, ends) = self.marked();
        let change = command_line::add_missing(&command, &Marked { text: &text, keys, ends });
        if !change.is_empty() {
            self.apply_change(&change);
        }
        if let Some((line, _)) = self.read_line()
            && let Some(pill) = line.pills.iter().find(|p| p.option == first)
        {
            self.place_caret(&Caret::Select(pill.value.clone()));
        }
        self.composer.error_bell();
        self.refresh_line();
    }

    /// The caret moved without the text changing. Inside a key it goes to
    /// the pill's value, whole (as clicking the key selects it), or, moving
    /// left from the value, before the key.
    fn caret_moved(self: &Rc<Self>) {
        let Some((line, _)) = self.read_line() else { return };
        let caret = self.cursor();
        let last = self.slash.active.borrow().as_ref().map_or(0, |a| a.caret);
        if let Some(pill) = line.pills.iter().find(|p| p.key.start < caret && caret < p.key.end) {
            let to = if last >= pill.key.end { Caret::At(pill.key.start) } else { Caret::Select(pill.value.clone()) };
            self.place_caret(&to);
        }
        self.refresh_line();
    }

    /// Check the options as the caret or the text moved on (`R` in 870748,
    /// see [`OptionStates::update`]).
    fn check_options(&self, line: &Line, text: &str, all: bool, allow_empty: bool) {
        let pill = command_line::active(line, self.cursor()).map(|p| p.option.clone());
        let mut slot = self.slash.active.borrow_mut();
        let Some(active) = slot.as_mut() else { return };
        let values = command_line::values(&active.command, line, text);
        let state = self.account.state.borrow();
        let names = Resolver { state: &state, channel: active.channel };
        let answers = active.autocomplete.answers();
        let check = |option: &CommandOption, value: Option<&str>, empty: bool| {
            commands::validate(option, value, empty, &names, answers)
        };
        active.states.update(&active.command, &values, pill.as_deref(), all, allow_empty, &check);
    }

    /// Everything drawn from the line: which options are checked, the
    /// pills' look, the bar, "+n options", the buttons, the app's choices
    /// and the popup.
    pub(super) fn refresh_line(self: &Rc<Self>) {
        let Some((line, text)) = self.read_line() else { return };
        let caret = self.cursor();
        self.check_options(&line, &text, false, false);
        let pill = command_line::active(&line, caret).cloned();
        let (command, states) = match self.slash.active.borrow_mut().as_mut() {
            Some(active) => {
                active.caret = caret;
                (active.command.clone(), active.states.clone())
            }
            None => return,
        };
        self.draw_pills(&line, &text, pill.as_ref().map(|p| p.option.as_str()), &states);
        let option = pill.as_ref().and_then(|p| command.options.iter().find(|o| o.name == p.option));
        let slash = &self.slash;
        match option {
            Some(option) => {
                slash.title.set_label(&option.display_name);
                match states.error(&option.name) {
                    Some(error) => {
                        slash.description.set_label(error);
                        slash.description.add_css_class("error");
                    }
                    None => {
                        slash.description.set_label(&option.description);
                        slash.description.remove_css_class("error");
                    }
                }
            }
            None => {
                slash.title.set_label(&format!("/{}", command.display_name));
                slash.description.set_label(&command.display_description);
                slash.description.remove_css_class("error");
            }
        }
        let more = command_line::placeholder(&command, &line);
        slash.more.set_visible(more.is_some());
        slash.more.set_label(more.as_deref().unwrap_or_default());
        self.refresh_chat_bar();
        // An option the app answers asks for choices as the caret enters it and as it is typed in.
        if let (Some(pill), Some(option)) = (&pill, option)
            && option.autocomplete
        {
            let query: String = text.chars().skip(pill.value.start).take(pill.value.len()).collect();
            let asked = (pill.option.clone(), query);
            let debounce = {
                let mut slot = self.slash.active.borrow_mut();
                let active = slot.as_mut().expect("active");
                (active.asked.as_ref() != Some(&asked)).then(|| {
                    active.asked = Some(asked);
                    active.debounce.clone()
                })
            };
            if let Some(debounce) = debounce {
                let (this, name) = (Rc::downgrade(self), pill.option.clone());
                debounce.poke(
                    &(Rc::new(move || {
                        if let Some(this) = this.upgrade() {
                            this.ask_autocomplete(&name);
                        }
                    }) as Rc<dyn Fn()>),
                );
            }
        }
        self.update_command_popup();
    }

    /// The name bold; each pill shaded, the one holding the caret more,
    /// one whose last check failed in the error colour.
    fn draw_pills(&self, line: &Line, text: &str, holding: Option<&str>, states: &OptionStates) {
        let buffer = self.composer.buffer();
        let length = text.chars().count();
        let iter = |at: usize| buffer.iter_at_offset(at.min(length) as i32);
        let (start, end) = buffer.bounds();
        for name in [NAME, VALUE, ACTIVE, ERROR] {
            buffer.remove_tag_by_name(name, &start, &end);
        }
        buffer.apply_tag_by_name(NAME, &iter(0), &iter(line.name.end));
        for pill in &line.pills {
            let (from, to) = (iter(pill.key.start), iter(pill.end + 1));
            buffer.apply_tag_by_name(VALUE, &from, &to);
            if holding == Some(pill.option.as_str()) {
                buffer.apply_tag_by_name(ACTIVE, &from, &to);
            } else if states.errored(&pill.option) {
                buffer.apply_tag_by_name(ERROR, &from, &to);
            }
        }
    }

    /// The type of the option whose pill holds the caret.
    pub(super) fn command_pill(&self) -> Option<u8> {
        let (line, _) = self.read_line()?;
        let pill = command_line::active(&line, self.cursor())?;
        let slot = self.slash.active.borrow();
        slot.as_ref()?.command.options.iter().find(|o| o.name == pill.option).map(|o| o.kind)
    }

    /// The app's picture in the attach button's place (`eY` in 699344,
    /// `getApplicationIconURL` with `botIconFirst`): the bot's avatar in
    /// this server, else its own, else the app's icon, never animated; a
    /// symbolic icon without one. Its tooltip names the bot, else the app.
    fn show_command_icon(&self) {
        let found = {
            let slot = self.slash.active.borrow();
            let Some(active) = slot.as_ref() else { return };
            let state = self.account.state.borrow();
            let guild = state.find_channel(active.channel).and_then(|(guild, _)| guild.map(|g| g.id));
            let app = &active.app;
            let member = guild.zip(app.bot).and_then(|(g, bot)| Some((g.0, state.members.avatar(g, bot)?.to_owned())));
            let size = discord::cdn::asset_size(24 * self.images.scale().max(1) as u32);
            let bot = app.bot.map(|bot| discord::cdn::IconBot {
                id: bot.0,
                avatar: app.bot_avatar.as_deref(),
                discriminator: "0",
                bot: true,
            });
            let url = discord::cdn::application_icon(
                app.id.0,
                app.icon.as_deref(),
                bot,
                member.as_ref().map(|(g, avatar)| (*g, avatar.as_str())),
                true,
                size,
            );
            (app.name.clone(), url)
        };
        let (name, url) = found;
        let icon = &self.slash.icon;
        icon.set_tooltip_text(Some(&name));
        icon.set_custom_image(None::<&gtk::gdk::Paintable>);
        icon.set_icon_name(Some("application-x-addon-symbolic"));
        if let Some(url) = url {
            let icon = icon.downgrade();
            self.images.load(&url, move |paintable| {
                if let Some(icon) = icon.upgrade() {
                    icon.set_custom_image(Some(&paintable));
                }
            });
        }
    }

    // ---- Keys ----

    /// What a key does with a command in the box, ahead of the box's other
    /// keys (84046's `handleKeyDown`, 31498's `onTabOrEnter`); `None` leaves
    /// it to them.
    pub(super) fn command_key(
        self: &Rc<Self>,
        key: gtk::gdk::Key,
        modifiers: gtk::gdk::ModifierType,
    ) -> Option<glib::Propagation> {
        use gtk::gdk::{Key, ModifierType};
        let stop = Some(glib::Propagation::Stop);
        let mods = modifiers & gtk::accelerator_get_default_mod_mask();
        let none = mods.is_empty();
        let enter = matches!(key, Key::Return | Key::KP_Enter | Key::ISO_Enter);
        let tab = key == Key::Tab && none;
        let back_tab = key == Key::ISO_Left_Tab || (key == Key::Tab && mods == ModifierType::SHIFT_MASK);
        if self.mention_popover.is_visible() {
            let count = self.mention.borrow().selectable();
            let ctrl = mods == ModifierType::CONTROL_MASK;
            let moved = match key {
                Key::Escape => {
                    self.mention_popover.popdown();
                    return stop;
                }
                Key::Down if none => Some(1),
                Key::Up if none => Some(-1),
                Key::n if ctrl => Some(1),
                Key::p if ctrl => Some(-1),
                _ => None,
            };
            if let Some(by) = moved {
                if let Some(active) = self.slash.active.borrow_mut().as_mut() {
                    active.selection.moved(by, count);
                }
                self.show_selection();
                return stop;
            }
            if tab || (enter && none) {
                let pick = self.slash.active.borrow_mut().as_mut().and_then(|a| a.selection.key(enter, count));
                match pick {
                    Some(Pick::Send(row) | Pick::Insert(row)) => {
                        self.accept_mention(row as i32);
                        return stop;
                    }
                    Some(Pick::Selected) => {
                        self.show_selection();
                        return stop;
                    }
                    None => {}
                }
            }
        }
        if tab || back_tab {
            let (line, text) = self.read_line()?;
            let caret = self.cursor();
            let to = if tab {
                Some(command_line::next(&line, &text, caret))
            } else {
                command_line::previous(&line, &text, caret)
            };
            if let Some(to) = to {
                self.place_caret(&to);
                self.refresh_line();
            }
            return stop;
        }
        if enter && (none || mods == ModifierType::CONTROL_MASK) {
            self.submit();
            return stop;
        }
        // A line break only inside a text option (68059's `insertBreak`).
        if enter {
            if self.command_pill() == Some(option_type::STRING) && self.text_pill() {
                let buffer = self.composer.buffer();
                buffer.begin_user_action();
                buffer.delete_selection(true, true);
                buffer.insert_interactive_at_cursor("\n", true);
                buffer.end_user_action();
            }
            return stop;
        }
        let back = key == Key::BackSpace;
        if (back || matches!(key, Key::Delete | Key::KP_Delete)) && !self.composer.buffer().has_selection() {
            let (line, text) = self.read_line()?;
            let caret = self.cursor();
            let change = if back {
                command_line::backspace(&line, &text, caret)
            } else {
                command_line::delete(&line, &text, caret)
            };
            if let Some(change) = change {
                self.apply_change(&change);
                self.sync_command_line();
                return stop;
            }
        }
        None
    }

    /// Whether the pill holding the caret is typed as text (a STRING
    /// without choices the list sets whole).
    fn text_pill(&self) -> bool {
        let Some((line, _)) = self.read_line() else { return false };
        let Some(pill) = command_line::active(&line, self.cursor()).map(|p| p.option.clone()) else { return false };
        let slot = self.slash.active.borrow();
        slot.as_ref().and_then(|a| a.command.options.iter().find(|o| o.name == pill)).is_some_and(|o| !picked_whole(o))
    }

    // ---- The popup ----

    /// What the popup lists for where the caret is in the line (496533's
    /// lists, the first that has something): nothing in the name; the
    /// options without a pill between pills; inside a pill, its choices,
    /// members and roles, channels, or the box's own `@` and `:` lists for
    /// text.
    pub(super) fn update_command_popup(self: &Rc<Self>) {
        let Some((line, text)) = self.read_line() else { return };
        let caret = self.cursor();
        let chars: Vec<char> = text.chars().collect();
        let buffer = self.composer.buffer();
        let holding = command_line::active(&line, caret);
        let listing = if (buffer.has_selection() && holding.is_none()) || caret < line.name.end + 1 {
            None
        } else {
            match holding {
                None => self.options_listing(&line, &chars, caret),
                Some(pill) => self.pill_listing(pill.clone(), &chars, caret),
            }
        };
        match listing {
            Some(listing) => self.show_listing(listing),
            None => self.mention_popover.popdown(),
        }
    }

    /// The options list (`K` in 496533): the options without a pill that
    /// the word typed starts, required ones first, nothing selected until
    /// something is typed.
    fn options_listing(&self, line: &Line, chars: &[char], caret: usize) -> Option<Listing> {
        let slot = self.slash.active.borrow();
        let command = &slot.as_ref()?.command;
        let segment = line.outside.iter().find(|s| s.start <= caret && caret <= s.end)?;
        let (start, word) = word_at(chars, segment.start, caret);
        let (required, others) = command_line::remaining(command, line, &word);
        let (required_header, others_header) = command_line::options_headers(!required.is_empty(), &word);
        let mut headers = Vec::new();
        if !required.is_empty() {
            headers.push((0, required_header));
        }
        if !others.is_empty() {
            headers.push((required.len(), others_header));
        }
        let candidates: Vec<Completion> = required
            .iter()
            .chain(&others)
            .map(|o| Completion::Option {
                name: o.name.clone(),
                display_name: o.display_name.clone(),
                description: o.description.clone(),
            })
            .collect();
        (!candidates.is_empty()).then_some(Listing {
            start: start as i32,
            candidates,
            headers,
            kind: "OPTIONS",
            focus: Focus::WhenFiltered,
            query: word,
            loading: false,
            pill: None,
        })
    }

    /// The list inside `pill`, by its option.
    fn pill_listing(self: &Rc<Self>, pill: command_line::Pill, chars: &[char], caret: usize) -> Option<Listing> {
        use option_type::*;
        let (option, channel, errored, answer) = {
            let slot = self.slash.active.borrow();
            let active = slot.as_ref()?;
            let option = active.command.options.iter().find(|o| o.name == pill.option)?.clone();
            let query: String = chars[pill.value.clone()].iter().collect();
            let answer = option
                .autocomplete
                .then(|| active.autocomplete.answers().choices(&option.name, &query).map(<[_]>::to_vec));
            (option, active.channel, active.autocomplete.errored, answer)
        };
        let value: String = chars[pill.value.clone()].iter().collect();
        let listing = |candidates: Vec<Completion>, headers, kind, loading| Listing {
            start: pill.value.start as i32,
            candidates,
            headers,
            kind,
            focus: Focus::Auto,
            query: value.clone(),
            loading,
            pill: Some(pill.option.clone()),
        };
        let matching = |plain: &'static str, matching: &'static str, query: &str, mark: &str| match query {
            "" => Header { words: plain, query: None },
            q => Header { words: matching, query: Some(format!("{mark}{q}")) },
        };
        // Choices come first (`F`): the option's own, true or false, or the app's.
        let static_choices = match &option.choices {
            Some(choices) if !choices.is_empty() => Some(choices.clone()),
            _ if option.kind == BOOLEAN => Some(commands::boolean_choices().to_vec()),
            _ => None,
        };
        if static_choices.is_some() || option.autocomplete {
            let header = vec![(0, matching("Options", "Options matching", &value, ""))];
            let (names, loading): (Vec<String>, bool) = match (&static_choices, answer.flatten()) {
                (Some(choices), _) => {
                    (commands::match_choices(choices, &value).iter().map(|c| c.name.clone()).collect(), false)
                }
                (None, Some(answered)) => (answered.iter().map(|c| c.name.clone()).collect(), false),
                (None, None) if errored => {
                    let failed = vec![Completion::Status("Loading options failed".into())];
                    return Some(listing(failed, header, "CHOICES", false));
                }
                (None, None) => (Vec::new(), true),
            };
            let candidates = if loading {
                vec![Completion::Status("Loading options…".into())]
            } else if names.is_empty() {
                vec![Completion::Status("No options match your search".into())]
            } else {
                names.into_iter().map(Completion::Choice).collect()
            };
            return Some(listing(candidates, header, "CHOICES", loading));
        }
        let (_, word) = word_at(chars, pill.value.start, caret);
        match option.kind {
            USER | ROLE | MENTIONABLE => {
                let query = word.strip_prefix('@').unwrap_or(&word).to_owned();
                let members = matches!(option.kind, USER | MENTIONABLE);
                let roles = matches!(option.kind, ROLE | MENTIONABLE);
                let (found, everyone, found_roles) = self.mentions(channel, &query, members, roles);
                let header = command_line::mention_header(members, found.len(), everyone, found_roles.len(), &query)?;
                let mut candidates: Vec<Completion> =
                    found.into_iter().map(|(id, name)| Completion::Mention(id.0, name)).collect();
                if everyone {
                    candidates.push(Completion::Everyone);
                }
                candidates.extend(found_roles.into_iter().map(|(id, name)| Completion::Role(id, name)));
                candidates.truncate(LISTED);
                Some(listing(candidates, vec![(0, header)], "MENTIONS", false))
            }
            CHANNEL => {
                let query = word.strip_prefix('#').unwrap_or(&word).to_owned();
                let channels = self.pill_channels(channel, &option, &query);
                if channels.is_empty() {
                    return None;
                }
                let header = vec![(0, matching("Channels", "Channels matching", &query, "#"))];
                let candidates = channels.into_iter().map(|(id, name)| Completion::Channel(id, name)).collect();
                Some(listing(candidates, header, "CHANNELS", false))
            }
            // Text takes the box's own lists, its value starting a word; numbers only `:emoji:`.
            _ => {
                let typed: String = chars[pill.value.start..caret.min(pill.value.end)].iter().collect();
                let (word_start, token) = word_at(&typed.chars().collect::<Vec<_>>(), 0, typed.chars().count());
                let at = pill.value.start + word_start;
                let candidates = match token.strip_prefix('@') {
                    Some(prefix) if option.kind == STRING && !prefix.contains('@') => self.mention_candidates(prefix),
                    _ => match token.strip_prefix(':') {
                        Some(query) if query.chars().count() >= 2 && !query.contains(':') => {
                            self.emoji_candidates(query)
                        }
                        _ => Vec::new(),
                    },
                };
                (!candidates.is_empty()).then(|| Listing {
                    start: at as i32,
                    candidates,
                    headers: Vec::new(),
                    kind: "TEXT",
                    focus: Focus::Auto,
                    query: token,
                    loading: false,
                    pill: None,
                })
            }
        }
    }

    /// The members a USER or MENTIONABLE pill offers for `query`, `@everyone`
    /// and the roles (`td` in 496533, `XM` in 168186). In a server the
    /// members it knows, and a search for the rest as the query changes
    /// (`queryGuildUsers`, 997625); with nothing typed, who last talked
    /// here. In a DM, its people. Every role but `@everyone`'s own, which
    /// `@everyone` stands for.
    fn mentions(self: &Rc<Self>, channel: Snowflake, query: &str, members: bool, roles: bool) -> (Named, bool, Named) {
        let lower = query.to_lowercase();
        let rank = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
            if names.iter().any(|n| n.starts_with(&lower)) {
                Some(0)
            } else if names.iter().any(|n| n.contains(&lower)) {
                Some(1)
            } else {
                None
            }
        };
        let (guild, found) = {
            let state = self.account.state.borrow();
            let Some((guild, open)) = state.find_channel(channel) else { return (Vec::new(), false, Vec::new()) };
            let guild = guild.cloned();
            let mut people: Vec<(Snowflake, String, Option<String>)> = Vec::new();
            match &guild {
                Some(g) => {
                    for (user, known) in state.members.of_guild(g.id) {
                        if let Some(name) = state.users.get(&user.0) {
                            people.push((user, name.clone(), known.nick.clone()));
                        }
                    }
                    if let Some(me) = state.me.as_ref().filter(|me| !people.iter().any(|p| p.0 == me.id)) {
                        people.push((
                            me.id,
                            me.display_name().to_owned(),
                            state.members.nick(g.id, me.id).map(str::to_owned),
                        ));
                    }
                }
                None => {
                    for user in open.recipients.iter().chain(state.me.as_ref()) {
                        people.push((user.id, user.display_name().to_owned(), None));
                    }
                }
            }
            (guild, people)
        };
        let mut listed: Vec<(Snowflake, String)> = Vec::new();
        if members {
            if query.is_empty() && guild.is_some() {
                // Who last talked here, newest first.
                let messages = self.messages();
                for position in (0..messages.n_items()).rev() {
                    let Some(message) = crate::timelines::message_at(&messages, position) else { continue };
                    let author = message.author.id;
                    if listed.len() < LISTED
                        && !listed.iter().any(|(id, _)| *id == author)
                        && let Some((_, name, nick)) = found.iter().find(|p| p.0 == author)
                    {
                        listed.push((author, nick.clone().unwrap_or_else(|| name.clone())));
                    }
                }
            }
            if listed.is_empty() {
                let mut ranked: Vec<(u8, String, Snowflake)> = found
                    .iter()
                    .filter_map(|(id, name, nick)| {
                        let shown = nick.clone().unwrap_or_else(|| name.clone());
                        let names: Vec<&str> = [Some(name.as_str()), nick.as_deref()].into_iter().flatten().collect();
                        Some((rank(&names)?, shown, *id))
                    })
                    .collect();
                ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
                listed = ranked.into_iter().take(LISTED).map(|(_, name, id)| (id, name)).collect();
            }
            if let Some(g) = &guild
                && !query.is_empty()
            {
                self.search_members(g.id, query);
            }
        }
        let Some(guild) = guild.filter(|_| roles) else { return (listed, false, Vec::new()) };
        let everyone = "everyone".starts_with(&lower);
        let mut found_roles: Vec<&discord::Role> =
            guild.roles.iter().filter(|r| r.id != guild.id && rank(&[r.name.as_str()]).is_some()).collect();
        found_roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        let found_roles = found_roles.into_iter().take(LISTED).map(|r| (r.id, r.name.clone())).collect();
        (listed, everyone, found_roles)
    }

    /// The channels a CHANNEL pill offers (`queryApplicationCommandChannelResults`
    /// in 997625): the server's of the option's types, all without any; in a
    /// DM, the DM itself if its type is one.
    fn pill_channels(&self, channel: Snowflake, option: &CommandOption, query: &str) -> Vec<(Snowflake, String)> {
        let state = self.account.state.borrow();
        let Some((guild, open)) = state.find_channel(channel) else { return Vec::new() };
        let lower = query.to_lowercase();
        let takes = |c: &discord::Channel| match &option.channel_types {
            Some(types) => types.contains(&u64::from(c.kind.raw())),
            None => c.kind != discord::ChannelKind::Category,
        };
        match guild {
            Some(guild) => guild
                .channels
                .iter()
                .chain(&guild.threads)
                .filter(|c| takes(c))
                .filter_map(|c| Some((c.id, c.name.clone()?)))
                .filter(|(_, name)| name.to_lowercase().contains(&lower))
                .take(LISTED)
                .collect(),
            None if takes(open) && open.title().to_lowercase().contains(&lower) => vec![(open.id, open.title())],
            None => Vec::new(),
        }
    }

    /// Search `guild`'s members for `query` on the gateway (695184), once
    /// the typing stops.
    fn search_members(self: &Rc<Self>, guild: Snowflake, query: &str) {
        let waits = self.account.state.borrow_mut().member_search.ask(guild, query, PILL_SEARCH_LIMIT, Instant::now());
        if !waits {
            return;
        }
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(SEARCH_WAIT, move || {
            let Some(this) = this.upgrade() else { return };
            let state = &mut *this.account.state.borrow_mut();
            let Some(search) = state.member_search.due(Instant::now()) else { return };
            // The server may have gone in the wait.
            if let (true, Some(gateway)) = (state.guilds.iter().any(|g| g.id == search.guild), &state.gateway) {
                gateway.search_members(search.guild, &search.query, search.limit);
            }
        });
    }

    /// A search's members arrived: a pill's list shows them.
    pub(super) fn members_found(self: &Rc<Self>) {
        if self.slash.is_active() && self.mention_popover.is_visible() {
            self.update_command_popup();
        }
    }

    fn show_listing(self: &Rc<Self>, listing: Listing) {
        let count = listing.candidates.iter().filter(|c| !matches!(c, Completion::Status(_))).count();
        let selected = {
            let mut slot = self.slash.active.borrow_mut();
            let Some(active) = slot.as_mut() else { return };
            active.selection.listed(listing.kind, listing.focus, &listing.query, listing.loading, count);
            active.selection.selected()
        };
        let ctx = MentionCtx {
            start: listing.start,
            candidates: listing.candidates,
            headers: listing.headers,
            pill: listing.pill,
        };
        self.show_listed(ctx, selected);
    }

    /// The selection moved: the popup's row follows.
    fn show_selection(&self) {
        let selected = self.slash.active.borrow().as_ref().and_then(|a| a.selection.selected());
        self.select_row(selected);
    }

    /// A row of the popup picked with a command in the box. True when it
    /// was the line's to take.
    pub(super) fn accept_in_line(self: &Rc<Self>, index: usize) -> bool {
        if !self.slash.is_active() {
            return false;
        }
        enum Picked {
            Option(String),
            Value(String, String),
            Nothing,
            Text,
        }
        let picked = {
            let ctx = self.mention.borrow();
            let value = |text: String| ctx.pill.clone().map_or(Picked::Text, |pill| Picked::Value(pill, text));
            match ctx.candidates.get(index) {
                Some(Completion::Option { name, .. }) => Picked::Option(name.clone()),
                Some(Completion::Choice(name)) => value(name.clone()),
                Some(Completion::Mention(id, _)) => value(format!("<@{id}>")),
                Some(Completion::Role(id, _)) => value(format!("<@&{id}>")),
                Some(Completion::Channel(id, _)) => value(format!("<#{id}>")),
                Some(Completion::Everyone) => value("@everyone".to_owned()),
                Some(Completion::Status(_)) | None => Picked::Nothing,
                Some(_) => Picked::Text,
            }
        };
        match picked {
            Picked::Text => return false,
            Picked::Nothing => {}
            // The option's key in place of the word, the caret in its value (`K.onSelect`).
            Picked::Option(option) => {
                let start = self.mention.borrow().start.max(0) as usize;
                let caret = self.cursor();
                let command = self.slash.active.borrow().as_ref().map(|a| a.command.clone());
                let Some(command) = command else { return true };
                let (text, keys, ends) = self.marked();
                let change = command_line::add(&command, &Marked { text: &text, keys, ends }, &option, start..caret);
                self.apply_change(&change);
                self.sync_command_line();
            }
            // The pill's whole value, then on to the next (`insertAutocomplete`).
            Picked::Value(option, value) => {
                let Some((line, _)) = self.read_line() else { return true };
                self.apply_change(&command_line::set_value(&line, &option, &value));
                if let Some((line, text)) = self.read_line() {
                    self.place_caret(&command_line::next(&line, &text, self.cursor()));
                }
                self.sync_command_line();
            }
        }
        self.composer.grab_focus();
        true
    }
}

/// A list's header: its words, and what was searched for in bold.
fn header_label(header: &Header) -> gtk::Label {
    let markup = match &header.query {
        Some(query) => format!("{} <b>{}</b>", header.words, glib::markup_escape_text(query)),
        None => header.words.to_owned(),
    };
    gtk::Label::builder()
        .use_markup(true)
        .label(markup)
        .xalign(0.0)
        .margin_start(6)
        .margin_top(6)
        .margin_bottom(2)
        .css_classes(["caption-heading", "dim-label"])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_typed_runs_back_to_a_space_or_where_it_may_start() {
        let chars: Vec<char> = "/roll count:3 si".chars().collect();
        assert_eq!(word_at(&chars, 14, 16), (14, "si".to_owned()));
        assert_eq!(word_at(&chars, 14, 14), (14, String::new()), "nothing typed yet");
        let value: Vec<char> = "who:@al".chars().collect();
        assert_eq!(word_at(&value, 4, 7), (4, "@al".to_owned()), "a pill's value starts a word");
    }
}
