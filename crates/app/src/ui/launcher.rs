//! The app launcher (621377), opened from the Apps button at the end of the
//! message box or the attach menu's App Launcher: a home with Recents and
//! Apps in this Server, search over apps and commands, a section's View
//! More, and an app's page whose commands are filled in or sent at once.
//! What each lists, and in which order, is `discord::launcher`'s; this is
//! the popover around it. Nothing is asked for beyond the command indexes,
//! the second settings proto and app icons (Q-LA1 (a)).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use discord::Snowflake;
use discord::builtins::BuiltIn;
use discord::cdn;
use discord::commands::{self, App as CommandApp, Command, CommandIndex, Origin, Place, RunFrom, Used, command_type};
use discord::launcher::{self, AppUsage, Section};
use gtk::{gdk, glib, pango};

use super::App;
use super::menus::{self, At, Entry, Item, Kind};
use super::state::now_ms;

/// An app a menu was opened on: a row, a tile or its page.
#[derive(Clone, Copy)]
pub(super) struct AppTarget {
    pub(super) id: Snowflake,
}

/// In Developer Mode (50268), on an app's right-click (`activity-shelf-item-context`)
/// and its page's More (`app-details-more-menu`). Their other items manage
/// the app in server or user settings, report it or open its store,
/// surfaces Neucord doesn't have.
pub(super) const COPY_APP_ID: Entry<AppTarget> = Entry::Item(Item {
    name: "copy-app-id",
    label: "Copy App _ID",
    shows: |app, _| app.account.state.borrow().settings.developer_mode,
    kind: Kind::Action(|app, target| app.copy(&target.id.to_string())),
});

/// App Launcher, in the attach menu in place of Use Apps where the Apps
/// button is turned off (`APP_LAUNCHER` in 316576): opens the launcher, as
/// the button does, without its dismissible-content write (Q-LA2, which
/// this path doesn't send in the client either).
pub(super) const APP_LAUNCHER: Entry<()> = Entry::Item(Item {
    name: "app-launcher",
    label: "App _Launcher",
    shows: |app, _| app.apps_channel().is_some() && app.attach_launcher(),
    kind: Kind::Action(|app, _| app.open_launcher()),
});

/// The launcher's place in the app: the one open, and when the last closed.
#[derive(Default)]
pub(super) struct Slot {
    open: RefCell<Option<Rc<Launcher>>>,
    /// A press on the Apps button closes an open launcher before the click
    /// lands (the popover hides on a press outside it), so a click this
    /// soon after a close was that close, not a new open.
    closed: Cell<Option<Instant>>,
}

/// What a press outside takes to reach the click it starts.
const CLOSED_BY_PRESS: Duration = Duration::from_millis(300);

/// The open launcher, in its channel.
pub(super) struct Launcher {
    app: Weak<App>,
    channel: Snowflake,
    popover: gtk::Popover,
    nav: adw::NavigationView,
    search: gtk::SearchEntry,
    /// The home page's sections, or the search results.
    home: gtk::Box,
    /// Each page's refill, run again when an index or the use arrives.
    refills: RefCell<Vec<Rc<dyn Fn()>>>,
}

/// An app as a row, tile or page shows it.
#[derive(Clone)]
struct AppRow {
    id: Snowflake,
    /// The application's own name (`X2` in 735991), not the bot's.
    name: String,
    description: String,
    icon: Option<String>,
    promoted: bool,
    partner: bool,
    /// The "Discord Built-In" pseudo-app, which has no link, menu or icon.
    built_in: bool,
}

impl AppRow {
    fn of(app: &CommandApp) -> AppRow {
        AppRow {
            id: app.id,
            name: app.app_name.clone(),
            description: app.description.clone(),
            icon: app.icon.clone(),
            promoted: app.promoted(),
            partner: app.partner(),
            built_in: app.id == CommandApp::discord_built_in().id,
        }
    }
}

/// A command as a row shows it.
#[derive(Clone)]
struct CommandRow {
    id: String,
    name: String,
    /// Its description, else its app's.
    description: String,
    app: AppRow,
    /// It has options to fill in, so its row has no Send.
    options: bool,
}

