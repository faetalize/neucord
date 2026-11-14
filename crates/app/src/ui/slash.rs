//! Slash commands: the suggestions after a `/`, the bar above the box while
//! a command is in it, and running it; and the Apps item of the message and
//! user menus, which runs a user or message command, and Use Apps in the
//! attach menu. The command's line in the box is `pills`. What each sends
//! and when comes from the client's command code; see `discord::commands`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use discord::builtins;
use discord::command_line::{Header, OptionStates};
use discord::commands::{self, App as CommandApp, Autocomplete, Command, Names, Run, RunFrom, Selection, command_type};
use discord::{Snowflake, interaction::Failure};
use gtk::{gio, glib};

use super::App;
use super::chat::MessageTarget;
use super::command_files::{FileBoxes, FileRun};
use super::composer::{Completion, MentionCtx};
use super::menus::{Entry, Item, Kind, Live};
use super::profile::UserTarget;
use super::state::now_unix;
use super::uploads::Upload;

/// The icons over the command browser's sections: the clock over Frequently
/// Used and a slash in a box over Built-In (`ClockIcon`, `SlashBoxIcon` in
/// 834968), and, since an app's picture would be a request the list doesn't
/// make, the one the box shows for an app without one.
const FREQUENTLY_USED_ICON: &str = "document-open-recent-symbolic";
pub(super) const BUILT_IN_ICON: &str = "utilities-terminal-symbolic";
pub(super) const APP_ICON: &str = "application-x-addon-symbolic";

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

/// What a command that ends leaves in the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keep {
    /// Its line, as plain text (`j(…, !0)` in 870748).
    Text,
    /// Nothing (`j(…, !1)`).
    Nothing,
}

/// The widgets for slash commands, around the message box.
pub(super) struct Slash {
    /// Above the message box while a command is in it (`tV` in 989161): the
    /// option the caret is in, or the command.
    pub(super) bar: gtk::Box,
    pub(super) title: gtk::Label,
    pub(super) description: gtk::Label,
    /// What running commands are doing, in the open channel.
    status: gtk::Label,
    /// The command's app, in the attach button's place.
    pub(super) icon: adw::Avatar,
    /// "+2 options" after the box (`e6` in 84046).
    pub(super) more: gtk::Label,
    pub(super) active: RefCell<Option<Active>>,
    /// The box is being changed by the line's own code, whose changes the
    /// box's handlers leave alone.
    pub(super) syncing: Cell<bool>,
    /// A /thread is on its way: Enter waits for it (the client's command
    /// stays, its run already in hand).
    pub(super) thread_busy: Cell<bool>,
    /// Inside a user action (typing, a deletion, a paste), at whose end the
    /// line is read again.
    pub(super) in_action: Cell<bool>,
    /// A change outside a user action waits for the line to be read again.
    pub(super) queued: Cell<bool>,
    /// The command's file boxes, in the place of the message's files.
    pub(super) file_boxes: FileBoxes,
}

/// The command in the box (`ApplicationCommandStore`, 861382).
pub(super) struct Active {
    pub(super) channel: Snowflake,
    pub(super) command: Command,
    pub(super) app: CommandApp,
    /// Where it was started from, which its run says (the origin and source
    /// `ApplicationCommandStore` keeps with the active command).
    pub(super) from: RunFrom,
    pub(super) autocomplete: Autocomplete,
    pub(super) debounce: Rc<Debounce>,
    pub(super) states: OptionStates,
    /// The pills a deletion of the selection takes whole (`covered`),
    /// worked out as the user action begins.
    pub(super) covered: Vec<String>,
    /// The option and text the app was last asked choices for, as the
    /// caret entered or the text changed.
    pub(super) asked: Option<(String, String)>,
    /// Where the caret was, to tell which way it went into a key.
    pub(super) caret: usize,
    /// Which row of the popup is selected.
    pub(super) selection: Selection,
    /// Each file option's file, by option name, pill or not.
    pub(super) files: BTreeMap<String, Rc<Upload>>,
}

