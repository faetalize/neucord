//! The window and everything in it, plus the glue between the UI and the
//! `discord` crate. All of this runs on the GTK main thread.

mod chat;
mod composer;
mod connection;
mod embeds;
mod friends;
mod login;
mod notify;
mod rail;
mod requests;
mod sidebar;
mod state;
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

pub struct App {
    window: adw::ApplicationWindow,
    /// Narrow windows: one pane at a time, and the login page in one column.
    narrow: adw::Breakpoint,
    stack: gtk::Stack,
    loading: adw::StatusPage,
    login: login::Page,
    /// The sidebar (rail and channels) beside the content.
    split: adw::NavigationSplitView,
    content_page: adw::NavigationPage,
    rail: gtk::Box,
    guild_name: adw::WindowTitle,
    channels: gtk::ListBox,
    /// What each row of `channels` opens, by row index.
    targets: RefCell<Vec<Option<sidebar::Target>>>,
    me_label: gtk::Label,
    header: adw::WindowTitle,
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
    /// Accept or ignore, in place of the composer while a message request is open.
    request_bar: gtk::Box,
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
    /// A message flashed after jumping to it.
    highlight: Cell<Option<Snowflake>>,
    state: RefCell<state::State>,
    /// What the rail and sidebar last drew, so badge refreshes can skip
    /// rebuilding them when nothing visible changed.
    drawn: RefCell<(Vec<u64>, Vec<u64>)>,
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
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            720.0,
            adw::LengthUnit::Sp,
        ));
        window.add_breakpoint(narrow.clone());

        let blank = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(blank.clone()));
        let this = Rc::new(App {
            window,
            narrow,
            stack: gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build(),
            loading: adw::StatusPage::builder().paintable(&adw::SpinnerPaintable::new(gtk::Widget::NONE)).build(),
            login: login::Page::new(),
            split: adw::NavigationSplitView::builder().min_sidebar_width(312.0).max_sidebar_width(360.0).build(),
            content_page: adw::NavigationPage::builder().title("Neucord").build(),
            rail: gtk::Box::new(gtk::Orientation::Vertical, 0),
            guild_name: adw::WindowTitle::new("", ""),
            channels: gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build(),
            targets: RefCell::default(),
            me_label: gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes(["me", "dimmed"])
                .build(),
            header: adw::WindowTitle::new("", ""),
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
                .css_classes(["composer-input"])
                .build(),
            composer_box: gtk::Box::builder().css_classes(["composer"]).build(),
            request_bar: gtk::Box::builder().spacing(8).visible(false).css_classes(["composer", "request-bar"]).build(),
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
            highlight: Cell::default(),
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
                            this.split.set_show_content(true);
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
        self.split.connect_show_content_notify(move |_| read_open());

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
        // The sidebar: server rail and channel list side by side, under the
        // open server's name.
        let rail_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&self.rail)
            .width_request(72)
            .css_classes(["rail"])
            .build();
        self.rail.set_margin_top(6);
        let channels = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&self.channels)
            .build();
        self.channels.connect_row_activated({
            let this = Rc::downgrade(self);
            move |_, row| {
                if let Some(this) = this.upgrade() {
                    this.activate_sidebar_row(row.index());
                }
            }
        });
        let panes = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        panes.append(&rail_scroller);
        panes.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        panes.append(&channels);

        let menu = gio::Menu::new();
        menu.append(Some("_Log Out"), Some("win.logout"));
        menu.append(Some("_About Neucord"), Some("app.about"));
        let header = adw::HeaderBar::builder().title_widget(&self.guild_name).build();
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Main Menu")
                .primary(true)
                .menu_model(&menu)
                .build(),
        );
        let sidebar = adw::ToolbarView::builder().content(&panes).build();
        sidebar.add_top_bar(&header);
        sidebar.add_bottom_bar(&self.me_label);
        self.split.set_sidebar(Some(&adw::NavigationPage::new(&sidebar, "Neucord")));

        self.content.add_named(&self.build_chat(), Some("chat"));
        self.content.add_named(&self.build_friends(), Some("friends"));
        self.content.add_named(&self.build_requests(), Some("requests"));
        self.content_page.set_child(Some(&self.content));
        self.split.set_content(Some(&self.content_page));
        self.narrow.add_setter(&self.split, "collapsed", Some(&true.to_value()));
        self.split.clone().upcast()
    }

    fn show_loading(&self, text: &str) {
        self.loading.set_title(text);
        self.stack.set_visible_child_name("loading");
    }

    /// Whether the open channel's messages are on screen.
    pub(super) fn chat_visible(&self) -> bool {
        self.window.is_active() && (!self.split.is_collapsed() || self.split.shows_content())
    }

    /// Name the content pane, for the window's title and screen readers.
    pub(super) fn set_content_title(&self, title: &str) {
        self.content_page.set_title(title);
    }
}

/// A page with nothing but window controls above it.
fn bare_page(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let page = adw::ToolbarView::builder().content(child).build();
    page.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    page.upcast()
}
