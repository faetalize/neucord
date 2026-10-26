//! The window and everything in it, plus the glue between the UI and the
//! `discord` crate. All of this runs on the GTK main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use discord::{Event, Gateway, Guild, Message, Rest, Snowflake, User, session};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use crate::images::Images;
use crate::render::{self, Names};

/// Messages fetched when a channel opens.
const HISTORY: u8 = 50;
/// Consecutive messages by one author within this window share a header.
const GROUP_WINDOW_SECS: i64 = 7 * 60;
/// Avatar/name colour classes in style.css.
const PALETTE_SIZE: u64 = 8;

#[derive(Default)]
struct State {
    token: Option<String>,
    /// The token is already in the keyring.
    persisted: bool,
    /// READY arrived at least once for this token.
    ready_seen: bool,
    reconnect_attempts: u32,
    rest: Option<Rest>,
    gateway: Option<Gateway>,
    me: Option<User>,
    guilds: Vec<Guild>,
    guild: Option<Snowflake>,
    channel: Option<Snowflake>,
    /// Display names by user id, accumulated from every user we see plus lazy
    /// `GET /users/{id}` lookups, so mentions resolve even for people no longer
    /// in the server.
    users: HashMap<u64, String>,
    /// User ids we have already tried to fetch, to avoid repeat requests.
    fetched_users: std::collections::HashSet<u64>,
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
    messages: gio::ListStore,
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

