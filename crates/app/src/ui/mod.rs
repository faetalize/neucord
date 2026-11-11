//! The window and everything in it, plus the glue between the UI and the
//! `discord` crate. All of this runs on the GTK main thread.

mod account;
mod attachments;
mod captcha;
mod chat;
mod components;
mod composer;
mod connection;
mod create_thread;
mod edits;
mod embeds;
mod emoji;
mod forum;
mod forum_post;
mod friends;
mod gifs;
mod login;
mod media_frame;
mod members;
mod menus;
mod message_row;
mod notify;
mod own_status;
mod panes;
mod picker;
mod poll_composer;
mod polls;
mod profile;
mod rail;
mod reactions;
mod reactors;
mod requests;
mod scheduling;
mod search;
mod settings;
mod sidebar;
mod slash;
mod state;
mod stickers;
mod switcher;
mod thread_line;
mod threads;
mod typing;
mod unread;
mod uploads;
mod video;
mod viewer;
mod voice;
mod widgets;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::{Snowflake, session};
use gtk::{gio, glib, pango};

use crate::config::{self, Store};
use crate::images::Images;
use crate::render;
use crate::timelines::Timelines;

/// The server rail's width.
const RAIL: f64 = 72.0;

pub struct App {
    window: adw::ApplicationWindow,
    /// Narrow windows: the sidebar slides over the chat, and the login page
    /// takes one column.
    narrow: adw::Breakpoint,
    /// Windows too narrow for a pane on the right of the chat.
    medium: adw::Breakpoint,
    stack: gtk::Stack,
    loading: adw::StatusPage,
    /// Covers the window while Discord requires a step.
    blocked: adw::StatusPage,
    login: login::Page,
    /// The sidebar (rail and channels) beside the content.
    split: adw::OverlaySplitView,
    /// The content, with room for a pane on its right.
    right: adw::OverlaySplitView,
    /// Home, pinned above the scrolling servers.
    home: gtk::Box,
    rail: gtk::Box,
    guild_name: widgets::Title,
    channels: gtk::ListBox,
    /// What each row of `channels` opens, by row index.
    targets: RefCell<Vec<Option<sidebar::Target>>>,
    me_label: gtk::Label,
    /// Under our name: the custom status, else the status.
    me_status: gtk::Box,
    /// Beside our name, the colour of the status.
    me_dot: gtk::Box,
    /// Opens the status menu, which `refresh_status` keeps up to date.
    status_button: RefCell<Option<gtk::MenuButton>>,
    header: widgets::Title,
    /// The open channel's chat, or one of Home's pages.
    content: gtk::Stack,
    friends: friends::FriendsPage,
    requests: requests::RequestsPage,
    /// The list's model; its inner store is the open channel's timeline.
    selection: gtk::NoSelection,
    /// Shown when no channel is open.
    blank: gio::ListStore,
    timelines: RefCell<Timelines>,
    list: gtk::ListView,
    scroller: gtk::ScrolledWindow,
    composer: gtk::TextView,
    composer_box: gtk::Box,
    /// Opens the emoji picker.
    emoji_button: gtk::Button,
    /// Opens the GIF picker.
    gif_button: gtk::Button,
    /// Opens the sticker picker, and the sticker waiting for the message.
    sticker_button: gtk::Button,
    waiting: stickers::Waiting,
    /// Who is typing in the open channel. It keeps its line when empty, so
    /// the bar's height, which pads the message list, doesn't change as people start and stop.
    typing_line: gtk::Label,
    /// Redraws the typing line when the first entry lapses.
    typing_tick: RefCell<Option<glib::SourceId>>,
    /// The message box's text is being set by us, not typed.
    composer_programmatic: Cell<bool>,
    length: composer::LengthIndicator,
    /// In place of the composer where we can't post: message requests, the
    /// system DM, a timeout. Above it while slowmode makes us wait.
    notice_bar: gtk::Box,
    /// Counts the notice bar's wait down, while it shows one.
    notice_tick: RefCell<Option<glib::SourceId>>,
    /// Fires when our timeout in the open server ends.
    timeout_end: RefCell<Option<glib::SourceId>>,
    /// Fires when a thread the sidebar lists would drop out of it.
    thread_expiry: RefCell<Option<glib::SourceId>>,
    /// Threads we started and are waiting for the gateway to bring.
    thread_waiters: RefCell<Vec<(Snowflake, async_channel::Sender<()>)>>,
    /// The post list a forum or media channel shows.
    forum: forum::Page,
    /// The form that starts a post, while it is open.
    post_form: RefCell<Option<Rc<forum_post::PostForm>>>,
    composer_error: gtk::Label,
    images: Images,
    /// Autocomplete for mentions and emoji, anchored to the composer.
    mention_popover: gtk::Popover,
    mention_list: gtk::ListBox,
    /// The token being completed (char offset of its start) and the
    /// candidates shown, row-aligned.
    mention: RefCell<composer::MentionCtx>,
    /// "Replying to …" above the composer, with the ping toggle.
    reply_bar: gtk::Box,
    reply_label: gtk::Label,
    reply_ping: gtk::ToggleButton,
    /// The message the composer replies to, if any.
    reply: RefCell<Option<composer::ReplyCtx>>,
    /// The files attached to messages being written, and their tray.
    drafts: uploads::Drafts,
    /// When the messages being written go out, if they are scheduled.
    scheduling: scheduling::Scheduling,
    /// Says how a request went, for the ones with nothing else to show it.
    toasts: adw::ToastOverlay,
    /// Shows the pictures and videos of messages over the window.
    viewer: viewer::Viewer,
    /// The voice message being recorded, or waiting to be sent.
    voice: voice::Voice,
    /// The command being filled in, and the suggestions for it.
    slash: slash::Slash,
    /// "Editing message" above the composer.
    edit_bar: gtk::Box,
    /// The message the composer is editing, if any.
    editing: RefCell<Option<edits::EditCtx>>,
    /// "You're viewing older messages", while the open channel is detached.
    present_bar: adw::Banner,
    /// How many messages are new since the channel was last read, with a
    /// way to jump to the first.
    unread_bar: adw::Banner,
    /// That Discord refused too many requests and none are being sent.
    paused_bar: adw::Banner,
    /// Hides the paused bar when the pause is over.
    paused_end: RefCell<Option<glib::SourceId>>,
    /// The divider above the first unread message, while it has a row.
    unread_divider: glib::WeakRef<gtk::Box>,
    /// A message flashed after jumping to it.
    highlight: Cell<Option<Snowflake>>,
    /// Counts changes to what every message's content is drawn from (names,
    /// colours, settings), so rows know to make theirs again.
    row_epoch: Cell<u64>,
    /// The list of who reacted, while it is open.
    reactors_view: RefCell<Option<Rc<reactors::View>>>,
    /// The thread browser, while it's open.
    threads_view: RefCell<Option<Rc<threads::View>>>,
    /// The profiles on screen: the popout and the panel beside a DM.
    profile_views: RefCell<profile::Views>,
    /// Opens the thread browser, from the chat's header.
    threads_button: gtk::Button,
    /// Opens the message search, from the chat's header.
    search_button: gtk::Button,
    /// The message search box, while it's open.
    search_view: RefCell<Option<Rc<search::View>>>,
    /// The quick switcher, while it's open.
    switcher: RefCell<Option<Rc<switcher::View>>>,
    /// The content of messages whose rows have moved on to others, for when
    /// they come back (see [`chat::KeptContent`]).
    kept_content: RefCell<crate::kept::Kept<Snowflake, chat::KeptContent>>,
    /// Every row the message list has made, to fill in again when what
    /// they show changes.
    rows: RefCell<Vec<glib::WeakRef<message_row::MessageRow>>>,
    /// When scrolling loads another page.
    scroll_loader: RefCell<discord::history::ScrollLoader>,
    state: RefCell<state::State>,
    /// Neucord's own options, which the settings window sets.
    config: Store,
    /// What Neucord remembers between runs.
    remembered: Store,
    /// The settings window, while it's open.
    settings_window: RefCell<Option<settings::Window>>,
    /// The member list pane, in the right pane of the chat.
    members: members::Pane,
    /// What the rail and sidebar last drew, so badge refreshes can skip
    /// rebuilding them when nothing visible changed.
    drawn: RefCell<(Vec<u64>, Vec<u64>)>,
    /// The rail's buttons, and the shape they were built for.
    rail_items: RefCell<Vec<rail::RailItem>>,
    rail_shape: RefCell<u64>,
    /// Per-folder colours, regenerated with the rail.
    folder_css: gtk::CssProvider,
    /// The desktop's notification server and launcher, once connected; `None`
    /// without a session bus.
    desktop: std::cell::OnceCell<Option<crate::desktop::Desktop>>,
    /// The count in front of the window's title, and the title it goes before.
    unread_count: Cell<i64>,
    content_title: RefCell<String>,
}