impl CommandRow {
    fn of(command: &Command, app: &CommandApp) -> CommandRow {
        let description =
            if command.display_description.is_empty() { &app.description } else { &command.display_description };
        CommandRow {
            id: command.id.clone(),
            name: command.display_name.clone(),
            description: description.clone(),
            app: AppRow::of(app),
            options: !command.options.is_empty(),
        }
    }
}

/// What the home shows.
enum HomeData {
    Search { commands: Vec<CommandRow>, apps: Vec<AppRow>, loading: bool },
    Home { recents: Vec<AppRow>, server: Option<Vec<AppRow>>, loading: bool, empty: bool },
}

/// An app page's commands.
struct PageData {
    recents: Vec<CommandRow>,
    commands: Vec<CommandRow>,
    popular: Option<Vec<CommandRow>>,
    loading: bool,
}

impl App {
    /// The attach menu offers App Launcher rather than Use Apps.
    pub(super) fn attach_launcher(&self) -> bool {
        discord::chat_bar::attach_launcher(self.chat_bar_customizable(), self.chat_bar_prefs())
    }

    /// The Apps button: open the launcher for the open channel, or close it
    /// if it is open there (`h` in 4025).
    pub(super) fn toggle_launcher(self: &Rc<Self>) {
        let just_closed = self.launcher.closed.get().is_some_and(|at| at.elapsed() < CLOSED_BY_PRESS);
        let channel = self.account.state.borrow().channel;
        let open_here = self.launcher.open.borrow().as_ref().is_some_and(|l| Some(l.channel) == channel);
        if open_here {
            self.close_launcher();
        } else if !just_closed {
            self.open_launcher();
        }
    }

    /// Close the launcher (`APP_LAUNCHER_DISMISS`, and the store's own
    /// closing on a channel change, READY, logout and a command being set).
    pub(super) fn close_launcher(&self) {
        let open = self.launcher.open.borrow().clone();
        if let Some(launcher) = open {
            launcher.popover.popdown();
        }
    }

    /// Open the launcher on its home, for the open channel
    /// (`APP_LAUNCHER_SHOW` with entrypoint TEXT).
    pub(super) fn open_launcher(self: &Rc<Self>) {
        self.close_launcher();
        let Some(channel) = self.account.state.borrow().channel else { return };
        let launcher = Launcher::new(self, channel);
        self.launcher.open.replace(Some(launcher.clone()));
        launcher.popover.popup();
        launcher.search.grab_focus();
        // What the home's hooks fetch when stale (R0, R9): the indexes, as
        // the Apps menus ask, and the use that orders everything.
        let refill = Rc::new({
            let launcher = Rc::downgrade(&launcher);
            move || {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.refill();
                }
            }
        });
        self.load_frecency({
            let refill = refill.clone();
            move || refill()
        });
        launcher.fetch(refill);
    }
}

