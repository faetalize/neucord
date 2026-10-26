//! The window and everything in it, plus the glue between the UI and the
//! `discord` crate. All of this runs on the GTK main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Duration;

use discord::notifications::{NotificationSettings, Place};
use discord::read_state::ReadStates;
use discord::settings::{GuildFolder, RailEntry};
use discord::{Channel, Event, Gateway, Guild, Message, Ready, Rest, Resume, Snowflake, User, session};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use crate::images::Images;
use crate::render::{self, Names};
use crate::timelines::{self, Timelines};

/// Messages fetched when a channel opens: enough to fill the view, small so
/// the channel appears fast. Also what a channel keeps once we leave it.
const FIRST_PAGE: u8 = 40;
/// Messages per scrollback request (Discord's maximum): these load in the
/// background, so bigger pages just mean fewer round trips.
const OLDER_PAGE: u8 = 100;
/// Start fetching older messages this many screens before reaching the top,
/// so they are usually in place before the user gets there.
const PREFETCH_SCREENS: f64 = 3.0;
/// Consecutive messages by one author within this window share a header.
const GROUP_WINDOW_SECS: i64 = 7 * 60;
/// Avatar/name colour classes in style.css.
const PALETTE_SIZE: u64 = 8;
/// Acks for a channel within this window are sent as one request.
const ACK_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct State {
    token: Option<String>,
    /// The token is already in the keyring.
    persisted: bool,
    /// READY arrived at least once for this token.
    ready_seen: bool,
    reconnect_attempts: u32,
    /// Set while the last session can be resumed; kept across failed
    /// attempts, replaced by each close.
    resume: Option<Resume>,
    rest: Option<Rest>,
    gateway: Option<Gateway>,
    me: Option<User>,
    guilds: Vec<Guild>,
    /// DMs and group DMs.
    private_channels: Vec<Channel>,
    /// The user's server folders, which also order the rail.
    guild_folders: Vec<GuildFolder>,
    /// Folders the user expanded in the rail (by folder id).
    open_folders: std::collections::HashSet<u64>,
    /// The open guild; `None` is Home, the DM list.
    guild: Option<Snowflake>,
    channel: Option<Snowflake>,
    /// Display names by user id, accumulated from every user we see plus lazy
    /// `GET /users/{id}` lookups, so mentions resolve even for people no longer
    /// in the server.
    users: HashMap<u64, String>,
    /// User ids we have already tried to fetch, to avoid repeat requests.
    fetched_users: std::collections::HashSet<u64>,
    read: ReadStates,
    notifications: NotificationSettings,
    /// Acks waiting to be sent: channel → newest message read.
    pending_acks: HashMap<Snowflake, Snowflake>,
}

impl State {
    fn rail_layout(&self) -> Vec<RailEntry> {
        let ids: Vec<Snowflake> = self.guilds.iter().map(|g| g.id).collect();
        discord::settings::rail_layout(&self.guild_folders, &ids)
    }

    /// The first server in rail order, opened on first connect.
    fn first_guild(&self) -> Option<Snowflake> {
        self.rail_layout().into_iter().find_map(|entry| match entry {
            RailEntry::Guild(id) => Some(id),
            RailEntry::Folder { guilds, .. } => guilds.first().copied(),
        })
    }

    fn me(&self) -> Option<Snowflake> {
        self.me.as_ref().map(|u| u.id)
    }

    /// The guild a channel belongs to and the channel itself; `None` guild for DMs.
    fn find_channel(&self, id: Snowflake) -> Option<(Option<&Guild>, &Channel)> {
        if let Some(dm) = self.private_channels.iter().find(|c| c.id == id) {
            return Some((None, dm));
        }
        self.guilds.iter().find_map(|g| g.channels.iter().find(|c| c.id == id).map(|c| (Some(g), c)))
    }

    fn place(&self, id: Snowflake) -> Option<Place> {
        let (guild, channel) = self.find_channel(id)?;
        Some(Place {
            guild: guild.map(|g| g.id),
            guild_default: guild.map_or(0, |g| g.default_message_notifications),
            channel: channel.id,
            parent: channel.parent_id,
        })
    }

    fn is_unread(&self, channel: &Channel) -> bool {
        self.read.is_unread(channel.id, channel.last_message_id)
    }

    /// Unread dot and mention count for a server's rail icon. Muted channels
    /// (and a muted server) add mentions but no dot, as in Discord.
    fn guild_badge(&self, guild: &Guild) -> (bool, u32) {
        let now = now_unix();
        let guild_muted = self.notifications.guild_muted(guild.id, now);
        let mut unread = false;
        let mut mentions = 0;
        for channel in guild.channels.iter().filter(|c| c.kind.is_text()) {
            mentions += self.read.mentions(channel.id);
            if !unread && !guild_muted && self.is_unread(channel) {
                let place =
                    Place { guild: Some(guild.id), guild_default: 0, channel: channel.id, parent: channel.parent_id };
                unread = !self.notifications.channel_muted(&place, now);
            }
        }
        (unread, mentions)
    }

    /// Everything the rail shows that can change without the guild list changing.
    fn rail_signature(&self) -> Vec<u64> {
        let mut sig = vec![self.guild.map_or(0, |g| g.0), self.home_badge() as u64];
        for guild in &self.guilds {
            let (unread, mentions) = self.guild_badge(guild);
            sig.extend([guild.id.0, unread as u64, mentions as u64]);
        }
        sig
    }

    /// The same for the sidebar: the open place, selection, order and markers.
    fn sidebar_signature(&self) -> Vec<u64> {
        let now = now_unix();
        let mut sig = vec![self.guild.map_or(0, |g| g.0), self.channel.map_or(0, |c| c.0)];
        let channels: Vec<(Place, &Channel)> = match self.guild.and_then(|id| self.guilds.iter().find(|g| g.id == id)) {
            Some(guild) => guild
                .channels
                .iter()
                .filter(|c| c.kind.is_text())
                .map(|c| (Place { guild: Some(guild.id), guild_default: 0, channel: c.id, parent: c.parent_id }, c))
                .collect(),
            None => {
                let mut dms: Vec<&Channel> = self.private_channels.iter().collect();
                dms.sort_by_key(|c| std::cmp::Reverse(c.activity()));
                dms.into_iter()
                    .map(|c| (Place { guild: None, guild_default: 0, channel: c.id, parent: None }, c))
                    .collect()
            }
        };
        for (place, channel) in channels {
            sig.extend([
                channel.id.0,
                self.is_unread(channel) as u64,
                self.notifications.channel_muted(&place, now) as u64,
                self.read.mentions(channel.id) as u64,
            ]);
        }
        sig
    }

    /// Home's badge: every DM message counts as a mention.
    fn home_badge(&self) -> u32 {
        self.private_channels.iter().map(|c| self.read.mentions(c.id)).sum()
    }

    /// Record a new newest message for a channel wherever it lives.
    fn bump_last_message(&mut self, channel: Snowflake, message: Snowflake) {
        let slot = self
            .private_channels
            .iter_mut()
            .chain(self.guilds.iter_mut().flat_map(|g| g.channels.iter_mut()))
            .find(|c| c.id == channel);
        if let Some(c) = slot
            && c.last_message_id.is_none_or(|last| message > last)
        {
            c.last_message_id = Some(message);
        }
    }
}

pub struct App {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    loading_label: gtk::Label,
    token_entry: gtk::PasswordEntry,
    login_error: gtk::Label,
    rail: gtk::Box,
    guild_name: gtk::Label,
    channels: gtk::Box,
    me_label: gtk::Label,
    header: gtk::Label,
    /// The list's model; its inner store is the open channel's timeline.
    selection: gtk::NoSelection,
    /// Shown when no channel is open.
    blank: gio::ListStore,
    timelines: RefCell<Timelines>,
    list: gtk::ListView,
    scroller: gtk::ScrolledWindow,
    composer: gtk::TextView,
    composer_error: gtk::Label,
    images: Images,
    /// Mention autocomplete, anchored to the composer.
    mention_popover: gtk::Popover,
    mention_list: gtk::ListBox,
    /// `@`-token start (char offset) and the candidates shown, row-aligned.
    mention: RefCell<MentionCtx>,
    state: RefCell<State>,
    /// What the rail and sidebar last drew, so badge refreshes can skip
    /// rebuilding them when nothing visible changed.
    drawn: RefCell<(Vec<u64>, Vec<u64>)>,
    /// Per-folder colours, regenerated with the rail.
    folder_css: gtk::CssProvider,
}