impl App {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        // GTK 4 doesn't speak xdg-decoration (it never asks a compositor to
        // draw the frame), so the window is always drawn client-side, and the
        // title bar is the header bar every page carries, with the window
        // controls in the order the desktop's button layout gives.
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Neucord")
            .default_width(1200)
            .default_height(800)
            // The smallest size GNOME asks apps to fit, so breakpoints can apply.
            .width_request(360)
            .height_request(294)
            .build();
        window.add_css_class("neucord");
        let max_width = |sp| {
            adw::Breakpoint::new(adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MaxWidth,
                sp,
                adw::LengthUnit::Sp,
            ))
        };
        // One breakpoint applies at a time, the last added that matches; so
        // `narrow` repeats what `medium` sets.
        let (medium, narrow) = (max_width(1000.0), max_width(720.0));
        window.add_breakpoint(medium.clone());
        window.add_breakpoint(narrow.clone());

        let blank = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(blank.clone()));
        let this = Rc::new(App {
            window,
            medium,
            narrow,
            stack: gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build(),
            loading: adw::StatusPage::builder().paintable(&adw::SpinnerPaintable::new(gtk::Widget::NONE)).build(),
            blocked: adw::StatusPage::new(),
            login: login::Page::new(),
            split: adw::OverlaySplitView::new(),
            // Pinned, or widening the window would show it, empty.
            right: adw::OverlaySplitView::builder()
                .sidebar_position(gtk::PackType::End)
                .show_sidebar(false)
                .pin_sidebar(true)
                .build(),
            home: gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(6).build(),
            rail: gtk::Box::new(gtk::Orientation::Vertical, 0),
            guild_name: widgets::Title::new(""),
            channels: gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build(),
            targets: RefCell::default(),
            me_label: gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build(),
            me_status: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            me_dot: gtk::Box::builder().valign(gtk::Align::Center).css_classes(["presence", "online"]).build(),
            status_button: RefCell::default(),
            header: widgets::Title::new(""),
            content: gtk::Stack::new(),
            friends: friends::FriendsPage::new(),
            requests: requests::RequestsPage::new(),
            list: gtk::ListView::new(Some(selection.clone()), None::<gtk::ListItemFactory>),
            selection,
            blank,
            timelines: RefCell::default(),
            scroller: gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build(),
            composer: gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .accepts_tab(false)
                .hexpand(true)
                .valign(gtk::Align::Center)
                .css_classes(["composer-input"])
                .build(),
            composer_box: gtk::Box::builder().css_classes(["composer", "card-row"]).build(),
            emoji_button: gtk::Button::builder()
                .icon_name("face-smile-symbolic")
                .tooltip_text("Emoji")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build(),
            gif_button: gtk::Button::builder()
                .label("GIF")
                .tooltip_text("Find a GIF")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build(),
            sticker_button: gtk::Button::builder()
                .icon_name("image-x-generic-symbolic")
                .tooltip_text("Stickers")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build(),
            waiting: stickers::Waiting::new(),
            typing_line: gtk::Label::builder()
                .xalign(0.0)
                .margin_start(12)
                .margin_bottom(4)
                .ellipsize(pango::EllipsizeMode::End)
                .build(),
            typing_tick: RefCell::default(),
            composer_programmatic: Cell::default(),
            length: composer::LengthIndicator::new(),
            notice_bar: gtk::Box::builder()
                .spacing(8)
                .visible(false)
                .css_classes(["composer", "card-row", "notice-bar"])
                .build(),
            notice_tick: RefCell::default(),
            timeout_end: RefCell::default(),
            thread_expiry: RefCell::default(),
            thread_waiters: RefCell::default(),
            forum: forum::Page::new(),
            post_form: RefCell::default(),
            composer_error: gtk::Label::builder()
                .xalign(0.0)
                .wrap(true)
                .visible(false)
                .css_classes(["composer-error", "error"])
                .build(),
            images: Images::new(),
            mention_popover: gtk::Popover::builder()
                .autohide(false)
                .has_arrow(false)
                .position(gtk::PositionType::Top)
                .build(),
            mention_list: gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build(),
            mention: RefCell::default(),
            reply_bar: gtk::Box::builder().spacing(8).visible(false).css_classes(["reply-bar"]).build(),
            reply_label: gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build(),
            reply_ping: gtk::ToggleButton::builder()
                .label("@ ON")
                .active(true)
                .tooltip_text("Click to disable pinging the original author.")
                .css_classes(["reply-ping", "flat"])
                .build(),
            reply: RefCell::default(),
            drafts: uploads::Drafts::new(),
            scheduling: scheduling::Scheduling::new(),
            toasts: adw::ToastOverlay::new(),
            viewer: viewer::Viewer::new(),
            voice: voice::Voice::new(),
            slash: slash::Slash::new(),
            // Styled as the reply bar, which takes its place while editing.
            edit_bar: gtk::Box::builder().spacing(8).visible(false).css_classes(["reply-bar"]).build(),
            editing: RefCell::default(),
            present_bar: adw::Banner::builder()
                .title("You're viewing older messages")
                .button_label("Jump to Present")
                .build(),
            unread_bar: adw::Banner::builder().button_label("Jump").build(),
            paused_bar: adw::Banner::new(""),
            paused_end: RefCell::default(),
            unread_divider: glib::WeakRef::new(),
            highlight: Cell::default(),
            row_epoch: Cell::default(),
            reactors_view: RefCell::default(),
            threads_view: RefCell::default(),
            profile_views: RefCell::default(),
            threads_button: gtk::Button::builder()
                .icon_name("chat-message-new-symbolic")
                .tooltip_text("Threads")
                .visible(false)
                .build(),
            search_button: gtk::Button::builder()
                .icon_name("system-search-symbolic")
                .tooltip_text("Search")
                .visible(false)
                .build(),
            search_view: RefCell::default(),
            switcher: RefCell::default(),
            kept_content: RefCell::new(crate::kept::Kept::new(chat::KEPT_CONTENT)),
            rows: RefCell::default(),
            scroll_loader: RefCell::default(),
            state: RefCell::default(),
            config: Store::open(config::config_path()),
            remembered: Store::open(config::state_path()),
            settings_window: RefCell::default(),
            members: members::Pane::new(),
            drawn: RefCell::default(),
            rail_items: RefCell::default(),
            rail_shape: RefCell::new(0),
            folder_css: gtk::CssProvider::new(),
            desktop: std::cell::OnceCell::new(),
            unread_count: Cell::new(0),
            content_title: RefCell::new("Neucord".to_owned()),
        });
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&this.window),
            &this.folder_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        *this.state.borrow_mut() = this.fresh_state();
        this.build();

        // Images from the media proxy are requested at the pixels shown.
        this.images.set_scale(this.window.scale_factor());
        this.window.connect_scale_factor_notify({
            let images = this.images.clone();
            move |window| images.set_scale(window.scale_factor())
        });

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

    fn build(self: &Rc<Self>) {
        self.stack.add_named(&self.build_loading(), Some("loading"));
        self.stack.add_named(&self.build_login(), Some("login"));
        self.stack.add_named(&self.build_main(), Some("main"));
        self.stack.add_named(&self.build_blocked(), Some("blocked"));
        // The viewer covers everything but the toasts, which say how it went.
        let layers = gtk::Overlay::builder().child(&self.stack).build();
        layers.add_overlay(self.viewer.widget());
        self.toasts.set_child(Some(&layers));
        self.window.set_content(Some(&self.toasts));

        let logout = gio::ActionEntry::builder("logout")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.logout();
                    }
                }
            })
            .build();
        let standing = gio::ActionEntry::builder("account-standing")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.show_standing();
                    }
                }
            })
            .build();
        let scheduled = gio::ActionEntry::builder("scheduled-messages")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.show_scheduled_messages();
                    }
                }
            })
            .build();
        let find = gio::ActionEntry::builder("search")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.toggle_search();
                    }
                }
            })
            .build();
        let switch = gio::ActionEntry::builder("quick-switcher")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.toggle_switcher();
                    }
                }
            })
            .build();
        let settings = gio::ActionEntry::builder("settings")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.show_settings();
                    }
                }
            })
            .build();
        self.window.add_action_entries([logout, standing, scheduled, find, switch, settings]);
        self.window.add_action_entries(self.status_actions());
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.logout", &["<Control><Shift>l"]);
            app.set_accels_for_action("win.search", &["<Control>f"]);
            // The client binds both (`mod+k`, `mod+t`).
            app.set_accels_for_action("win.quick-switcher", &["<Control>k", "<Control>t"]);
        }

        // Coming back to the open channel reads it.
        let read_open = {
            let this = Rc::downgrade(self);
            move || {
                let Some(this) = this.upgrade() else { return };
                let channel = this.state.borrow().channel;
                if let (true, Some(channel)) = (this.chat_visible(), channel) {
                    this.mark_read(channel);
                }
            }
        };
        self.window.connect_is_active_notify({
            let read_open = read_open.clone();
            move |_| read_open()
        });
        self.split.connect_show_sidebar_notify(move |_| read_open());

        // Mentions are drawn in the accent colour, which Pango markup can't
        // follow by itself.
        let accent = {
            let this = Rc::downgrade(self);
            move |style: &adw::StyleManager| {
                let foreground = style.accent_color().to_standalone_rgba(style.is_dark());
                render::set_accent(&style.accent_color_rgba(), &foreground);
                if let Some(this) = this.upgrade() {
                    this.refresh_rows();
                }
            }
        };
        let style = adw::StyleManager::default();
        accent(&style);
        style.connect_accent_color_rgba_notify(accent.clone());
        style.connect_dark_notify(accent);
    }

    fn build_loading(&self) -> gtk::Widget {
        bare_page(&self.loading)
    }

    fn build_main(self: &Rc<Self>) -> gtk::Widget {
        // The sidebar: the server rail at full height, beside the channel
        // list and its header bar. Dragging the rail's empty space moves the
        // window, as a header bar would.
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&self.home);
        column.append(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::External)
                .child(&self.rail)
                .css_classes(["under-account"])
                .build(),
        );
        let rail = gtk::WindowHandle::builder().child(&column).width_request(RAIL as i32).css_classes(["rail"]).build();
        let channels = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&self.channels)
            .css_classes(["under-account"])
            .build();
        self.channels.connect_row_activated({
            let this = Rc::downgrade(self);
            move |_, row| {
                if let Some(this) = this.upgrade() {
                    this.activate_sidebar_row(row.index());
                }
            }
        });

        let header = self.guild_name.header_bar(&[]);
        let channel_pane = adw::ToolbarView::builder().hexpand(true).content(&channels).build();
        channel_pane.add_top_bar(&header);
        let panes = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        panes.append(&rail);
        panes.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        panes.append(&channel_pane);
        let sidebar = gtk::Overlay::builder().child(&panes).build();
        sidebar.add_overlay(&self.build_account_panel());
        self.split.set_sidebar(Some(&sidebar));

        self.content.add_named(&self.build_chat(), Some("chat"));
        self.content.add_named(&self.build_friends(), Some("friends"));
        self.content.add_named(&self.build_requests(), Some("requests"));
        self.content.add_named(&self.build_forum(), Some("forum"));
        self.right.set_sidebar(Some(&self.build_members()));
        self.right.set_content(Some(&self.content));
        self.split.set_content(Some(&self.right));

        // The rail and 240 px of channels to start with.
        panes::resizable(&self.split, RAIL + 240.0, (RAIL + 180.0, RAIL + 420.0));
        panes::resizable(&self.right, 260.0, (200.0, 420.0));
        self.medium.add_setter(&self.right, "collapsed", Some(&true.to_value()));
        self.narrow.add_setter(&self.right, "collapsed", Some(&true.to_value()));
        self.narrow.add_setter(&self.split, "collapsed", Some(&true.to_value()));
        // Dragged widths could be wider than a narrow window.
        for property in ["min-sidebar-width", "max-sidebar-width"] {
            self.narrow.add_setter(&self.split, property, Some(&(RAIL + 240.0).to_value()));
        }

        let toggle = gio::ActionEntry::builder("toggle-sidebar")
            .activate({
                let split = self.split.downgrade();
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(split) = split.upgrade() {
                        split.set_show_sidebar(!split.shows_sidebar());
                    }
                }
            })
            .build();
        self.window.add_action_entries([toggle]);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.toggle-sidebar", &["F9"]);
        }
        self.split.clone().upcast()
    }

    /// Who is logged in, with the app's menu: a card floating over the
    /// bottom of the rail and channel list, which scroll on beneath it, the
    /// same size and in the same place as the composer beside it.
    fn build_account_panel(self: &Rc<Self>) -> gtk::Widget {
        let row = gtk::Box::builder().spacing(4).css_classes(["card-row"]).build();
        row.append(&self.build_status_button());
        // The app's menu too, as Discord's settings live here.
        let menu = gio::Menu::new();
        menu.append(Some("_User Settings"), Some("win.settings"));
        menu.append(Some("_Scheduled Messages"), Some("win.scheduled-messages"));
        menu.append(Some("Account _Standing"), Some("win.account-standing"));
        menu.append(Some("_About Neucord"), Some("app.about"));
        menu.append(Some("_Log Out"), Some("win.logout"));
        row.append(
            &gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .tooltip_text("Account")
                .primary(true)
                .menu_model(&menu)
                .css_classes(["flat"])
                .build(),
        );
        let panel = gtk::Box::builder().valign(gtk::Align::End).css_classes(["account-panel", "floating-card"]).build();
        panel.append(&row);

        // The lists under it end with room to scroll their last entries out
        // from under it, and their scrollbars stop above it.
        let room = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&self.window),
            &room,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        panel.connect_map(move |panel| {
            let (_, height, _, _) = panel.measure(gtk::Orientation::Vertical, -1);
            room.load_from_string(&format!(
                "scrolledwindow.under-account > viewport > *, \
                 scrolledwindow.under-account > scrollbar.vertical {{ margin-bottom: {height}px; }}"
            ));
        });
        panel.upcast()
    }

    /// The button in each content header bar that shows and hides the sidebar.
    pub(super) fn sidebar_toggle(&self) -> gtk::ToggleButton {
        let button =
            gtk::ToggleButton::builder().icon_name("sidebar-show-symbolic").tooltip_text("Toggle Sidebar").build();
        self.split.bind_property("show-sidebar", &button, "active").bidirectional().sync_create().build();
        button
    }

    /// After opening something from the sidebar: on a narrow window, put
    /// the sidebar away so it shows.
    pub(super) fn show_content(&self) {
        if self.split.is_collapsed() {
            self.split.set_show_sidebar(false);
        }
    }

    fn show_loading(&self, text: &str) {
        self.loading.set_title(text);
        self.stack.set_visible_child_name("loading");
    }

    /// Whether the open channel's messages are on screen.
    pub(super) fn chat_visible(&self) -> bool {
        self.window.is_active() && !(self.split.is_collapsed() && self.split.shows_sidebar())
    }

    /// Title the window after what the content shows.
    pub(super) fn set_content_title(&self, title: &str) {
        *self.content_title.borrow_mut() = title.to_owned();
        self.apply_title();
    }

    /// The window's title: the content's, behind the unread count.
    fn apply_title(&self) {
        let prefix = discord::badge::title_prefix(self.unread_count.get());
        self.window.set_title(Some(&format!("{prefix}{}", self.content_title.borrow())));
    }
}

/// A page with nothing but window controls above it.
fn bare_page(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let page = adw::ToolbarView::builder().content(child).build();
    page.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    page.upcast()
}