impl Slash {
    pub(super) fn new() -> Self {
        Slash {
            bar: gtk::Box::builder().spacing(8).visible(false).build(),
            title: gtk::Label::builder().xalign(0.0).css_classes(["heading"]).build(),
            description: gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label"])
                .build(),
            status: gtk::Label::builder().xalign(0.0).visible(false).css_classes(["dim-label"]).build(),
            icon: adw::Avatar::builder().size(24).show_initials(false).visible(false).build(),
            more: gtk::Label::builder().visible(false).valign(gtk::Align::Center).css_classes(["dim-label"]).build(),
            active: RefCell::default(),
            syncing: Cell::default(),
            thread_busy: Cell::default(),
            in_action: Cell::default(),
            queued: Cell::default(),
            file_boxes: FileBoxes::new(),
        }
    }

    /// Whether a command is in the box.
    pub(super) fn is_active(&self) -> bool {
        self.active.borrow().is_some()
    }
}

/// The bar above a command, for its menu: Copy Command ID copies the root
/// command's id (`dev-context`, 646938).
pub(super) struct BarTarget {
    root: Option<Snowflake>,
}

/// Copy Command ID, only in Developer Mode (50268); a built-in has none.
pub(super) const COPY_COMMAND_ID: Entry<BarTarget> = Entry::Item(Item {
    name: "copy-command-id",
    label: "Copy Command _ID",
    shows: |app, target| target.root.is_some() && app.account.state.borrow().settings.developer_mode,
    kind: Kind::Action(|app, target| {
        if let Some(id) = target.root {
            app.copy(&id.to_string());
        }
    }),
});

/// A user or message a command of the Apps menu acts on, and where.
#[derive(Clone, Copy)]
pub(super) struct Subject {
    pub(super) channel: Snowflake,
    /// [`command_type::USER`] or [`command_type::MESSAGE`].
    pub(super) kind: u8,
    pub(super) target: Snowflake,
}

/// What a menu is opened on, for the Apps item.
pub(super) trait CommandTarget {
    /// What its commands act on, if the menu has an Apps item.
    fn subject(&self) -> Option<Subject>;
}

impl CommandTarget for MessageTarget {
    /// The message the row shows (`commandTargetId` in 761304).
    fn subject(&self) -> Option<Subject> {
        let message = self.shown();
        Some(Subject { channel: message.channel_id, kind: command_type::MESSAGE, target: message.id })
    }
}

impl CommandTarget for UserTarget {
    /// The person, in the channel the menu was opened in (107632). A system
    /// account or a webhook's author has no Apps item.
    fn subject(&self) -> Option<Subject> {
        let channel = self.channel.filter(|_| !self.user.non_user_bot())?;
        Some(Subject { channel, kind: command_type::USER, target: self.user.id })
    }
}

/// Apps, in the message and user menus (173500): the commands of the apps
/// in the channel that act on that message or person.
pub(super) const MESSAGE_APPS: Entry<MessageTarget> = apps_item();
pub(super) const USER_APPS: Entry<UserTarget> = apps_item();

const fn apps_item<T: CommandTarget + 'static>() -> Entry<T> {
    Entry::Item(Item { name: "apps", label: "_Apps", shows: shows_apps::<T>, kind: Kind::Live(apps_menu::<T>) })
}

/// Textual channels only (`offers_apps`), and where there is somewhere to
/// look for commands.
fn shows_apps<T: CommandTarget>(app: &App, target: &T) -> bool {
    let Some(subject) = target.subject() else { return false };
    let state = app.account.state.borrow();
    state.find_channel(subject.channel).is_some_and(|(_, c)| commands::offers_apps(c.kind))
        && state.command_place(subject.channel).is_some()
}

/// The commands under Apps, kept for the menu while it is open: a search
/// box, and the commands by app, or the ones the search finds. The indexes
/// are fetched as the item is made, as the client's hook does as the menu
/// renders (`cu` in 842209 with `allowFetch`), and the list is made again
/// when they arrive.
struct AppsMenu {
    app: std::rc::Weak<App>,
    subject: Subject,
    /// The commands, below the search box.
    menu: gio::Menu,
    actions: gio::SimpleActionGroup,
    /// The actions of the commands now listed.
    listed: RefCell<Vec<String>>,
    /// What was typed in the search box.
    query: RefCell<String>,
    made: Cell<u32>,
}