#[derive(Default)]
struct MentionCtx {
    /// Char offset of the `@` being completed.
    start: i32,
    candidates: Vec<(u64, String)>,
}

impl App {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Neucord")
            .default_width(1200)
            .default_height(800)
            // niri draws the window border; client-side chrome comes later.
            .decorated(false)
            .build();
        window.add_css_class("neucord");

        let blank = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(blank.clone()));
        let this = Rc::new(App {
            window,
            stack: gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build(),
            loading_label: gtk::Label::new(None),
            token_entry: gtk::PasswordEntry::builder().show_peek_icon(true).hexpand(true).build(),
            login_error: gtk::Label::builder().wrap(true).xalign(0.0).visible(false).css_classes(["error"]).build(),
            rail: gtk::Box::new(gtk::Orientation::Vertical, 0),
            guild_name: gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["guild-name"])
                .build(),
            channels: gtk::Box::new(gtk::Orientation::Vertical, 0),
            me_label: gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["me"])
                .build(),
            header: gtk::Label::builder().xalign(0.0).css_classes(["chat-header"]).build(),
            list: gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>),
            selection,
            blank,
            timelines: RefCell::default(),
            scroller: gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build(),
            composer: gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .accepts_tab(false)
                .hexpand(true)
                .css_classes(["composer-input"])
                .build(),
            composer_error: gtk::Label::builder()
                .xalign(0.0)
                .wrap(true)
                .visible(false)
                .css_classes(["composer-error"])
                .build(),
            images: Images::new(),
            mention_popover: gtk::Popover::builder()
                .autohide(false)
                .has_arrow(false)
                .position(gtk::PositionType::Top)
                .css_classes(["mention-popover"])
                .build(),
            mention_list: gtk::ListBox::builder().css_classes(["mention-list"]).build(),
            mention: RefCell::default(),
            state: RefCell::default(),
            drawn: RefCell::default(),
            folder_css: gtk::CssProvider::new(),
        });
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&this.window),
            &this.folder_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        this.build();

        // Handlers hold weak references, so something must own the App while
        // the window is open. Released on close, which breaks the cycle.
        let keep_alive = RefCell::new(Some(this.clone()));
        let popover = this.mention_popover.clone();
        this.window.connect_close_request(move |_| {
            // A manually-parented popover must be unparented before teardown.
            popover.unparent();
            keep_alive.take();
            glib::Propagation::Proceed
        });
        this
    }

    pub fn start(self: &Rc<Self>) {
        self.window.present();
        self.show_loading("Checking keyring…");
        let this = self.clone();
        glib::spawn_future_local(async move {
            match session::load_token().await {
                Ok(Some(token)) => this.connect(token, true),
                Ok(None) => this.show_login(None),
                Err(e) => this.show_login(Some(&e.to_string())),
            }
        });
    }

    // ---- layout -----------------------------------------------------------

    fn build(self: &Rc<Self>) {
        self.stack.add_named(&self.build_loading(), Some("loading"));
        self.stack.add_named(&self.build_login(), Some("login"));
        self.stack.add_named(&self.build_main(), Some("main"));
        self.window.set_child(Some(&self.stack));

        let logout = gio::ActionEntry::builder("logout")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &gtk::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.logout();
                    }
                }
            })
            .build();
        self.window.add_action_entries([logout]);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.logout", &["<Control><Shift>l"]);
            // Notification clicks arrive as an app action, with the channel id.
            let open = gio::ActionEntry::builder("open-channel")
                .parameter_type(Some(glib::VariantTy::STRING))
                .activate({
                    let this = Rc::downgrade(self);
                    move |_: &gtk::Application, _, parameter| {
                        let id = parameter.and_then(|p| p.str()).and_then(|s| s.parse().ok());
                        if let (Some(this), Some(id)) = (this.upgrade(), id) {
                            this.open_channel(Snowflake(id));
                        }
                    }
                })
                .build();
            app.add_action_entries([open]);
        }

        // Coming back to the window reads the open channel.
        self.window.connect_is_active_notify({
            let this = Rc::downgrade(self);
            move |window| {
                let Some(this) = this.upgrade() else { return };
                let channel = this.state.borrow().channel;
                if let (true, Some(channel)) = (window.is_active(), channel) {
                    this.mark_read(channel);
                }
            }
        });
    }

    fn build_loading(&self) -> gtk::Widget {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        page.append(&gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build());
        self.loading_label.add_css_class("hint");
        page.append(&self.loading_label);
        wrap_page(&page)
    }

    fn build_login(self: &Rc<Self>) -> gtk::Widget {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .css_classes(["card"])
            .build();
        card.append(&gtk::Label::builder().label("Neucord").xalign(0.0).css_classes(["card-title"]).build());
        card.append(
            &gtk::Label::builder()
                .label("Paste your Discord token to log in. It is kept in your desktop keyring, never in a file.")
                .wrap(true)
                .max_width_chars(44)
                .xalign(0.0)
                .css_classes(["hint"])
                .build(),
        );
        card.append(&self.token_entry);
        card.append(&self.login_error);
        let button = gtk::Button::builder().label("Log in").css_classes(["login-button"]).build();
        card.append(&button);

        let login = {
            let this = Rc::downgrade(self);
            move || {
                if let Some(this) = this.upgrade() {
                    this.login();
                }
            }
        };
        self.token_entry.connect_activate({
            let login = login.clone();
            move |_| login()
        });
        button.connect_clicked(move |_| login());
        wrap_page(&card)
    }

    fn build_main(self: &Rc<Self>) -> gtk::Widget {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        // Server rail
        let rail_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&self.rail)
            .width_request(72)
            .css_classes(["rail"])
            .build();
        self.rail.set_margin_top(8);
        root.append(&rail_scroller);

        // Channel sidebar
        let sidebar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(240)
            .css_classes(["sidebar"])
            .build();
        sidebar.append(&self.guild_name);
        sidebar.append(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&self.channels)
                .build(),
        );
        sidebar.append(&self.me_label);
        root.append(&sidebar);

        // Chat
        let chat =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).css_classes(["chat"]).build();
        chat.append(&self.header);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_activatable(false);
            item.set_child(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 16)));
        });
        factory.connect_bind({
            let this = Rc::downgrade(self);
            move |_, item| {
                let Some(this) = this.upgrade() else { return };
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                let row = item.child().and_downcast::<gtk::Box>().expect("row box");
                let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { return };
                let message = message.borrow::<Message>().clone();
                let messages = this.messages();
                let previous = item.position().checked_sub(1).and_then(|p| timelines::message_at(&messages, p));
                let names = this.message_names(&message);
                fill_message_row(&row, &message, previous.as_ref(), &names, Some(&this.images));
            }
        });
        self.list.set_factory(Some(&factory));
        self.list.add_css_class("messages");
        self.scroller.set_child(Some(&self.list));
        chat.append(&self.scroller);

        // Scrollback: fetch older messages well before the top is reached.
        self.scroller.vadjustment().connect_value_changed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.maybe_load_older();
                }
            }
        });

        chat.append(&self.composer_error);
        let composer_box = gtk::Box::builder().css_classes(["composer"]).build();
        composer_box.append(&self.composer);
        chat.append(&composer_box);

        // Mention autocomplete popup, anchored to the composer.
        self.mention_popover.set_child(Some(&self.mention_list));
        self.mention_popover.set_parent(&self.composer);
        self.mention_list.connect_row_activated({
            let this = Rc::downgrade(self);
            move |_, row| {
                if let Some(this) = this.upgrade() {
                    this.accept_mention(row.index());
                }
            }
        });
        self.composer.buffer().connect_changed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.update_mention_popup();
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let this = Rc::downgrade(self);
            move |_, key, _, modifiers| {
                let Some(this) = this.upgrade() else { return glib::Propagation::Proceed };
                use gtk::gdk::Key;
                // While the mention popup is open, it owns the navigation keys.
                if this.mention_popover.is_visible() {
                    match key {
                        Key::Escape => {
                            this.mention_popover.popdown();
                            return glib::Propagation::Stop;
                        }
                        Key::Down | Key::Tab => {
                            this.move_mention_selection(1);
                            return glib::Propagation::Stop;
                        }
                        Key::Up => {
                            this.move_mention_selection(-1);
                            return glib::Propagation::Stop;
                        }
                        Key::Return | Key::KP_Enter => {
                            let index = this.mention_list.selected_row().map(|r| r.index()).unwrap_or(0);
                            this.accept_mention(index);
                            return glib::Propagation::Stop;
                        }
                        _ => {}
                    }
                }
                let enter = matches!(key, Key::Return | Key::KP_Enter);
                if enter && !modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    this.send_composer();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        self.composer.add_controller(keys);

        root.append(&chat);
        root.upcast()
    }

    // ---- pages ------------------------------------------------------------

    fn show_loading(&self, text: &str) {
        self.loading_label.set_label(text);
        self.stack.set_visible_child_name("loading");
    }

    fn show_login(&self, error: Option<&str>) {
        self.login_error.set_label(error.unwrap_or_default());
        self.login_error.set_visible(error.is_some());
        self.stack.set_visible_child_name("login");
        self.token_entry.grab_focus();
    }

    fn show_composer_error(&self, text: &str) {
        self.composer_error.set_label(text);
        self.composer_error.set_visible(true);
        let label = self.composer_error.downgrade();
        let shown = text.to_owned();
        glib::timeout_add_local_once(Duration::from_secs(8), move || {
            if let Some(label) = label.upgrade()
                && label.label() == shown
            {
                label.set_visible(false);
            }
        });
    }

    // ---- session ----------------------------------------------------------

    fn login(self: &Rc<Self>) {
        // Tokens copied from devtools often come wrapped in quotes.
        let token = self.token_entry.text().trim().trim_matches('"').to_owned();
        self.token_entry.set_text("");
        if token.is_empty() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.ready_seen = false;
            state.reconnect_attempts = 0;
        }
        self.connect(token, false);
    }

    fn logout(self: &Rc<Self>) {
        *self.state.borrow_mut() = State::default();
        self.show_blank();
        self.timelines.borrow_mut().clear();
        glib::spawn_future_local(async {
            if let Err(e) = session::clear_token().await {
                eprintln!("logout: {e}");
            }
        });
        self.show_login(None);
    }

    fn connect(self: &Rc<Self>, token: String, persisted: bool) {
        {
            let mut state = self.state.borrow_mut();
            state.rest = Some(Rest::new(token.clone()));
            state.token = Some(token.clone());
            state.persisted = persisted;
            state.gateway = None;
        }
        if !self.state.borrow().ready_seen {
            self.show_loading("Connecting to Discord…");
        }
        let resume = self.state.borrow().resume.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            match Gateway::connect(&token, resume).await {
                Ok((gateway, events)) => {
                    this.state.borrow_mut().gateway = Some(gateway);
                    this.run_events(events).await;
                }
                Err(e) => this.connection_lost(&e.to_string()),
            }
        });
    }

    /// Transport-level failure or a non-fatal close: retry with backoff if
    /// this token is known good, otherwise send the user back to login.
    fn connection_lost(self: &Rc<Self>, why: &str) {
        let (known_good, attempts, token, resumable) = {
            let mut state = self.state.borrow_mut();
            state.gateway = None;
            state.reconnect_attempts += 1;
            (state.persisted || state.ready_seen, state.reconnect_attempts, state.token.clone(), state.resume.is_some())
        };
        let Some(token) = token.filter(|_| known_good) else {
            return self.show_login(Some(why));
        };
        // A resumable session is worth retrying at once: Discord only keeps
        // it for a short while. After that, back off.
        let delay = if resumable && attempts == 1 { 1 } else { 2u64.saturating_pow(attempts.min(6)).min(60) };
        let status = format!("Disconnected ({why}). Reconnecting in {delay}s…");
        if self.state.borrow().ready_seen {
            self.me_label.set_label(&status);
        } else {
            self.show_loading(&status);
        }
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            if let Some(this) = this.upgrade() {
                // Logged out meanwhile?
                if this.state.borrow().token.as_ref() == Some(&token) {
                    let persisted = this.state.borrow().persisted;
                    this.connect(token, persisted);
                }
            }
        });
    }

    async fn run_events(self: &Rc<Self>, events: async_channel::Receiver<Event>) {
        while let Ok(event) = events.recv().await {
            match event {
                Event::Ready(ready) => self.on_ready(*ready),
                Event::GuildCreate(guild) => self.on_guild(guild),
                Event::PrivateChannelCreate(channel) => self.on_private_channel(channel),
                Event::PrivateChannelDelete(id) => self.on_private_channel_delete(id),
                Event::MessageCreate(message) => self.on_message(message),
                Event::MessageUpdate { id, channel_id, content, edited } => {
                    self.on_message_update(id, channel_id, content, edited)
                }
                Event::MessageDelete { id, channel_id } => self.on_message_delete(id, channel_id),
                Event::MessageAck { channel_id, message_id } => {
                    self.state.borrow_mut().read.ack(channel_id, message_id);
                    self.withdraw_notification(channel_id);
                    self.refresh_badges();
                }
                Event::ChannelUnreadUpdate { channels, .. } => {
                    {
                        let mut state = self.state.borrow_mut();
                        for (channel, last) in channels {
                            state.bump_last_message(channel, last);
                        }
                    }
                    self.refresh_badges();
                }
                Event::GuildFoldersUpdate(folders) => {
                    self.state.borrow_mut().guild_folders = folders;
                    self.rebuild_rail();
                }
                Event::NotificationSettingsUpdate(settings) => {
                    self.state.borrow_mut().notifications.update(settings);
                    self.refresh_badges();
                }
                Event::Resumed => {
                    let name = {
                        let mut state = self.state.borrow_mut();
                        state.reconnect_attempts = 0;
                        state.me.as_ref().map(|u| u.display_name().to_owned()).unwrap_or_default()
                    };
                    self.me_label.set_label(&name);
                    // Replayed messages have already updated the open channel.
                }
                Event::Closed { code, reason, resume } => {
                    self.state.borrow_mut().resume = resume;
                    if Event::is_fatal_close(code) {
                        *self.state.borrow_mut() = State::default();
                        glib::spawn_future_local(async {
                            let _ = session::clear_token().await;
                        });
                        self.show_login(Some(&format!(
                            "Discord rejected the token (close code {code}). Log in again."
                        )));
                    } else if self.state.borrow().token.is_some() {
                        let why = if reason.is_empty() { format!("code {code}") } else { reason };
                        self.connection_lost(&why);
                    }
                    return;
                }
            }
        }
    }

    // ---- gateway events ---------------------------------------------------

    fn on_ready(self: &Rc<Self>, ready: Ready) {
        let Ready { user, guilds, private_channels, read_states, notification_settings, guild_folders } = ready;
        self.learn_users(private_channels.iter().flat_map(|c| &c.recipients));
        let (store, was_ready) = {
            let mut state = self.state.borrow_mut();
            let was_ready = state.ready_seen;
            state.ready_seen = true;
            state.reconnect_attempts = 0;
            state.guilds = guilds;
            state.private_channels = private_channels;
            state.read = read_states;
            state.guild_folders = guild_folders;
            state.notifications = notification_settings;
            let store = (!state.persisted).then(|| (state.token.clone(), user.username.clone()));
            state.persisted = true;
            state.users.insert(user.id.0, user.display_name().to_string());
            state.me = Some(user);
            (store, was_ready)
        };
        if let Some((Some(token), username)) = store {
            glib::spawn_future_local(async move {
                if let Err(e) = session::store_token(&token, &format!("Neucord ({username})")).await {
                    eprintln!("could not save token: {e}");
                }
            });
        }

        let me = self.state.borrow().me.as_ref().map(|u| u.display_name().to_owned()).unwrap_or_default();
        self.me_label.set_label(&me);
        self.stack.set_visible_child_name("main");

        // Keep the open guild (or Home) across reconnects; on first connect
        // open the first guild, or Home if there are none.
        let open = {
            let state = self.state.borrow();
            match state.guild {
                Some(id) if state.guilds.iter().any(|g| g.id == id) => Some(id),
                None if was_ready => None,
                _ => state.first_guild(),
            }
        };
        // After a reconnect that could not resume, stay in the open channel.
        let keep = {
            let state = self.state.borrow();
            state.channel.filter(|&c| was_ready && state.find_channel(c).is_some_and(|(g, _)| g.map(|g| g.id) == open))
        };
        match (keep, open) {
            (Some(channel), _) => {
                self.state.borrow_mut().guild = open;
                self.rebuild_rail();
                self.select_channel(channel);
            }
            (None, Some(id)) => self.select_guild(id),
            (None, None) => self.select_home(),
        }
    }

    fn on_private_channel(self: &Rc<Self>, channel: Channel) {
        self.learn_users(channel.recipients.iter());
        {
            let mut state = self.state.borrow_mut();
            state.private_channels.retain(|c| c.id != channel.id);
            state.private_channels.push(channel);
        }
        if self.state.borrow().guild.is_none() {
            self.rebuild_sidebar();
        }
    }

    fn on_private_channel_delete(self: &Rc<Self>, id: Snowflake) {
        let (home, was_open) = {
            let mut state = self.state.borrow_mut();
            state.private_channels.retain(|c| c.id != id);
            (state.guild.is_none(), state.channel == Some(id))
        };
        if was_open {
            self.select_home();
        } else if home {
            self.rebuild_sidebar();
        }
    }

    fn on_guild(self: &Rc<Self>, guild: Guild) {
        let id = guild.id;
        let is_open = {
            let mut state = self.state.borrow_mut();
            match state.guilds.iter_mut().find(|g| g.id == id) {
                Some(existing) => *existing = guild,
                None => state.guilds.push(guild),
            }
            state.guild == Some(id)
        };
        self.rebuild_rail();
        if is_open {
            self.rebuild_sidebar();
        }
    }

    fn on_message(self: &Rc<Self>, message: Message) {
        self.learn_users(std::iter::once(&message.author).chain(&message.mentions));
        let channel = message.channel_id;
        let viewing = self.state.borrow().channel == Some(channel) && self.window.is_active();
        let notify = {
            let mut state = self.state.borrow_mut();
            state.bump_last_message(channel, message.id);
            let own = state.me() == Some(message.author.id);
            if own || viewing {
                // Discord marks what we send as read; what we watch arrive, we ack.
                state.read.ack(channel, message.id);
                None
            } else {
                let place = state.place(channel);
                let roles = message
                    .guild_id
                    .and_then(|g| state.guilds.iter().find(|guild| guild.id == g))
                    .map(|g| g.my_roles.clone())
                    .unwrap_or_default();
                let is_dm = place.is_some_and(|p| p.guild.is_none());
                let mentioned =
                    is_dm || state.me().is_some_and(|me| state.notifications.mentions_me(&message, me, &roles));
                state.read.on_message(channel, mentioned);
                place.filter(|p| state.notifications.should_notify(p, mentioned, now_unix()))
            }
        };
        if viewing && self.state.borrow().me() != Some(message.author.id) {
            self.queue_ack(channel, message.id);
        }
        if let Some(place) = notify {
            self.send_notification(&message, &place);
        }
        self.refresh_badges();

        // Keep every cached timeline current, not only the open one.
        let Some(store) = self.timelines.borrow().get(channel).map(|t| t.store.clone()) else { return };
        // A history fetch may have raced this event and already added it.
        if timelines::find(&store, message.id).is_some() {
            return;
        }
        let open = self.state.borrow().channel == Some(channel);
        let at_bottom = open && self.at_bottom();
        if open {
            self.resolve_mentions(&message.content);
        }
        store.append(&timelines::boxed(message));
        if at_bottom {
            self.scroll_to_end();
        }
    }

    fn at_bottom(&self) -> bool {
        let adjustment = self.scroller.vadjustment();
        adjustment.value() + adjustment.page_size() >= adjustment.upper() - 80.0
    }

    fn on_message_update(&self, id: Snowflake, channel_id: Snowflake, content: Option<String>, edited: bool) {
        // Embed-only edits carry no content; nothing to re-render yet.
        let Some(content) = content else { return };
        let Some(store) = self.timelines.borrow().get(channel_id).map(|t| t.store.clone()) else { return };
        if let Some((position, mut message)) = timelines::find(&store, id) {
            message.content = content;
            if edited && message.edited_timestamp.is_none() {
                message.edited_timestamp = Some(String::new());
            }
            store.splice(position, 1, &[timelines::boxed(message)]);
        }
    }

    fn on_message_delete(&self, id: Snowflake, channel_id: Snowflake) {
        let Some(store) = self.timelines.borrow().get(channel_id).map(|t| t.store.clone()) else { return };
        if let Some((position, _)) = timelines::find(&store, id) {
            store.remove(position);
            // The message after it may now group with the one before.
            if position < store.n_items() {
                store.items_changed(position, 1, 1);
            }
        }
    }

    /// The open channel's store (or the blank one).
    fn messages(&self) -> gio::ListStore {
        self.selection.model().and_downcast().unwrap_or_else(|| self.blank.clone())
    }

    fn show_blank(&self) {
        self.selection.set_model(Some(&self.blank));
    }

    // ---- navigation -------------------------------------------------------

    fn rebuild_rail(self: &Rc<Self>) {
        while let Some(child) = self.rail.first_child() {
            self.rail.remove(&child);
        }
        let state = self.state.borrow();
        self.drawn.borrow_mut().0 = state.rail_signature();

        let home =
            gtk::Button::builder().label("DM").tooltip_text("Direct Messages").css_classes(["guild", "home"]).build();
        if state.guild.is_none() {
            home.add_css_class("active");
        }
        home.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.select_home();
                }
            }
        });
        self.rail.append(&rail_item(&home, false, state.home_badge()));
        self.rail.append(&gtk::Separator::builder().css_classes(["rail-separator"]).build());

        let mut folder_css = String::new();
        let guild = |id: Snowflake| state.guilds.iter().find(|g| g.id == id);
        for entry in state.rail_layout() {
            match entry {
                RailEntry::Guild(id) => {
                    if let Some(guild) = guild(id) {
                        self.rail.append(&self.guild_item(&state, guild));
                    }
                }
                RailEntry::Folder { id, name, color, guilds } => {
                    let members: Vec<&Guild> = guilds.iter().filter_map(|g| guild(*g)).collect();
                    let class = format!("folder-{id}");
                    if let Some(color) = color {
                        let (r, g, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
                        let _ = writeln!(
                            folder_css,
                            ".folder.{class}:not(.expanded) {{ background: rgba({r}, {g}, {b}, 0.35); }}\n\
                             .folder.{class} image {{ color: rgb({r}, {g}, {b}); }}"
                        );
                    }
                    let expanded = state.open_folders.contains(&id);
                    let button = gtk::Button::builder()
                        .tooltip_text(name.as_deref().unwrap_or("Folder"))
                        .css_classes(["guild", "folder", &class])
                        .overflow(gtk::Overflow::Hidden)
                        .build();
                    if expanded {
                        button.add_css_class("expanded");
                        button.set_child(Some(&gtk::Image::from_icon_name("folder-symbolic")));
                    } else {
                        button.set_child(Some(&folder_preview(&members, &self.images)));
                        if members.iter().any(|g| state.guild == Some(g.id)) {
                            button.add_css_class("active");
                        }
                    }
                    button.connect_clicked({
                        let this = Rc::downgrade(self);
                        move |_| {
                            if let Some(this) = this.upgrade() {
                                this.toggle_folder(id);
                            }
                        }
                    });
                    if expanded {
                        let group = gtk::Box::builder()
                            .orientation(gtk::Orientation::Vertical)
                            .css_classes(["folder-open"])
                            .build();
                        group.append(&rail_item(&button, false, 0));
                        for member in &members {
                            group.append(&self.guild_item(&state, member));
                        }
                        self.rail.append(&group);
                    } else {
                        // A collapsed folder carries its servers' markers.
                        let (unread, mentions) = members.iter().fold((false, 0), |(unread, mentions), g| {
                            let (u, m) = state.guild_badge(g);
                            (unread || u, mentions + m)
                        });
                        self.rail.append(&rail_item(&button, unread, mentions));
                    }
                }
            }
        }
        self.folder_css.load_from_string(&folder_css);
    }

    /// One server's rail button with its unread pill and mention count.
    fn guild_item(self: &Rc<Self>, state: &State, guild: &Guild) -> gtk::Widget {
        let button = gtk::Button::builder()
            .tooltip_text(&guild.name)
            .css_classes(["guild"])
            // Clip the icon to the button's rounded corners.
            .overflow(gtk::Overflow::Hidden)
            .build();
        match guild.icon.as_deref() {
            Some(hash) => {
                let picture = gtk::Picture::builder()
                    .width_request(48)
                    .height_request(48)
                    .content_fit(gtk::ContentFit::Cover)
                    .overflow(gtk::Overflow::Hidden)
                    .build();
                self.images.set(
                    &picture,
                    &discord::cdn::guild_icon_url(guild.id.0, hash, 64),
                    discord::cdn::is_animated(hash),
                );
                button.set_child(Some(&picture));
            }
            None => button.set_label(&initials(&guild.name)),
        }
        if state.guild == Some(guild.id) {
            button.add_css_class("active");
        }
        let id = guild.id;
        let this = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.select_guild(id);
            }
        });
        let (unread, mentions) = state.guild_badge(guild);
        rail_item(&button, unread, mentions)
    }

    fn toggle_folder(self: &Rc<Self>, id: u64) {
        {
            let mut state = self.state.borrow_mut();
            if !state.open_folders.remove(&id) {
                state.open_folders.insert(id);
            }
        }
        self.rebuild_rail();
    }

    fn select_guild(self: &Rc<Self>, id: Snowflake) {
        let first_channel = {
            let mut state = self.state.borrow_mut();
            state.guild = Some(id);
            state
                .guilds
                .iter()
                .find(|g| g.id == id)
                .and_then(|g| g.sidebar().first().and_then(|(_, chans)| chans.first().map(|c| c.id)))
        };
        self.rebuild_rail();
        match first_channel {
            Some(channel) => self.select_channel(channel),
            None => {
                self.state.borrow_mut().channel = None;
                self.show_blank();
                self.header.set_label("No text channels you can see here");
                self.rebuild_sidebar();
            }
        }
    }

    /// Open Home: the DM list, with the most recent DM selected.
    fn select_home(self: &Rc<Self>) {
        let latest = {
            let mut state = self.state.borrow_mut();
            state.guild = None;
            state.private_channels.iter().max_by_key(|c| c.activity()).map(|c| c.id)
        };
        self.rebuild_rail();
        match latest {
            Some(channel) => self.select_channel(channel),
            None => {
                self.state.borrow_mut().channel = None;
                self.show_blank();
                self.header.set_label("No direct messages yet");
                self.rebuild_sidebar();
            }
        }
    }

    fn rebuild_sidebar(self: &Rc<Self>) {
        while let Some(child) = self.channels.first_child() {
            self.channels.remove(&child);
        }
        let state = self.state.borrow();
        self.drawn.borrow_mut().1 = state.sidebar_signature();
        let Some(id) = state.guild else {
            self.guild_name.set_label("Direct Messages");
            let mut dms: Vec<&Channel> = state.private_channels.iter().collect();
            dms.sort_by_key(|c| std::cmp::Reverse(c.activity()));
            for dm in dms {
                let row = dm_row(dm, &self.images);
                if let Some(badge) = badge(state.read.mentions(dm.id)) {
                    row.append(&badge);
                }
                let button = gtk::Button::builder().child(&row).css_classes(["channel", "dm"]).build();
                if state.channel == Some(dm.id) {
                    button.add_css_class("active");
                }
                if state.is_unread(dm) {
                    button.add_css_class("unread");
                }
                let place = Place { guild: None, guild_default: 0, channel: dm.id, parent: None };
                if state.notifications.channel_muted(&place, now_unix()) {
                    button.add_css_class("muted");
                }
                let id = dm.id;
                let this = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(this) = this.upgrade() {
                        this.select_channel(id);
                    }
                });
                self.channels.append(&button);
            }
            return;
        };
        let Some(guild) = state.guilds.iter().find(|g| g.id == id) else {
            return;
        };
        self.guild_name.set_label(&guild.name);
        for (category, channels) in guild.sidebar() {
            if let Some(category) = category {
                self.channels.append(
                    &gtk::Label::builder()
                        .label(category.to_uppercase())
                        .xalign(0.0)
                        .ellipsize(pango::EllipsizeMode::End)
                        .css_classes(["category"])
                        .build(),
                );
            }
            for channel in channels {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                row.append(
                    &gtk::Label::builder()
                        .label(format!("# {}", channel.name.as_deref().unwrap_or("unnamed")))
                        .xalign(0.0)
                        .hexpand(true)
                        .ellipsize(pango::EllipsizeMode::End)
                        .build(),
                );
                if let Some(badge) = badge(state.read.mentions(channel.id)) {
                    row.append(&badge);
                }
                let button = gtk::Button::builder().child(&row).css_classes(["channel"]).build();
                if state.channel == Some(channel.id) {
                    button.add_css_class("active");
                }
                let place = Place {
                    guild: Some(guild.id),
                    guild_default: guild.default_message_notifications,
                    channel: channel.id,
                    parent: channel.parent_id,
                };
                if state.notifications.channel_muted(&place, now_unix()) {
                    button.add_css_class("muted");
                } else if state.is_unread(channel) {
                    button.add_css_class("unread");
                }
                let id = channel.id;
                let this = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(this) = this.upgrade() {
                        this.select_channel(id);
                    }
                });
                self.channels.append(&button);
            }
        }
    }

    fn select_channel(self: &Rc<Self>, id: Snowflake) {
        let (title, rest, left) = {
            let mut state = self.state.borrow_mut();
            let left = state.channel.filter(|&c| c != id);
            state.channel = Some(id);
            let title = match state.private_channels.iter().find(|c| c.id == id) {
                Some(dm) if dm.kind == discord::ChannelKind::Dm => format!("@ {}", dm.title()),
                Some(group) => group.title(),
                None => {
                    let name = state
                        .guilds
                        .iter()
                        .flat_map(|g| &g.channels)
                        .find(|c| c.id == id)
                        .and_then(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("# {name}")
                }
            };
            (title, state.rest.clone(), left)
        };
        // The channel we left keeps only its newest messages: live events keep
        // those current, and coming back refetches the newest page anyway.
        if let Some(left) = left
            && let Some(timeline) = self.timelines.borrow_mut().get_mut(left)
        {
            timelines::keep_newest(timeline, FIRST_PAGE as u32);
        }
        self.rebuild_sidebar();
        self.header.set_label(&title);
        self.composer_error.set_visible(false);

        // Show what we have at once (instant for a recently viewed channel),
        // then fetch the newest page and merge it in.
        let store = self.timelines.borrow_mut().open(id).store.clone();
        self.selection.set_model(Some(&store));
        self.scroll_to_end();

        let Some(rest) = rest else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(id, FIRST_PAGE, None).await;
            match result {
                Ok(page) => {
                    this.learn_messages(&page);
                    let full = page.len() == FIRST_PAGE as usize;
                    let open = this.state.borrow().channel == Some(id);
                    let at_bottom = !open || this.at_bottom();
                    let replaced = timelines::merge_newest(&store, page);
                    if let Some(timeline) = this.timelines.borrow_mut().get_mut(id) {
                        // A short page is the whole channel; a replaced store
                        // lost its scrollback and can page again.
                        if !full {
                            timeline.complete = true;
                        } else if replaced {
                            timeline.complete = false;
                        }
                    }
                    if let Some(newest) = store.n_items().checked_sub(1).and_then(|p| timelines::message_at(&store, p))
                    {
                        // READY's newest-message ids can be stale; history is not.
                        this.state.borrow_mut().bump_last_message(id, newest.id);
                    }
                    // The user may have switched channels while this was loading.
                    if open && this.state.borrow().channel == Some(id) {
                        if at_bottom {
                            this.scroll_to_end();
                        }
                        this.mark_read(id);
                        this.load_older_if_short();
                    }
                }
                Err(e) => {
                    if this.state.borrow().channel == Some(id) {
                        this.show_composer_error(&format!("Could not load messages: {e}"));
                    }
                }
            }
        });
    }

    /// Learn names from a batch of messages and look up unknown mentions.
    fn learn_messages(self: &Rc<Self>, messages: &[Message]) {
        self.learn_users(messages.iter().flat_map(|m| std::iter::once(&m.author).chain(&m.mentions)));
        for message in messages {
            self.resolve_mentions(&message.content);
        }
    }

    /// Fetch the page before the oldest loaded message when the user is within
    /// a few screens of the top. One request per channel at a time.
    fn maybe_load_older(self: &Rc<Self>) {
        let adjustment = self.scroller.vadjustment();
        if adjustment.value() > adjustment.page_size() * PREFETCH_SCREENS {
            return;
        }
        let Some(channel) = self.state.borrow().channel else { return };
        let (store, before) = {
            let mut timelines = self.timelines.borrow_mut();
            let Some(timeline) = timelines.get_mut(channel) else { return };
            if timeline.complete || timeline.loading_older {
                return;
            }
            // Nothing loaded yet: the newest page is still on its way.
            let Some(before) = timelines::oldest_id(&timeline.store) else { return };
            timeline.loading_older = true;
            (timeline.store.clone(), before)
        };
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(channel, OLDER_PAGE, Some(before)).await;
            let still_cached = this.timelines.borrow().get(channel).is_some_and(|t| t.store == store);
            if let Some(timeline) = this.timelines.borrow_mut().get_mut(channel) {
                timeline.loading_older = false;
            }
            if !still_cached {
                return;
            }
            match result {
                // Only prepend if the store still starts where we asked from;
                // a merge that replaced it would make this page a gap.
                Ok(page) if timelines::oldest_id(&store) == Some(before) => {
                    if page.len() < OLDER_PAGE as usize
                        && let Some(timeline) = this.timelines.borrow_mut().get_mut(channel)
                    {
                        timeline.complete = true;
                    }
                    this.learn_messages(&page);
                    timelines::prepend(&store, page);
                    this.load_older_if_short();
                }
                Ok(_) => {}
                Err(e) => {
                    if this.state.borrow().channel == Some(channel) {
                        this.show_composer_error(&format!("Could not load older messages: {e}"));
                    }
                }
            }
        });
    }

    /// After layout, page again if the loaded messages still don't reach far
    /// enough up (a short page, or a tall window).
    fn load_older_if_short(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(this) = this.upgrade() {
                this.maybe_load_older();
            }
        });
    }

    /// We are looking at `channel`: if it has anything unread, ack its newest
    /// message here and on Discord.
    fn mark_read(self: &Rc<Self>, channel: Snowflake) {
        let newest = {
            let mut state = self.state.borrow_mut();
            let Some((_, c)) = state.find_channel(channel) else { return };
            let Some(last) = c.last_message_id else { return };
            let pending = state.is_unread(c) || state.read.mentions(channel) > 0;
            if !pending {
                return;
            }
            state.read.ack(channel, last);
            last
        };
        self.withdraw_notification(channel);
        self.queue_ack(channel, newest);
        self.refresh_badges();
    }

    /// Send an ack after a short delay, folding rapid acks for one channel
    /// (a busy channel we are watching) into a single request.
    fn queue_ack(self: &Rc<Self>, channel: Snowflake, message: Snowflake) {
        let first = self.state.borrow_mut().pending_acks.insert(channel, message).is_none();
        if !first {
            return;
        }
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(ACK_DELAY, move || {
            let Some(this) = this.upgrade() else { return };
            let (message, rest) = {
                let mut state = this.state.borrow_mut();
                (state.pending_acks.remove(&channel), state.rest.clone())
            };
            let (Some(message), Some(rest)) = (message, rest) else { return };
            glib::spawn_future_local(async move {
                if let Err(e) = rest.ack(channel, message).await {
                    eprintln!("ack {channel}: {e}");
                }
            });
        });
    }

    /// Redraw rail and sidebar if their unread dots, badges, mutes or DM
    /// order changed. Called on every message, so it skips no-op rebuilds.
    fn refresh_badges(self: &Rc<Self>) {
        let (rail, sidebar) = {
            let state = self.state.borrow();
            (state.rail_signature(), state.sidebar_signature())
        };
        let (rail_changed, sidebar_changed) = {
            let drawn = self.drawn.borrow();
            (drawn.0 != rail, drawn.1 != sidebar)
        };
        if rail_changed {
            self.rebuild_rail();
        }
        if sidebar_changed {
            self.rebuild_sidebar();
        }
    }

    fn send_notification(&self, message: &Message, place: &Place) {
        let Some(app) = self.window.application() else { return };
        let (title, body) = {
            let state = self.state.borrow();
            let author = message.author.display_name();
            let title = match state.find_channel(message.channel_id) {
                Some((Some(guild), channel)) => {
                    format!("{author} (#{}, {})", channel.name.as_deref().unwrap_or("unnamed"), guild.name)
                }
                Some((None, dm)) if dm.kind == discord::ChannelKind::GroupDm => format!("{author} ({})", dm.title()),
                _ => author.to_owned(),
            };
            drop(state);
            let names = self.message_names(message);
            let mut body = render::plain_text(&discord::markup::parse(&message.content), &names);
            if body.is_empty() && !message.attachments.is_empty() {
                body = "Sent an attachment".into();
            }
            if body.chars().count() > 200 {
                body = body.chars().take(200).chain(std::iter::once('…')).collect();
            }
            (title, body)
        };
        let notification = gio::Notification::new(&title);
        notification.set_body(Some(&body));
        notification
            .set_default_action_and_target_value("app.open-channel", Some(&place.channel.to_string().to_variant()));
        app.send_notification(Some(&notification_id(place.channel)), &notification);
    }

    fn withdraw_notification(&self, channel: Snowflake) {
        if let Some(app) = self.window.application() {
            app.withdraw_notification(&notification_id(channel));
        }
    }

    /// Jump to any channel, DM or server (from a notification click).
    fn open_channel(self: &Rc<Self>, id: Snowflake) {
        let found = {
            let mut state = self.state.borrow_mut();
            let guild = state.find_channel(id).map(|(guild, _)| guild.map(|g| g.id));
            if let Some(guild) = guild {
                state.guild = guild;
            }
            guild.is_some()
        };
        self.window.present();
        if found {
            self.rebuild_rail();
            self.select_channel(id);
        }
    }

    /// Resolve the mentions a message can contain: the user cache (plus this
    /// message's own mentions) for users, and every channel of the open guild
    /// for channel references.
    fn message_names(&self, message: &Message) -> Names {
        let state = self.state.borrow();
        let mut users = state.users.clone();
        for user in message.mentions.iter().chain(std::iter::once(&message.author)) {
            users.entry(user.id.0).or_insert_with(|| user.display_name().to_string());
        }
        let channels = state
            .guild
            .and_then(|id| state.guilds.iter().find(|g| g.id == id))
            .map(|guild| {
                guild.channels.iter().filter_map(|c| c.name.as_ref().map(|name| (c.id.0, name.clone()))).collect()
            })
            .unwrap_or_default();
        Names { users, channels }
    }

    /// Record display names for later mention resolution.
    fn learn_users<'a>(&self, users: impl Iterator<Item = &'a User>) {
        let mut state = self.state.borrow_mut();
        for user in users {
            state.users.insert(user.id.0, user.display_name().to_string());
        }
    }

    /// For each `<@id>` in `content` we cannot yet name, fetch the user once and
    /// refresh the view when it arrives. Fixes mentions of people who left.
    fn resolve_mentions(self: &Rc<Self>, content: &str) {
        let unknown: Vec<Snowflake> = {
            let state = self.state.borrow();
            discord::markup::parse(content)
                .iter()
                .flat_map(collect_user_mentions)
                .filter(|id| !state.users.contains_key(id) && !state.fetched_users.contains(id))
                .map(Snowflake)
                .collect()
        };
        if unknown.is_empty() {
            return;
        }
        let rest = self.state.borrow().rest.clone();
        let Some(rest) = rest else { return };
        for id in unknown {
            self.state.borrow_mut().fetched_users.insert(id.0);
            let this = self.clone();
            let rest = rest.clone();
            glib::spawn_future_local(async move {
                if let Ok(user) = rest.user(id).await {
                    this.learn_users(std::iter::once(&user));
                    this.refresh_rows();
                }
            });
        }
    }

    /// Re-bind visible rows so newly resolved names appear.
    fn refresh_rows(&self) {
        let messages = self.messages();
        let n = messages.n_items();
        if n > 0 {
            messages.items_changed(0, n, n);
        }
    }

    fn scroll_to_end(&self) {
        let list = self.list.downgrade();
        let messages = self.messages();
        // After layout, so the new rows have sizes.
        glib::idle_add_local_once(move || {
            if let (Some(list), Some(last)) = (list.upgrade(), messages.n_items().checked_sub(1)) {
                list.scroll_to(last, gtk::ListScrollFlags::NONE, None);
            }
        });
    }

    /// Show or hide the mention popup based on the `@word` at the cursor.
    fn update_mention_popup(self: &Rc<Self>) {
        let Some((start, prefix)) = mention_token(&self.composer.buffer()) else {
            self.mention_popover.popdown();
            return;
        };
        let prefix = prefix.to_lowercase();
        let candidates: Vec<(u64, String)> = {
            let state = self.state.borrow();
            let mut matches: Vec<(u64, String)> = state
                .users
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&prefix))
                .map(|(id, name)| (*id, name.clone()))
                .collect();
            // Prefix matches first, then alphabetical; cap the list.
            matches.sort_by(|a, b| {
                let rank = |n: &str| if n.to_lowercase().starts_with(&prefix) { 0 } else { 1 };
                rank(&a.1).cmp(&rank(&b.1)).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            });
            matches.truncate(8);
            matches
        };
        if candidates.is_empty() {
            self.mention_popover.popdown();
            return;
        }

        while let Some(child) = self.mention_list.first_child() {
            self.mention_list.remove(&child);
        }
        for (_, name) in &candidates {
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&gtk::Label::builder().label(format!("@{name}")).xalign(0.0).build()));
            self.mention_list.append(&row);
        }
        self.mention_list.select_row(self.mention_list.row_at_index(0).as_ref());
        *self.mention.borrow_mut() = MentionCtx { start, candidates };

        // Point at the caret so the popup tracks where you are typing.
        if let Some(rect) = self.caret_rect() {
            self.mention_popover.set_pointing_to(Some(&rect));
        }
        self.mention_popover.popup();
    }

    fn move_mention_selection(&self, delta: i32) {
        let count = self.mention.borrow().candidates.len() as i32;
        if count == 0 {
            return;
        }
        let current = self.mention_list.selected_row().map(|r| r.index()).unwrap_or(0);
        let next = (current + delta).rem_euclid(count);
        self.mention_list.select_row(self.mention_list.row_at_index(next).as_ref());
    }

    fn accept_mention(self: &Rc<Self>, index: i32) {
        let (start, id) = {
            let ctx = self.mention.borrow();
            match ctx.candidates.get(index.max(0) as usize) {
                Some((id, _)) => (ctx.start, *id),
                None => return,
            }
        };
        let buffer = self.composer.buffer();
        let cursor = buffer.property::<i32>("cursor-position");
        let mut from = buffer.iter_at_offset(start);
        let mut to = buffer.iter_at_offset(cursor);
        buffer.delete(&mut from, &mut to);
        let mut at = buffer.iter_at_offset(start);
        buffer.insert(&mut at, &format!("<@{id}> "));
        self.mention_popover.popdown();
        self.composer.grab_focus();
    }

    /// The caret's rectangle in composer coordinates, for popover anchoring.
    fn caret_rect(&self) -> Option<gtk::gdk::Rectangle> {
        let buffer = self.composer.buffer();
        let iter = buffer.iter_at_offset(buffer.property::<i32>("cursor-position"));
        let strong = self.composer.cursor_locations(Some(&iter)).0;
        let (x, y) = self.composer.buffer_to_window_coords(gtk::TextWindowType::Widget, strong.x(), strong.y());
        Some(gtk::gdk::Rectangle::new(x, y, 1, strong.height()))
    }

    fn send_composer(self: &Rc<Self>) {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim().to_owned();
        let (channel, rest) = {
            let state = self.state.borrow();
            (state.channel, state.rest.clone())
        };
        let (Some(channel), Some(rest)) = (channel, rest) else { return };
        if text.is_empty() {
            return;
        }
        buffer.set_text("");
        self.mention_popover.popdown();
        let this = self.clone();
        glib::spawn_future_local(async move {
            // The gateway echoes our own message back, which is what adds it to the list.
            if let Err(e) = rest.send_message(channel, &text).await {
                this.show_composer_error(&format!("Not sent: {e}"));
                let buffer = this.composer.buffer();
                if buffer.char_count() == 0 {
                    buffer.set_text(&text);
                }
            }
        });
    }
}