        let messages = gio::ListStore::new::<glib::BoxedAnyObject>();
        let this = Rc::new(App {
            window,
            stack: gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build(),
            loading_label: gtk::Label::new(None),
            token_entry: gtk::PasswordEntry::builder().show_peek_icon(true).hexpand(true).build(),
            login_error: gtk::Label::builder().wrap(true).xalign(0.0).visible(false).css_classes(["error"]).build(),
            rail: gtk::Box::new(gtk::Orientation::Vertical, 0),
            guild_name: gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).css_classes(["guild-name"]).build(),
            channels: gtk::Box::new(gtk::Orientation::Vertical, 0),
            me_label: gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::End).css_classes(["me"]).build(),
            header: gtk::Label::builder().xalign(0.0).css_classes(["chat-header"]).build(),
            list: gtk::ListView::new(Some(gtk::NoSelection::new(Some(messages.clone()))), None::<gtk::ListItemFactory>),
            messages,
            scroller: gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build(),
            composer: gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .accepts_tab(false)
                .hexpand(true)
                .css_classes(["composer-input"])
                .build(),
            composer_error: gtk::Label::builder().xalign(0.0).wrap(true).visible(false).css_classes(["composer-error"]).build(),
            images: Images::new(),
            mention_popover: gtk::Popover::builder().autohide(false).has_arrow(false).position(gtk::PositionType::Top).css_classes(["mention-popover"]).build(),
            mention_list: gtk::ListBox::builder().css_classes(["mention-list"]).build(),
            mention: RefCell::default(),
            state: RefCell::default(),
        });
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
        }
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
        let chat = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .css_classes(["chat"])
            .build();
        chat.append(&self.header);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_activatable(false);
            item.set_child(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 16)));
        });
        factory.connect_bind({
            let messages = self.messages.clone();
            let this = Rc::downgrade(self);
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                let row = item.child().and_downcast::<gtk::Box>().expect("row box");
                let position = item.position();
                let message = boxed_message(&messages, position).expect("bound item exists");
                let previous = position.checked_sub(1).and_then(|p| boxed_message(&messages, p));
                let (names, images) = match this.upgrade() {
                    Some(this) => (this.message_names(&message), Some(this.images.clone())),
                    None => (Names::empty(), None),
                };
                fill_message_row(&row, &message, previous.as_ref(), &names, images.as_ref());
            }
        });
        self.list.set_factory(Some(&factory));
        self.list.add_css_class("messages");
        self.scroller.set_child(Some(&self.list));
        chat.append(&self.scroller);

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
        self.messages.remove_all();
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
        let this = self.clone();
        glib::spawn_future_local(async move {
            match Gateway::connect(&token).await {
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
        let (known_good, attempts, token) = {
            let mut state = self.state.borrow_mut();
            state.gateway = None;
            state.reconnect_attempts += 1;
            (state.persisted || state.ready_seen, state.reconnect_attempts, state.token.clone())
        };
        let Some(token) = token.filter(|_| known_good) else {
            return self.show_login(Some(why));
        };
        let delay = 2u64.saturating_pow(attempts.min(6)).min(60);
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
                Event::Ready { user, guilds } => self.on_ready(user, guilds),
                Event::GuildCreate(guild) => self.on_guild(guild),
                Event::MessageCreate(message) => self.on_message(message),
                Event::MessageUpdate { id, channel_id, content, edited } => {
                    self.on_message_update(id, channel_id, content, edited)
                }
                Event::MessageDelete { id, channel_id } => self.on_message_delete(id, channel_id),
                Event::Closed { code, reason } => {
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

    fn on_ready(self: &Rc<Self>, user: User, guilds: Vec<Guild>) {
        let store = {
            let mut state = self.state.borrow_mut();
            state.ready_seen = true;
            state.reconnect_attempts = 0;
            state.guilds = guilds;
            let store = (!state.persisted).then(|| (state.token.clone(), user.username.clone()));
            state.persisted = true;
            state.users.insert(user.id.0, user.display_name().to_string());
            state.me = Some(user);
            store
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

        // Keep the open guild across reconnects; otherwise open the first.
        let keep = {
            let state = self.state.borrow();
            state.guild.filter(|id| state.guilds.iter().any(|g| g.id == *id))
        };
        self.rebuild_rail();
        match keep.or_else(|| self.state.borrow().guilds.first().map(|g| g.id)) {
            Some(id) => self.select_guild(id),
            None => self.header.set_label("You are not in any servers yet"),
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
        if self.state.borrow().channel != Some(message.channel_id) {
            return;
        }
        let adjustment = self.scroller.vadjustment();
        let at_bottom = adjustment.value() + adjustment.page_size() >= adjustment.upper() - 80.0;
        self.resolve_mentions(&message.content);
        self.messages.append(&glib::BoxedAnyObject::new(message));
        if at_bottom {
            self.scroll_to_end();
        }
    }

    fn on_message_update(&self, id: Snowflake, channel_id: Snowflake, content: Option<String>, edited: bool) {
        // Embed-only edits carry no content; nothing to re-render yet.
        let Some(content) = content else { return };
        if self.state.borrow().channel != Some(channel_id) {
            return;
        }
        if let Some((position, mut message)) = self.find_message(id) {
            message.content = content;
            if edited && message.edited_timestamp.is_none() {
                message.edited_timestamp = Some(String::new());
            }
            self.messages.splice(position, 1, &[glib::BoxedAnyObject::new(message)]);
        }
    }

    fn on_message_delete(&self, id: Snowflake, channel_id: Snowflake) {
        if self.state.borrow().channel != Some(channel_id) {
            return;
        }
        if let Some((position, _)) = self.find_message(id) {
            self.messages.remove(position);
        }
    }

    /// Find a loaded message by id: its store position and a clone.
    fn find_message(&self, id: Snowflake) -> Option<(u32, Message)> {
        (0..self.messages.n_items()).find_map(|position| {
            let message = boxed_message(&self.messages, position)?;
            (message.id == id).then_some((position, message))
        })
    }

    // ---- navigation -------------------------------------------------------

    fn rebuild_rail(self: &Rc<Self>) {
        while let Some(child) = self.rail.first_child() {
            self.rail.remove(&child);
        }
        let state = self.state.borrow();
        for guild in &state.guilds {
            let button = gtk::Button::builder()
                .tooltip_text(&guild.name)
                .css_classes(["guild"])
                .build();
            match guild.icon.as_deref() {
                Some(hash) => {
                    let picture = gtk::Picture::builder()
                        .width_request(48)
                        .height_request(48)
                        .content_fit(gtk::ContentFit::Cover)
                        .overflow(gtk::Overflow::Hidden)
                        .build();
                    self.images.set(&picture, &discord::cdn::guild_icon_url(guild.id.0, hash, 64), discord::cdn::is_animated(hash));
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
            self.rail.append(&button);
        }
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
                self.messages.remove_all();
                self.header.set_label("No text channels you can see here");
                self.rebuild_sidebar();
            }
        }
    }

    fn rebuild_sidebar(self: &Rc<Self>) {
        while let Some(child) = self.channels.first_child() {
            self.channels.remove(&child);
        }
        let state = self.state.borrow();
        let Some(guild) = state.guild.and_then(|id| state.guilds.iter().find(|g| g.id == id)) else {
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
                let label = gtk::Label::builder()
                    .label(format!("# {}", channel.name.as_deref().unwrap_or("unnamed")))
                    .xalign(0.0)
                    .ellipsize(pango::EllipsizeMode::End)
                    .build();
                let button = gtk::Button::builder().child(&label).css_classes(["channel"]).build();
                if state.channel == Some(channel.id) {
                    button.add_css_class("active");
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
        let (name, rest) = {
            let mut state = self.state.borrow_mut();
            state.channel = Some(id);
            let name = state
                .guilds
                .iter()
                .flat_map(|g| &g.channels)
                .find(|c| c.id == id)
                .and_then(|c| c.name.clone())
                .unwrap_or_default();
            (name, state.rest.clone())
        };
        self.rebuild_sidebar();
        self.header.set_label(&format!("# {name}"));
        self.composer_error.set_visible(false);
        self.messages.remove_all();

        let Some(rest) = rest else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.messages(id, HISTORY).await;
            // The user may have switched channels while this was loading.
            if this.state.borrow().channel != Some(id) {
                return;
            }
            match result {
                Ok(history) => {
                    this.learn_users(history.iter().flat_map(|m| std::iter::once(&m.author).chain(&m.mentions)));
                    for message in &history {
                        this.resolve_mentions(&message.content);
                    }
                    let objects: Vec<glib::BoxedAnyObject> =
                        history.into_iter().map(glib::BoxedAnyObject::new).collect();
                    this.messages.splice(0, this.messages.n_items(), &objects);
                    this.scroll_to_end();
                }
                Err(e) => this.show_composer_error(&format!("Could not load messages: {e}")),
            }
        });
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
                guild
                    .channels
                    .iter()
                    .filter_map(|c| c.name.as_ref().map(|name| (c.id.0, name.clone())))
                    .collect()
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
        let n = self.messages.n_items();
        if n > 0 {
            self.messages.items_changed(0, n, n);
        }
    }

    fn scroll_to_end(&self) {
        let list = self.list.downgrade();
        let messages = self.messages.clone();
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

fn boxed_message(store: &gio::ListStore, position: u32) -> Option<Message> {
    store
        .item(position)
        .and_downcast::<glib::BoxedAnyObject>()
        .map(|object| object.borrow::<Message>().clone())
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
        gutter.append(&avatar_widget(&message.author, colour, images));
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
                body.append(&gtk::Label::builder().use_markup(true).label(markup).xalign(0.0).css_classes(["attachment"]).build());
            }
        }
    }
    row.append(&body);
}

/// A round avatar Picture (with the coloured initial behind it while it loads),
/// or just the initial when the user has no avatar.
fn avatar_widget(user: &User, colour: u64, images: Option<&Images>) -> gtk::Widget {
    let initial = user.display_name().chars().next().unwrap_or('?').to_uppercase().to_string();
    let fallback = gtk::Label::builder()
        .label(initial)
        .width_request(40)
        .height_request(40)
        .css_classes(["avatar", &format!("avatar-{colour}")])
        .build();
    let (Some(hash), Some(images)) = (user.avatar.as_deref(), images) else {
        return fallback.upcast();
    };
    // Overlay the picture on the coloured initial so there is no blank gap.
    let overlay = gtk::Overlay::builder().child(&fallback).build();
    let picture = gtk::Picture::builder()
        .width_request(40)
        .height_request(40)
        .content_fit(gtk::ContentFit::Cover)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["avatar"])
        .build();
    images.set(&picture, &discord::cdn::avatar_url(user.id.0, hash, 64), discord::cdn::is_animated(hash));
    overlay.add_overlay(&picture);
    overlay.upcast()
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