fn apps_menu<T: CommandTarget + 'static>(app: &Rc<App>, target: &Rc<T>, live: Live) {
    let Some(subject) = target.subject() else { return };
    let search = gtk::SearchEntry::builder().placeholder_text("Search commands").build();
    let custom = gio::MenuItem::new(None, None);
    custom.set_attribute_value("custom", Some(&"command-search".to_variant()));
    let top = gio::Menu::new();
    top.append_item(&custom);
    live.menu.append_section(None, &top);
    let commands = gio::Menu::new();
    live.menu.append_section(None, &commands);
    live.widgets.borrow_mut().push(("command-search", search.clone().upcast()));

    let menu = Rc::new(AppsMenu {
        app: Rc::downgrade(app),
        subject,
        menu: commands,
        actions: live.actions,
        listed: RefCell::default(),
        query: RefCell::default(),
        made: Cell::default(),
    });
    // The box owns the list: no more of it once the menu is gone.
    search.connect_search_changed({
        let menu = menu.clone();
        move |search| {
            *menu.query.borrow_mut() = search.text().to_string();
            menu.list();
        }
    });
    // `F` in 56494, as the menu's commands are asked for: what is used
    // orders them and makes the Frequently Used Commands.
    app.load_frecency({
        let menu = Rc::downgrade(&menu);
        move || {
            if let Some(menu) = menu.upgrade() {
                menu.list();
            }
        }
    });
    let place = app.account.state.borrow().command_place(subject.channel);
    if let Some(place) = place {
        app.fetch_command_indexes(&place, commands::Offer::ALL, "", {
            let menu = Rc::downgrade(&menu);
            Rc::new(move || {
                if let Some(menu) = menu.upgrade() {
                    menu.list();
                }
            })
        });
    }
    menu.list();
}

/// What the Apps menu has to show.
enum Listing {
    Loading,
    Empty,
    /// Commands found for what was searched for, best first: id and name.
    Found(Vec<(String, String)>),
    /// The commands used most here, then each app and its commands.
    Apps {
        frequent: Vec<(String, String)>,
        apps: Vec<(String, Vec<(String, String)>)>,
    },
}

impl AppsMenu {
    /// Fill the menu with what is known now.
    fn list(&self) {
        let Some(app) = self.app.upgrade() else { return };
        let used = app.command_use();
        let listing = {
            let state = app.account.state.borrow();
            let Some(place) = state.command_place(self.subject.channel) else { return };
            let query = self.query.borrow();
            let asked = commands::Query {
                kind: self.subject.kind,
                text: &query,
                offer: commands::Offer::ALL,
                builtins: &[],
                used: Some(&used),
            };
            let found = state.commands.query(&place, asked);
            let command = |c: &Command| (c.id.clone(), c.display_name.clone());
            if state.commands.loading(&place) {
                Listing::Loading
            } else if found.is_empty() {
                Listing::Empty
            } else if !query.is_empty() {
                Listing::Found(found.iter().map(|f| command(f.command)).collect())
            } else {
                // `XC` in 264322: of the commands in the order they are listed, the five used most.
                let by_app = commands::by_app(found);
                let listed: Vec<&Command> = by_app.iter().flat_map(|(_, commands)| commands.iter().copied()).collect();
                let frequent = used.frequently_used(&listed, place.guild).into_iter().map(command).collect();
                let apps =
                    by_app.iter().map(|(app, found)| (app.name.clone(), found.iter().copied().map(command).collect()));
                Listing::Apps { frequent, apps: apps.collect() }
            }
        };
        for name in self.listed.take() {
            self.actions.remove_action(&name);
        }
        self.menu.remove_all();
        match listing {
            Listing::Loading => self.placeholder("Loading…"),
            Listing::Empty => self.placeholder("No Commands Available"),
            Listing::Found(found) => {
                for (id, name) in found {
                    self.menu.append_item(&self.command(&id, &name));
                }
            }
            Listing::Apps { frequent, apps } => {
                if !frequent.is_empty() {
                    let group = gio::Menu::new();
                    for (id, name) in frequent {
                        group.append_item(&self.command(&id, &name));
                    }
                    self.menu.append_section(Some("Frequently Used Commands"), &group);
                }
                let group = gio::Menu::new();
                for (name, found) in apps {
                    let commands = gio::Menu::new();
                    for (id, name) in found {
                        commands.append_item(&self.command(&id, &name));
                    }
                    group.append_submenu(Some(&name.replace('_', "__")), &commands);
                }
                self.menu.append_section(Some("Apps"), &group);
            }
        }
    }