// ---- helpers --------------------------------------------------------------

fn now_unix() -> i64 {
    glib::real_time() / 1_000_000
}

/// One notification per channel: a newer message replaces the older one.
fn notification_id(channel: Snowflake) -> String {
    format!("channel-{channel}")
}

/// A red mention count, or nothing for zero.
fn badge(count: u32) -> Option<gtk::Label> {
    (count > 0).then(|| {
        let text = if count > 99 { "99+".to_owned() } else { count.to_string() };
        gtk::Label::builder().label(text).valign(gtk::Align::Center).css_classes(["badge"]).build()
    })
}

/// A rail button with Discord's markers: a pill at the left edge for unread
/// messages, a mention count at the bottom right.
fn rail_item(button: &gtk::Button, unread: bool, mentions: u32) -> gtk::Widget {
    let overlay = gtk::Overlay::builder().child(button).build();
    if unread {
        let pill =
            gtk::Box::builder().halign(gtk::Align::Start).valign(gtk::Align::Center).css_classes(["rail-pill"]).build();
        pill.set_can_target(false);
        overlay.add_overlay(&pill);
    }
    if let Some(badge) = badge(mentions) {
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::End);
        badge.set_margin_end(8);
        badge.set_can_target(false);
        overlay.add_overlay(&badge);
    }
    overlay.upcast()
}

