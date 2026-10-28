//! The window and everything in it, plus the glue between the UI and the
//! `discord` crate. All of this runs on the GTK main thread.

mod account;
mod attachments;
mod captcha;
mod chat;
mod components;
mod composer;
mod connection;
mod embeds;
mod friends;
mod login;
mod media_frame;
mod menus;
mod notify;
mod panes;
mod polls;
mod rail;
mod requests;
mod sidebar;
mod state;
mod thread_line;
mod unread;
mod widgets;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::{Snowflake, session};
use gtk::{gio, glib, pango};

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
    /// In place of the composer where we can't post: message requests, the
    /// system DM, a timeout. Above it while slowmode makes us wait.
    notice_bar: gtk::Box,
    /// Counts the notice bar's wait down, while it shows one.
    notice_tick: RefCell<Option<glib::SourceId>>,
    /// Fires when a thread the sidebar lists would drop out of it.
    thread_expiry: RefCell<Option<glib::SourceId>>,
    /// What opening a forum or media channel shows, until #70 lists posts.
    forum_page: adw::StatusPage,
    forum_title: widgets::Title,
    composer_error: gtk::Label,
    images: Images,
    /// Mention autocomplete, anchored to the composer.
    mention_popover: gtk::Popover,
    mention_list: gtk::ListBox,
    /// `@`-token start (char offset) and the candidates shown, row-aligned.
    mention: RefCell<composer::MentionCtx>,
    /// "Replying to …" above the composer, with the ping toggle.
    reply_bar: gtk::Box,
    reply_label: gtk::Label,
    reply_ping: gtk::ToggleButton,
    /// The message the composer replies to, if any.
    reply: RefCell<Option<composer::ReplyCtx>>,
    /// "You're viewing older messages", while the open channel is detached.
    present_bar: adw::Banner,
    /// How many messages are new since the channel was last read, with a
    /// way to jump to the first.
    unread_bar: adw::Banner,
    /// The divider above the first unread message, while it has a row.
    unread_divider: glib::WeakRef<gtk::Box>,
    /// A message flashed after jumping to it.
    highlight: Cell<Option<Snowflake>>,
    state: RefCell<state::State>,
    /// What the rail and sidebar last drew, so badge refreshes can skip
    /// rebuilding them when nothing visible changed.
    drawn: RefCell<(Vec<u64>, Vec<u64>)>,
    /// The rail's buttons, and the shape they were built for.
    rail_items: RefCell<Vec<rail::RailItem>>,
    rail_shape: RefCell<u64>,
    /// Per-folder colours, regenerated with the rail.
    folder_css: gtk::CssProvider,
}

impl App {
    pub fn new(app: &adw::Application) -> Rc<Self> {
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
            notice_bar: gtk::Box::builder()
                .spacing(8)
                .visible(false)
                .css_classes(["composer", "card-row", "notice-bar"])
                .build(),
            notice_tick: RefCell::default(),
            thread_expiry: RefCell::default(),
            forum_page: adw::StatusPage::builder()
                .title("No Posts Shown")
                .description("Neucord can't list posts yet. Posts you've joined are under the channel in the sidebar.")
                .vexpand(true)
                .build(),
            forum_title: widgets::Title::new(""),
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
            present_bar: adw::Banner::builder()
                .title("You're viewing older messages")
                .button_label("Jump to Present")
                .build(),
            unread_bar: adw::Banner::builder().button_label("Jump").build(),
            unread_divider: glib::WeakRef::new(),
            highlight: Cell::default(),
            state: RefCell::default(),
            drawn: RefCell::default(),
            rail_items: RefCell::default(),
            rail_shape: RefCell::new(0),
            folder_css: gtk::CssProvider::new(),
        });
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&this.window),
            &this.folder_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
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
        self.window.set_content(Some(&self.stack));

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
        self.window.add_action_entries([logout, standing]);
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
            // Friend request notifications open the friends page.
            let friends = gio::ActionEntry::builder("open-friends")
                .activate({
                    let this = Rc::downgrade(self);
                    move |_: &gtk::Application, _, _| {
                        if let Some(this) = this.upgrade() {
                            this.window.present();
                            this.state.borrow_mut().friends_tab =
                                friends::Tab::List(discord::relationships::List::Pending);
                            this.show_friends();
                            this.show_content();
                        }
                    }
                })
                .build();
            app.add_action_entries([open, friends]);
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
        let forum = adw::ToolbarView::builder().content(&self.forum_page).build();
        forum.add_top_bar(&self.forum_title.header_bar(&[self.sidebar_toggle().upcast_ref()]));
        self.content.add_named(&forum, Some("forum"));
        // The right pane stays empty until the member list (#10) fills it.
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
    fn build_account_panel(&self) -> gtk::Widget {
        let row = gtk::Box::builder().spacing(4).css_classes(["card-row"]).build();
        row.append(&self.me_label);
        // The app's menu too, as Discord's settings live here.
        let menu = gio::Menu::new();
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
        self.window.set_title(Some(title));
    }
}

/// A page with nothing but window controls above it.
fn bare_page(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let page = adw::ToolbarView::builder().content(child).build();
    page.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    page.upcast()
}