    /// An item that does nothing.
    fn placeholder(&self, label: &str) {
        let action = gio::SimpleAction::new("command-none", None);
        action.set_enabled(false);
        self.actions.add_action(&action);
        self.listed.borrow_mut().push("command-none".into());
        self.menu.append(Some(label), Some("menu.command-none"));
    }

    /// An item that runs the command `id`. Names from Discord have no
    /// mnemonics.
    fn command(&self, id: &str, name: &str) -> gio::MenuItem {
        let n = self.made.get();
        self.made.set(n + 1);
        let action_name = format!("command-{n}");
        let action = gio::SimpleAction::new(&action_name, None);
        let (app, subject, id) = (self.app.clone(), self.subject, id.to_owned());
        action.connect_activate(move |_, _| {
            if let Some(app) = app.upgrade() {
                app.run_context_command(subject, &id);
            }
        });
        self.actions.add_action(&action);
        self.listed.borrow_mut().push(action_name.clone());
        gio::MenuItem::new(Some(&name.replace('_', "__")), Some(&format!("menu.{action_name}")))
    }
}

/// Use Apps, in the attach menu (`SLASH_COMMAND` in 316576): starts a command
/// in the message box, as typing `/` does. Where the Apps button is turned
/// off, App Launcher takes its place (`launcher::APP_LAUNCHER`). The attach
/// button on the notice that replaces the message box lacks it, as a command
/// has nowhere to be typed there. Neucord sends no analytics for it
/// (`CHANNEL_ATTACH_MENU_USE_APPS_CLICKED`).
pub(super) const USE_APPS: Entry<()> = Entry::Item(Item {
    name: "use-apps",
    label: "Use _Apps",
    shows: |app, _| app.apps_channel().is_some() && !app.attach_launcher(),
    kind: Kind::Action(|app, _| app.use_apps()),
});