impl Launcher {
    fn new(app: &Rc<App>, channel: Snowflake) -> Rc<Launcher> {
        let search = gtk::SearchEntry::builder().placeholder_text("Search Apps & Commands").hexpand(true).build();
        let home = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
        let scroller =
            gtk::ScrolledWindow::builder().child(&home).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        body.append(&search);
        body.append(&scroller);
        let home_page = adw::NavigationPage::builder().title("Application Launcher").child(&body).build();
        // Escape closes the whole launcher from any page (`POP_LAYER`).
        let nav = adw::NavigationView::builder().pop_on_escape(false).build();
        nav.add(&home_page);
        // 500 by min(100vh, 680) in the client, whose viewport is what the
        // popover may cover; here that is the window less the message box
        // and the header bar it pops up between.
        let width = app.window.width().saturating_sub(24).clamp(280, 500);
        let height = app.window.height().saturating_sub(160).clamp(240, 680);
        nav.set_size_request(width, height);
        let popover = gtk::Popover::builder()
            .child(&nav)
            .position(gtk::PositionType::Top)
            .halign(gtk::Align::End)
            .has_arrow(false)
            .build();
        // Against the end of the message box's row, not the button (`eS` in
        // 989161), so the attach menu's App Launcher opens it in the same
        // place while the button is hidden.
        let row = &app.composer_box;
        popover.set_pointing_to(Some(&gdk::Rectangle::new(0, 0, row.width(), row.height())));
        popover.set_parent(row);

        let launcher = Rc::new(Launcher {
            app: Rc::downgrade(app),
            channel,
            popover,
            nav,
            search,
            home,
            refills: RefCell::default(),
        });
        launcher.search.connect_changed({
            let launcher = Rc::downgrade(&launcher);
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.fill_home();
                }
            }
        });
        launcher.search.connect_stop_search({
            let popover = launcher.popover.downgrade();
            move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
            }
        });
        launcher.popover.connect_closed({
            let (app, launcher) = (Rc::downgrade(app), Rc::downgrade(&launcher));
            move |popover| {
                if let Some(app) = app.upgrade() {
                    app.launcher.closed.set(Some(Instant::now()));
                    let ours = app
                        .launcher
                        .open
                        .borrow()
                        .as_ref()
                        .is_some_and(|l| launcher.upgrade().is_some_and(|launcher| Rc::ptr_eq(l, &launcher)));
                    if ours {
                        app.launcher.open.take();
                    }
                    // Back to the message box when nothing else took the
                    // focus (`TEXTAREA_FOCUS` in 854379).
                    let lost = gtk::prelude::GtkWindowExt::focus(&app.window)
                        .is_none_or(|focus| focus.is_ancestor(popover) || !focus.is_mapped());
                    if lost && app.composer.is_sensitive() {
                        app.composer.grab_focus();
                    }
                }
                // Once the pick has been handled, which may come after closing.
                let popover = popover.clone();
                glib::idle_add_local_once(move || popover.unparent());
            }
        });
        launcher.fill_home();
        launcher
    }

    /// Ask for the indexes the launcher reads, when stale.
    fn fetch(&self, refill: Rc<dyn Fn()>) {
        let Some(app) = self.app.upgrade() else { return };
        let place = app.account.state.borrow().command_place(self.channel);
        if let Some(place) = place {
            app.fetch_command_indexes(&place, commands::Offer::ALL, "", refill);
        }
    }

    /// Everything shown again, as something it lists arrived.
    fn refill(self: &Rc<Self>) {
        self.fill_home();
        let refills = self.refills.borrow().clone();
        for refill in refills {
            refill();
        }
    }

    /// Read the indexes and the use for this channel.
    fn read<R>(&self, read: impl FnOnce(&CommandIndex, &Place, &AppUsage, &Used, &[Command]) -> R) -> Option<R> {
        let app = self.app.upgrade()?;
        // The built-ins the search and the "Discord Built-In" page list
        // (`includeBuiltIn` in 621377): those offered here.
        let builtins = app.builtin_commands(self.channel, commands::BuiltIns::All);
        let used = app.command_use();
        let usage = app.account.state.borrow_mut().frecency.stores.app_use(now_ms() as u64);
        let state = app.account.state.borrow();
        let place = state.command_place(self.channel)?;
        Some(read(&state.commands, &place, &usage, &used, &builtins))
    }

    fn fill_home(self: &Rc<Self>) {
        let text = self.search.text().to_string();
        let data = self.read(|index, place, usage, used, builtins| {
            if text.is_empty() {
                let home = launcher::home(index, place, usage);
                HomeData::Home {
                    recents: home.recents.iter().map(|a| AppRow::of(a)).collect(),
                    server: home.server.as_ref().map(|apps| apps.iter().map(|a| AppRow::of(a)).collect()),
                    loading: home.loading,
                    empty: home.empty(),
                }
            } else {
                let commands = launcher::search_commands(index, place, &text, used, builtins);
                let apps = launcher::search_apps(index, place, &text, usage, builtins);
                HomeData::Search {
                    commands: commands.iter().map(|hit| CommandRow::of(hit.command, hit.app)).collect(),
                    apps: apps.iter().map(|a| AppRow::of(a)).collect(),
                    loading: index.loading(place),
                }
            }
        });
        clear(&self.home);
        match data {
            None => {}
            Some(HomeData::Search { commands, apps, loading }) => self.fill_search(commands, apps, loading),
            Some(HomeData::Home { recents, server, loading, empty }) => {
                self.fill_sections(recents, server, loading, empty)
            }
        }
    }

    fn fill_sections(self: &Rc<Self>, recents: Vec<AppRow>, server: Option<Vec<AppRow>>, loading: bool, empty: bool) {
        // Recents: hidden while loading and when empty (`lb`).
        if !loading && !recents.is_empty() {
            self.home.append(&header("Recents"));
            let tiles = gtk::Box::builder().spacing(6).build();
            for app in recents.iter().take(launcher::RECENTS) {
                tiles.append(&self.tile(app, Section::RecentApps));
            }
            self.home.append(&tiles);
        }
        // Apps in this Server: in a server, waiting while an index loads
        // (`lL`), hidden when there are none.
        if let Some(server) = server
            && (loading || !server.is_empty())
        {
            self.home.append(&header("Apps in this Server"));
            if loading {
                self.home.append(&spinner());
            } else {
                let list = list();
                for app in &server {
                    list.append(&self.app_row(app, Section::AppsInThisServer));
                }
                self.home.append(&list);
            }
        }
        if empty {
            self.home.append(&notice("Go find some things to do in search!"));
        }
    }

    fn fill_search(self: &Rc<Self>, commands: Vec<CommandRow>, apps: Vec<AppRow>, loading: bool) {
        if commands.is_empty() && apps.is_empty() {
            let shown: gtk::Widget =
                if loading { spinner().upcast() } else { notice("No apps or commands match your search").upcast() };
            self.home.append(&shown);
            return;
        }
        if !commands.is_empty() {
            self.home.append(&header("Commands"));
            let list = list();
            for command in &commands {
                let row = adw::ActionRow::builder()
                    .title(&command.name)
                    .subtitle(&command.description)
                    .subtitle_lines(1)
                    .use_markup(false)
                    .activatable(true)
                    .build();
                row.add_prefix(&self.icon(&command.app, 32, 128));
                row.add_suffix(&gtk::Label::builder().label(&command.app.name).css_classes(["dim-label"]).build());
                // No origin: the run says `slash_ui`, with the entrypoint.
                row.connect_activated({
                    let (launcher, id) = (Rc::downgrade(self), command.id.clone());
                    move |_| {
                        if let Some(launcher) = launcher.upgrade() {
                            launcher.fill_in(&id, launcher::filled_in(Origin::Chat));
                        }
                    }
                });
                list.append(&row);
            }
            self.home.append(&list);
        }
        if !apps.is_empty() {
            self.home.append(&header("Apps"));
            let list = list();
            for app in &apps {
                list.append(&self.app_row(app, Section::Search));
            }
            self.home.append(&list);
        }
    }

    /// An app's icon, `px` wide, asked for at `ask` (128, or 84 on its page)
    /// × the scale, as the client asks whatever it draws.
    fn icon(&self, app: &AppRow, px: i32, ask: u32) -> adw::Avatar {
        let avatar = adw::Avatar::builder().size(px).text(&app.name).show_initials(false).build();
        avatar.set_icon_name(Some("application-x-addon-symbolic"));
        let Some(shell) = self.app.upgrade() else { return avatar };
        let size = cdn::asset_size(ask * shell.images.scale().max(1) as u32);
        // `X2` in 735991 passes the app alone: no bot picture first.
        let url =
            (!app.built_in).then(|| cdn::application_icon(app.id.0, app.icon.as_deref(), None, None, false, size));
        if let Some(url) = url.flatten() {
            let avatar = avatar.downgrade();
            shell.images.load(&url, move |paintable| {
                if let Some(avatar) = avatar.upgrade() {
                    avatar.set_custom_image(Some(&paintable));
                }
            });
        }
        avatar
    }

    /// Recents' look: the icon alone, the name in its tooltip.
    fn tile(self: &Rc<Self>, app: &AppRow, section: Section) -> gtk::Button {
        let button = gtk::Button::builder()
            .child(&self.icon(app, 40, 128))
            .tooltip_text(&app.name)
            .css_classes(["flat"])
            .build();
        button.connect_clicked({
            let (launcher, app) = (Rc::downgrade(self), app.clone());
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.open_app(app.clone(), Some(section));
                }
            }
        });
        self.app_menu(&button, app);
        button
    }

    /// A row's look (`nH`): icon, name with Promoted, one line of
    /// description.
    fn app_row(self: &Rc<Self>, app: &AppRow, section: Section) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&app.name)
            .subtitle(&app.description)
            .subtitle_lines(1)
            .use_markup(false)
            .activatable(true)
            .build();
        row.add_prefix(&self.icon(app, 32, 128));
        if app.promoted {
            row.add_suffix(&gtk::Label::builder().label("Promoted").css_classes(["caption", "dim-label"]).build());
        }
        row.connect_activated({
            let (launcher, app) = (Rc::downgrade(self), app.clone());
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.open_app(app.clone(), Some(section));
                }
            }
        });
        self.app_menu(&row, app);
        row
    }

    /// An app's right-click (`eq` in 621377), not the built-ins'.
    fn app_menu(&self, widget: &impl IsA<gtk::Widget>, app: &AppRow) {
        if app.built_in {
            return;
        }
        let (shell, target, widget_ref) = (self.app.clone(), AppTarget { id: app.id }, widget.as_ref().downgrade());
        menus::on_secondary(widget, move |x, y| {
            if let (Some(shell), Some(widget)) = (shell.upgrade(), widget_ref.upgrade()) {
                menus::popup(&shell, menus::LAUNCHER_APP, target, &widget, At::Point(x, y));
            }
        });
    }

    /// Show `body` as a page with a back button, and `end` at the end of its
    /// header.
    fn push(&self, title: &str, body: &impl IsA<gtk::Widget>, end: Option<&gtk::Box>) {
        let bar = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
        if let Some(end) = end {
            bar.pack_end(end);
        }
        let toolbar = adw::ToolbarView::builder().content(body).build();
        toolbar.add_top_bar(&bar);
        let page = adw::NavigationPage::builder().title(title).child(&toolbar).build();
        self.nav.push(&page);
    }

    /// An app's page (`tM`), opened from `section`.
    fn open_app(self: &Rc<Self>, app: AppRow, section: Option<Section>) {
        // The page's hooks fetch the indexes when stale, as the home's do.
        let refill = Rc::new({
            let launcher = Rc::downgrade(self);
            move || {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.refill();
                }
            }
        });
        self.fetch(refill);

        let end = gtk::Box::builder().spacing(6).build();
        if !app.built_in {
            // Only Copy App ID of the client's More shows here, so only
            // where that would.
            if self.app.upgrade().is_some_and(|a| a.account.state.borrow().settings.developer_mode) {
                let more = gtk::Button::builder().icon_name("view-more-symbolic").tooltip_text("More").build();
                more.connect_clicked({
                    let (shell, target) = (self.app.clone(), AppTarget { id: app.id });
                    move |button| {
                        let Some(shell) = shell.upgrade() else { return };
                        let rect = gdk::Rectangle::new(0, 0, button.width(), button.height());
                        menus::popup(&shell, menus::LAUNCHER_APP, target, button, At::Below(rect));
                    }
                });
                end.append(&more);
            }
        }

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .build();
        body.append(&self.icon(&app, 84, 84));
        let name = gtk::Box::builder().spacing(8).build();
        name.append(
            &gtk::Label::builder()
                .label(&app.name)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .css_classes(["title-2"])
                .build(),
        );
        if app.partner {
            name.append(
                &gtk::Label::builder()
                    .label("Partner")
                    .valign(gtk::Align::Center)
                    .css_classes(["caption-heading", "accent"])
                    .build(),
            );
        }
        body.append(&name);
        if !app.description.is_empty() {
            body.append(&description(&app.description));
        }
        let commands = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        body.append(&commands);
        let scroller =
            gtk::ScrolledWindow::builder().child(&body).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();

        let fill: Rc<dyn Fn()> = Rc::new({
            let (launcher, commands, id) = (Rc::downgrade(self), commands.downgrade(), app.id);
            move || {
                let (Some(launcher), Some(commands)) = (launcher.upgrade(), commands.upgrade()) else { return };
                launcher.fill_page(&commands, id, section);
            }
        });
        fill();
        self.refills.borrow_mut().push(fill);
        self.push(&app.name, &scroller, Some(&end));
    }

    /// The commands block of an app's page (`eA`).
    fn fill_page(self: &Rc<Self>, block: &gtk::Box, id: Snowflake, section: Option<Section>) {
        let data = self.read(|index, place, _, used, builtins| {
            let page = launcher::page(index, place, id, used, builtins);
            let app = if id == CommandApp::discord_built_in().id {
                Some(CommandApp::discord_built_in())
            } else {
                index.app(place, id)
            };
            let row = |command: &&Command| app.map(|app| CommandRow::of(command, app));
            PageData {
                recents: page.recents.iter().filter_map(row).collect(),
                commands: page.commands.iter().filter_map(row).collect(),
                popular: page.popular.map(|popular| popular.iter().filter_map(row).collect()),
                loading: index.loading(place),
            }
        });
        clear(block);
        let Some(data) = data else { return };
        if data.commands.is_empty() {
            if data.loading {
                block.append(&spinner());
            }
            return;
        }
        if !data.recents.is_empty() {
            block.append(&header("Recents"));
            let list = list();
            for command in &data.recents {
                list.append(&self.command_row(command, section));
            }
            block.append(&list);
        }
        // In the client's default order: Popular where the app has one.
        block.append(&header("Use this app"));
        let shown = data.popular.as_ref().unwrap_or(&data.commands);
        let list = list();
        for command in shown {
            list.append(&self.command_row(command, section));
        }
        block.append(&list);
    }

    /// A command of an app's page (`eu`): a chevron where it has options to
    /// fill in, else Send.
    fn command_row(self: &Rc<Self>, command: &CommandRow, section: Option<Section>) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&command.name)
            .subtitle(&command.description)
            .subtitle_lines(1)
            .use_markup(false)
            .activatable(true)
            .build();
        if command.options {
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        } else {
            let send = gtk::Button::builder()
                .label("Send")
                .tooltip_text(format!("Send {}", command.name))
                .valign(gtk::Align::Center)
                .build();
            send.connect_clicked({
                let (launcher, id) = (Rc::downgrade(self), command.id.clone());
                move |_| {
                    if let Some(launcher) = launcher.upgrade() {
                        launcher.send(&id, section);
                    }
                }
            });
            row.add_suffix(&send);
        }
        row.connect_activated({
            let (launcher, id) = (Rc::downgrade(self), command.id.clone());
            move |_| {
                if let Some(launcher) = launcher.upgrade() {
                    launcher.fill_in(&id, launcher::filled_in(Origin::Launcher));
                }
            }
        });
        row
    }

    /// Put the command `id` in the message box to be filled in, from
    /// `from`: the one place the launcher hands a command to the box, which
    /// also closes the launcher (`U.Gf` in 721768).
    fn fill_in(&self, id: &str, from: RunFrom) {
        if let Some(app) = self.app.upgrade() {
            app.activate_command(id, from);
        }
    }

    /// Send (`ep`): run the command at once with no options, with the
    /// section the page was opened from (`MJ` in 735991), then close.
    /// A built-in runs as the box would run it with nothing typed (a text
    /// one posts what it makes), or, with options to fill in, goes in the
    /// box.
    fn send(&self, id: &str, section: Option<Section>) {
        let Some(app) = self.app.upgrade() else { return };
        if BuiltIn::from_id(id).is_some() {
            let command = app.builtin_commands(self.channel, commands::BuiltIns::All).into_iter().find(|c| c.id == id);
            match command {
                Some(command) if command.options.iter().any(|o| o.required) => {
                    self.fill_in(id, launcher::filled_in(Origin::Launcher))
                }
                Some(command) => _ = app.run_built_in(self.channel, &command, &[]),
                None => {}
            }
            return app.close_launcher();
        }
        app.run_at_once(self.channel, command_type::CHAT, id, None, launcher::sent(section));
        app.close_launcher();
    }
}

/// A section's title.
fn header(title: &str) -> gtk::Label {
    gtk::Label::builder().label(title).xalign(0.0).css_classes(["heading"]).build()
}

fn list() -> gtk::ListBox {
    gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build()
}

fn spinner() -> gtk::Spinner {
    gtk::Spinner::builder().spinning(true).halign(gtk::Align::Center).margin_top(12).build()
}

fn notice(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).wrap(true).margin_top(24).css_classes(["dim-label"]).build()
}

/// An app's description.
fn description(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build()
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