/// A collapsed folder's face: its first four servers' icons in a 2×2 grid.
fn folder_preview(guilds: &[&Guild], images: &Images) -> gtk::Widget {
    const MINI: i32 = 18;
    let grid = gtk::Grid::builder()
        .row_spacing(4)
        .column_spacing(4)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    for (i, guild) in guilds.iter().take(4).enumerate() {
        let mini: gtk::Widget = match guild.icon.as_deref() {
            Some(hash) => {
                let picture = gtk::Picture::builder()
                    .width_request(MINI)
                    .height_request(MINI)
                    .content_fit(gtk::ContentFit::Cover)
                    .overflow(gtk::Overflow::Hidden)
                    .css_classes(["mini-icon"])
                    .build();
                // Static frame only: four animations in one button is noise.
                images.set(&picture, &discord::cdn::guild_icon_still_url(guild.id.0, hash, 32), false);
                picture.upcast()
            }
            None => gtk::Label::builder()
                .label(initials(&guild.name).chars().next().map(String::from).unwrap_or_default())
                .width_request(MINI)
                .height_request(MINI)
                .css_classes(["mini-icon"])
                .build()
                .upcast(),
        };
        grid.attach(&mini, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    grid.upcast()
}

fn wrap_page(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let page = gtk::Box::builder().css_classes(["page"]).build();
    child.set_hexpand(true);
    child.set_vexpand(true);
    page.append(child);
    page.upcast()
}

/// If the cursor sits just after an `@word` token, return the `@`'s char offset
/// and the word typed so far. The `@` must start a word (start of buffer or
/// after whitespace), and the token must contain no whitespace.
fn mention_token(buffer: &gtk::TextBuffer) -> Option<(i32, String)> {
    let cursor = buffer.property::<i32>("cursor-position");
    let text: String = buffer.text(&buffer.start_iter(), &buffer.iter_at_offset(cursor), false).into();
    let chars: Vec<char> = text.chars().collect();
    let at = chars.iter().rposition(|&c| c == '@')?;
    // The word between `@` and the cursor must have no whitespace.
    if chars[at + 1..].iter().any(|c| c.is_whitespace()) {
        return None;
    }
    // `@` must begin a word.
    if at > 0 && !chars[at - 1].is_whitespace() {
        return None;
    }
    Some((at as i32, chars[at + 1..].iter().collect()))
}

/// User ids referenced by `<@id>` mentions within a parsed span (recursive, so
/// mentions inside styled/quoted spans count).
fn collect_user_mentions(span: &discord::markup::Span) -> Vec<u64> {
    use discord::markup::{Entity, Span};
    match span {
        Span::Entity(Entity::User(id)) => vec![*id],
        Span::Styled(_, children) | Span::Link { children, .. } => {
            children.iter().flat_map(collect_user_mentions).collect()
        }
        _ => Vec::new(),
    }
}

fn initials(name: &str) -> String {
    let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect();
    if letters.is_empty() { "?".into() } else { letters }
}

fn palette_index(user: &User) -> u64 {
    user.id.0 % PALETTE_SIZE
}

fn parse_time(timestamp: &str) -> Option<glib::DateTime> {
    glib::DateTime::from_iso8601(timestamp, None).ok()?.to_local().ok()
}

fn format_time(time: &glib::DateTime) -> String {
    let today = glib::DateTime::now_local().ok().map(|now| now.ymd());
    let pattern = if today == Some(time.ymd()) { "Today at %H:%M" } else { "%Y-%m-%d %H:%M" };
    time.format(pattern).map(String::from).unwrap_or_default()
}

fn is_grouped(message: &Message, previous: Option<&Message>) -> bool {
    let Some(previous) = previous else { return false };
    if previous.author.id != message.author.id {
        return false;
    }
    match (parse_time(&previous.timestamp), parse_time(&message.timestamp)) {
        (Some(a), Some(b)) => b.difference(&a).as_seconds() < GROUP_WINDOW_SECS,
        _ => false,
    }
}

/// Rows are rebuilt on every bind: simple, and cheap at chat-history sizes.
fn fill_message_row(
    row: &gtk::Box,
    message: &Message,
    previous: Option<&Message>,
    names: &Names,
    images: Option<&Images>,
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    let grouped = is_grouped(message, previous);
    row.set_css_classes(if grouped { &["message"] } else { &["message", "first"] });
    let colour = palette_index(&message.author);

    let gutter = gtk::Box::builder().width_request(40).valign(gtk::Align::Start).build();
    if !grouped {
        gutter.append(&avatar_widget(&message.author, colour, 40, images));
    }
    row.append(&gutter);

    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
    if !grouped {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.append(
            &gtk::Label::builder()
                .label(message.author.display_name())
                .css_classes(["author", &format!("author-{colour}")])
                .build(),
        );
        let time = parse_time(&message.timestamp).map(|t| format_time(&t)).unwrap_or_default();
        header.append(&gtk::Label::builder().label(time).css_classes(["time"]).build());
        body.append(&header);
    }
    let edited = message.edited_timestamp.is_some();
    if !message.content.is_empty() {
        body.append(&render::render(&discord::markup::parse(&message.content), names, edited));
    } else if edited {
        body.append(&render::render(&[], names, edited));
    }
    for attachment in &message.attachments {
        match (attachment.is_image(), images) {
            (true, Some(images)) => body.append(&image_attachment(attachment, images)),
            _ => {
                let markup = format!(
                    "<a href=\"{}\">📎 {}</a>",
                    glib::markup_escape_text(&attachment.url),
                    glib::markup_escape_text(&attachment.filename)
                );
                body.append(
                    &gtk::Label::builder()
                        .use_markup(true)
                        .label(markup)
                        .xalign(0.0)
                        .css_classes(["attachment"])
                        .build(),
                );
            }
        }
    }
    row.append(&body);
}

/// A round avatar Picture of `size` px (with the coloured initial behind it
/// while it loads), or just the initial when the user has no avatar.
fn avatar_widget(user: &User, colour: u64, size: i32, images: Option<&Images>) -> gtk::Widget {
    let initial = user.display_name().chars().next().unwrap_or('?').to_uppercase().to_string();
    let fallback = gtk::Label::builder()
        .label(initial)
        .width_request(size)
        .height_request(size)
        .css_classes(["avatar", &format!("avatar-{colour}")])
        .build();
    let (Some(hash), Some(images)) = (user.avatar.as_deref(), images) else {
        return fallback.upcast();
    };
    // Overlay the picture on the coloured initial so there is no blank gap.
    let overlay = gtk::Overlay::builder().child(&fallback).build();
    let picture = gtk::Picture::builder()
        .width_request(size)
        .height_request(size)
        .content_fit(gtk::ContentFit::Cover)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["avatar"])
        .build();
    images.set(&picture, &discord::cdn::avatar_url(user.id.0, hash, 64), discord::cdn::is_animated(hash));
    overlay.add_overlay(&picture);
    overlay.upcast()
}

/// A DM list entry: the other person's avatar (or the group's icon) and the title.
fn dm_row(channel: &Channel, images: &Images) -> gtk::Box {
    const SIZE: i32 = 32;
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let icon: gtk::Widget = match (channel.kind, channel.recipients.first(), channel.icon.as_deref()) {
        (discord::ChannelKind::GroupDm, _, Some(hash)) => {
            let picture = gtk::Picture::builder()
                .width_request(SIZE)
                .height_request(SIZE)
                .content_fit(gtk::ContentFit::Cover)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["avatar", "small"])
                .build();
            images.set(&picture, &discord::cdn::group_dm_icon_url(channel.id.0, hash, 64), false);
            picture.upcast()
        }
        (discord::ChannelKind::Dm, Some(user), _) => avatar_widget(user, palette_index(user), SIZE, Some(images)),
        _ => gtk::Label::builder()
            .label(initials(&channel.title()))
            .width_request(SIZE)
            .height_request(SIZE)
            .css_classes(["avatar", "small", &format!("avatar-{}", channel.id.0 % PALETTE_SIZE)])
            .build()
            .upcast(),
    };
    icon.set_valign(gtk::Align::Center);
    row.append(&icon);
    row.append(
        &gtk::Label::builder()
            .label(channel.title())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .build(),
    );
    row
}

/// An inline image attachment, scaled to fit a reasonable box.
fn image_attachment(attachment: &discord::Attachment, images: &Images) -> gtk::Widget {
    const MAX_W: f64 = 400.0;
    const MAX_H: f64 = 300.0;
    let (mut w, mut h) = match (attachment.width, attachment.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (MAX_W, MAX_H),
    };
    let scale = (MAX_W / w).min(MAX_H / h).min(1.0);
    w *= scale;
    h *= scale;
    let animated = attachment.content_type.as_deref() == Some("image/gif")
        || attachment.url.split('?').next().is_some_and(|u| u.ends_with(".gif"));
    let picture = gtk::Picture::builder()
        .width_request(w as i32)
        .height_request(h as i32)
        .halign(gtk::Align::Start)
        .content_fit(gtk::ContentFit::Contain)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["attachment-image"])
        .build();
    images.set(&picture, &attachment.url, animated);
    picture.upcast()
}