/// Who and what a name in an option can mean, from what the app knows.
pub(super) struct Resolver<'a> {
    pub(super) state: &'a super::state::State,
    pub(super) channel: Snowflake,
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
    /// The running commands' status and the command's bar, in the message
    /// area.
    pub(super) fn build_slash(self: &Rc<Self>, area: &gtk::Box) {
        let slash = &self.slash;
        area.append(&slash.status);
        slash.bar.append(&slash.title);
        slash.bar.append(&slash.description);
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close")
            .css_classes(["flat"])
            .build();
        close.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.end_command(Keep::Nothing);
                    this.composer.grab_focus();
                }
            }
        });
        slash.bar.append(&close);
        super::menus::on_secondary(&slash.bar, {
            let this = Rc::downgrade(self);
            move |x, y| {
                let Some(this) = this.upgrade() else { return };
                let root = this.slash.active.borrow().as_ref().and_then(|a| a.command.root_id());
                let at = super::menus::At::Point(x, y);
                super::menus::popup(&this, super::menus::COMMAND_BAR, BarTarget { root }, &this.slash.bar, at);
            }
        });
        area.append(&slash.bar);
        self.build_command_files(area);
    }

    /// Show the suggestions for the command being typed after a `/` at the
    /// start of the message box, fetching the apps and commands first if the
    /// client would. Returns whether the box holds a command.
    pub(super) fn update_slash_popup(self: &Rc<Self>) -> bool {
        self.slash_popup(true)
    }

    /// A reply started or ended: the `/` list follows it at once, fetching
    /// nothing. A bare `/`'s list is the client's command browser, whose
    /// filters follow the reply (`ed` in 989161) and which fetches only as it
    /// opens (`ee`, `et` in 264322); a typed list is asked again whenever
    /// `PendingReplyStore` changes, one of the stores of `B` in 496533 that
    /// 133746 listens to, without `allowFetch` (31498).
    pub(super) fn reply_changed_slash(self: &Rc<Self>) {
        self.slash_popup(false);
    }

    fn slash_popup(self: &Rc<Self>, fetch: bool) -> bool {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let Some(typed) = text.strip_prefix('/') else { return false };
        if self.editing().is_some() || self.slash.is_active() {
            return false;
        }
        let Some(channel) = self.account.state.borrow().channel else { return false };
        let Some(place) = self.account.state.borrow().command_place(channel) else { return false };
        let replying = self.reply.borrow().as_ref().is_some_and(|r| r.channel == channel);
        let offer = commands::offer(replying);
        if fetch {
            self.fetch_command_indexes(&place, offer, typed, {
                let this = Rc::downgrade(self);
                Rc::new(move || {
                    // The box may have moved on while the index came. The
                    // client's typed list asks again as the index store
                    // changes, without `allowFetch` (31498), and its browser
                    // fetches only as it opens: no fetch here either.
                    if let Some(this) = this.upgrade() {
                        this.slash_popup(false);
                    }
                })
            });
        }
        let loads_use = commands::loads_use(&self.account.state.borrow().commands, &place, typed, offer);
        if loads_use {
            let this = Rc::downgrade(self);
            self.load_frecency(move || {
                if let Some(this) = this.upgrade() {
                    this.slash_popup(false);
                }
            });
        }
        let used = self.command_use();
        if let Some(id) = self.typed_command_id() {
            self.activate_typed(id);
            return true;
        }
        let builtins = self.builtin_commands(channel, offer.builtins);
        let suggestion = |f: &commands::Found<'_>| {
            Completion::Command(Suggestion {
                id: f.command.id.clone(),
                name: format!("/{}", f.command.display_name),
                description: f.command.display_description.clone(),
                app: f.app.name.clone(),
            })
        };
        let listed = {
            let state = self.account.state.borrow();
            if typed.is_empty() {
                // The command browser: each section under its name and icon.
                let mut listed = MentionCtx::default();
                for section in commands::browse(&state.commands, &place, offer, &builtins, &used) {
                    let at = listed.candidates.len();
                    let icon = match section.heading {
                        commands::Heading::FrequentlyUsed => FREQUENTLY_USED_ICON,
                        commands::Heading::App(app) if app.is_built_in() => BUILT_IN_ICON,
                        commands::Heading::App(_) => APP_ICON,
                    };
                    let words = section.heading.title().to_owned().into();
                    listed.headers.push((at, Header { words, query: None }));
                    listed.icons.push((at, icon));
                    listed.candidates.extend(section.commands.iter().map(suggestion));
                }
                listed
            } else {
                let found = commands::suggestions(&state.commands, &place, typed, offer, &builtins, &used);
                MentionCtx { candidates: found.iter().map(suggestion).collect(), ..MentionCtx::default() }
            }
        };
        self.show_listed(listed, Some(0));
        true
    }

    /// The built-ins offered in `channel` now, as `offer` lists them (`F`
    /// in 12115 and each one's predicate), made again each time since they
    /// follow our permissions, the TTS setting and the message limit.
    pub(super) fn builtin_commands(&self, channel: Snowflake, offer: commands::BuiltIns) -> Vec<Command> {
        let state = self.account.state.borrow();
        let (Some((guild, at)), Some(me)) = (state.find_channel(channel), state.account.as_ref()) else {
            return Vec::new();
        };
        let facts = builtins::Facts {
            dice: discord::dice::enabled(&state.experiments, me.id),
            ..builtins::Facts::of(guild, at, me, state.settings.tts_command(), now_unix())
        };
        builtins::commands(&facts, offer, state.limits().max_message_length() as u64)
    }

    /// The command the box's text names whole, its name ended, where the
    /// `/` list would offer it now (870748, with no command active).
    pub(super) fn typed_command_id(&self) -> Option<String> {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let typed = text.strip_prefix('/')?;
        if self.editing().is_some() || self.slash.is_active() {
            return None;
        }
        let channel = self.account.state.borrow().channel?;
        let replying = self.reply.borrow().as_ref().is_some_and(|r| r.channel == channel);
        let used = self.command_use();
        let state = self.account.state.borrow();
        let place = state.command_place(channel)?;
        let (query, terminated) = commands::query_of(typed);
        let offer = commands::offer(replying);
        drop(state);
        let builtins = self.builtin_commands(channel, offer.builtins);
        let state = self.account.state.borrow();
        let asked =
            commands::Query { kind: command_type::CHAT, text: &query, offer, builtins: &builtins, used: Some(&used) };
        let found = state.commands.query(&place, asked);
        commands::typed_command(found, &query, terminated).map(|c| c.id.clone())
    }

    /// Ask for the indexes a list of commands needs (`to_fetch`): each once,
    /// until Discord says it changed. `loaded` runs as each arrives.
    pub(super) fn fetch_command_indexes(
        self: &Rc<Self>,
        place: &commands::Place,
        offer: commands::Offer,
        typed: &str,
        loaded: Rc<dyn Fn()>,
    ) {
        let (rest, me) = {
            let state = self.account.state.borrow();
            (state.rest.clone(), state.me())
        };
        let (Some(rest), Some(me)) = (rest, me) else { return };
        let targets = self.account.state.borrow_mut().commands.to_fetch(place, Instant::now(), offer, typed);
        let epoch = self.account.epoch.get();
        for target in targets {
            let (this, rest, loaded) = (self.clone(), rest.clone(), loaded.clone());
            glib::spawn_future_local(async move {
                let index = rest.command_index(target).await;
                // An index of an account that was left isn't the next one's.
                if this.account.epoch.get() != epoch {
                    return;
                }
                {
                    let mut state = this.account.state.borrow_mut();
                    match &index {
                        Some(index) => state.commands.loaded(target, index, me),
                        None => state.commands.failed(target, Instant::now()),
                    }
                }
                loaded();
            });
        }
    }

    /// The open channel, if the client's attach menu would offer Use Apps in
    /// it now (`canUseApplicationCommands`): the message box shows, nothing
    /// is being edited or replied to, and commands have a place to run.
    pub(super) fn apps_channel(&self) -> Option<Snowflake> {
        let channel = self.account.state.borrow().channel?;
        let replying = self.reply.borrow().as_ref().is_some_and(|r| r.channel == channel);
        let allowed = self.composer_box.is_visible()
            && self.editing().is_none()
            && !self.has_voice_draft(channel)
            && !replying
            && self.account.state.borrow().command_place(channel).is_some();
        allowed.then_some(channel)
    }

    /// Replace what the message box holds with a `/`, which lists the
    /// commands (`setValue` in 316576).
    fn use_apps(self: &Rc<Self>) {
        if self.apps_channel().is_none() {
            return;
        }
        self.end_command(Keep::Nothing);
        self.set_composer_text("/");
        self.composer.grab_focus();
    }

    /// Ask the app for the choices to `option` for what it holds, if they
    /// aren't known (`WL` in 721768).
    pub(super) fn ask_autocomplete(self: &Rc<Self>, option: &str) {
        enum Ask {
            Nothing,
            Known,
            Send(discord::Rest, serde_json::Value, Snowflake),
        }
        let Some(values) = self.line_values() else { return };
        let ask = {
            let mut slot = self.slash.active.borrow_mut();
            let Some(active) = slot.as_mut() else { return };
            // The option asking goes out as typed, even as spaces alone (`autocomplete.query` in 545152's `A`).
            let Some(query) = values.get(option).cloned() else { return };
            let state = self.account.state.borrow();
            let names = Resolver { state: &state, channel: active.channel };
            let options =
                commands::run_options(&active.command, &values, &names, active.autocomplete.answers(), Some(option));
            match (options, state.rest.clone()) {
                (Ok(options), Some(rest)) => {
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
                            from: active.from,
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
            Ask::Known => self.update_command_popup(),
            Ask::Send(rest, body, nonce) => {
                let this = self.clone();
                glib::spawn_future_local(async move {
                    // The request failing is the app failing, as the client's `INTERACTION_FAILURE` has it.
                    if rest.autocomplete(body).await.is_err() {
                        this.autocomplete_failed(nonce);
                    }
                    this.update_command_popup();
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
            let integer: Vec<(String, bool)> = active
                .command
                .options
                .iter()
                .map(|o| (o.name.clone(), o.kind == commands::option_type::INTEGER))
                .collect();
            let whole = move |name: &str| integer.iter().any(|(n, whole)| n == name && *whole);
            active.autocomplete.answer(nonce, choices, whole)
        };
        if option.is_some() {
            self.update_command_popup();
        }
    }

    /// Check the line and run the command (`eX` in 84046, then `A` in
    /// 545152 and `handleCommand` in 870801). Options that fail keep it in
    /// the box: required ones without a pill get an empty one at the end
    /// (`insertCommandOption`), the first that fails is selected and the box
    /// shakes. The box empties once the run is on its way.
    pub(super) fn run_command(self: &Rc<Self>) {
        enum Prepared {
            Nothing,
            Failing(String),
            BuiltIn(Snowflake, Box<Command>, Vec<serde_json::Value>),
            Error(String),
            NoticeBar,
            Go {
                channel: Snowflake,
                command: Box<Command>,
                app: Box<CommandApp>,
                body: serde_json::Value,
                nonce: Snowflake,
            },
            Files(Box<Command>, Box<CommandApp>, FileRun),
        }
        let Some((line, _)) = self.read_line() else { return };
        let Some(values) = self.line_values() else { return };
        let prepared = {
            let mut slot = self.slash.active.borrow_mut();
            let Some(active) = slot.as_mut() else { return };
            let state = self.account.state.borrow();
            let names = Resolver { state: &state, channel: active.channel };
            let answers = active.autocomplete.answers();
            let check = |option: &commands::CommandOption, value: Option<&str>, empty: bool| {
                commands::validate(option, value, empty, &names, answers)
            };
            let checked = active.states.submit(&active.command, &line, &values, &check);
            if let Err(failing) = checked {
                Prepared::Failing(failing.into_iter().next().unwrap_or_default())
            } else if active.command.is_built_in() {
                match commands::run_options(&active.command, &values, &names, answers, None) {
                    Err(why) => Prepared::Error(why),
                    Ok(options) => Prepared::BuiltIn(active.channel, Box::new(active.command.clone()), options),
                }
            } else if state.session_id.is_empty() {
                Prepared::Nothing
            } else if !state.can_send(active.channel) {
                Prepared::NoticeBar
            } else {
                match commands::run_options(&active.command, &values, &names, answers, None) {
                    Err(why) => Prepared::Error(why),
                    Ok(options) => {
                        let guild = state.find_channel(active.channel).and_then(|(guild, _)| guild.map(|g| g.id));
                        let nonce = Snowflake::now();
                        // Taken from the command, so that ending it doesn't stop them.
                        let files: Vec<Rc<Upload>> = commands::file_options(&options)
                            .iter()
                            .filter_map(|name| active.files.remove(*name))
                            .collect();
                        if !files.is_empty() {
                            let run = FileRun {
                                application: active.command.application,
                                guild,
                                channel: active.channel,
                                data: active.command.data(options, None),
                                nonce,
                                from: active.from,
                                files,
                            };
                            Prepared::Files(Box::new(active.command.clone()), Box::new(active.app.clone()), run)
                        } else {
                            let run = Run {
                                application: active.command.application,
                                guild,
                                channel: active.channel,
                                session: &state.session_id,
                                data: active.command.data(options, None),
                                nonce,
                                from: active.from,
                            };
                            Prepared::Go {
                                channel: active.channel,
                                command: Box::new(active.command.clone()),
                                app: Box::new(active.app.clone()),
                                body: run.body(&[]),
                                nonce,
                            }
                        }
                    }
                }
            }
        };
        let ran = match prepared {
            Prepared::Nothing => false,
            Prepared::Failing(first) => {
                self.command_failed(&first);
                false
            }
            Prepared::BuiltIn(channel, command, options) => self.run_built_in(channel, &command, &options),
            Prepared::Error(why) => {
                self.show_composer_error(&why);
                false
            }
            Prepared::NoticeBar => {
                self.update_notice_bar();
                false
            }
            Prepared::Go { channel, command, app, body, nonce } => {
                self.send_command(channel, *command, &app, body, nonce)
            }
            Prepared::Files(command, app, run) => self.send_command_files(*command, &app, run),
        };
        if ran {
            self.end_command(Keep::Nothing);
        }
    }

    /// Run a user or message command from the Apps menu of `subject`'s menu
    /// (`commandTargetId` in 545152's `A`): the command `id` on the user or
    /// message, with no options.
    fn run_context_command(self: &Rc<Self>, subject: Subject, id: &str) {
        self.run_at_once(subject.channel, subject.kind, id, Some(subject.target), RunFrom::CHAT);
    }

    /// Run the command `id` of `kind` in `channel` with no options, on
    /// `target` if it is a user or message command: the Apps menus, and the
    /// launcher's Send (`MJ` in 735991).
    pub(super) fn run_at_once(
        self: &Rc<Self>,
        channel: Snowflake,
        kind: u8,
        id: &str,
        target: Option<Snowflake>,
        from: RunFrom,
    ) {
        enum Prepared {
            Nothing,
            NoticeBar,
            Go { command: Box<Command>, app: Box<CommandApp>, body: serde_json::Value, nonce: Snowflake },
        }
        let prepared = {
            let state = self.account.state.borrow();
            let found = state.command_place(channel).and_then(|place| {
                let (command, app) = state.commands.get(&place, kind, id)?;
                Some((command.clone(), app.clone()))
            });
            match found {
                Some(_) if state.session_id.is_empty() => Prepared::Nothing,
                Some(_) if !state.can_send(channel) => Prepared::NoticeBar,
                Some((command, app)) => {
                    let guild = state.find_channel(channel).and_then(|(guild, _)| guild.map(|g| g.id));
                    let nonce = Snowflake::now();
                    let run = Run {
                        application: command.application,
                        guild,
                        channel,
                        session: &state.session_id,
                        data: command.data_at_once(target),
                        nonce,
                        from,
                    };
                    let body = run.body(&[]);
                    Prepared::Go { command: Box::new(command), app: Box::new(app), body, nonce }
                }
                None => Prepared::Nothing,
            }
        };
        match prepared {
            Prepared::Nothing => {}
            Prepared::NoticeBar => self.update_notice_bar(),
            Prepared::Go { command, app, body, nonce } => {
                self.send_command(channel, *command, &app, body, nonce);
            }
        }
    }

    /// Send the run `body` of `command` to `channel`, as the message queue
    /// does, and show it pending until its app answers. Whether it started.
    fn send_command(
        self: &Rc<Self>,
        channel: Snowflake,
        command: Command,
        app: &CommandApp,
        body: serde_json::Value,
        nonce: Snowflake,
    ) -> bool {
        let rest = self.account.state.borrow().rest.clone();
        let Some(rest) = rest else { return false };
        let guild = self.account.state.borrow().find_channel(channel).and_then(|(guild, _)| guild.map(|g| g.id));
        self.command_used(&command, guild);
        self.account
            .state
            .borrow_mut()
            .running
            .insert(nonce, Running { channel, app: app.name.clone(), created: false });
        self.refresh_command_status();
        let this = self.clone();
        let epoch = self.account.epoch.get();
        glib::spawn_future_local(async move {
            let reopened = this.reopen(&rest, channel).await;
            if this.account.epoch.get() != epoch {
                return;
            }
            if let Err(why) = reopened {
                this.account.state.borrow_mut().running.remove(&nonce);
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
    pub(super) fn command_refused(self: &Rc<Self>, nonce: Snowflake, command: &Command, failure: &Failure) {
        let Some(running) = self.account.state.borrow_mut().running.remove(&nonce) else { return };
        if failure.stale_command {
            self.account.state.borrow_mut().commands_out_of_date(command.application, running.channel);
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
        let state = self.account.state.borrow();
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
        if outcome == Outcome::Failure && self.autocomplete_failed(nonce).is_some() {
            self.update_command_popup();
            return true;
        }
        let Some(running) = self.account.state.borrow_mut().running.remove(&nonce) else { return false };
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
        if let Some(running) = self.account.state.borrow_mut().running.get_mut(&nonce) {
            running.created = true;
        }
        self.refresh_command_status();
    }

    /// The server's apps or commands changed.
    pub(super) fn command_index_changed(self: &Rc<Self>, guild: Snowflake, version: Option<String>) {
        let mut state = self.account.state.borrow_mut();
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
pub(super) struct Debounce {
    waiting: Cell<bool>,
    pending: Cell<bool>,
}

impl Debounce {
    pub(super) fn poke(self: &Rc<Self>, ask: &Rc<dyn Fn()>) {
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
