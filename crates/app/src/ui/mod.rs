//! The window and everything in it, plus the glue between the UI and the
//! `discord` crate. All of this runs on the GTK main thread.

mod chat;
mod composer;
mod connection;
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

use discord::{Snowflake, session};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use crate::images::Images;
use crate::timelines::Timelines;

pub struct App {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    loading_label: gtk::Label,
    login: login::Page,
    rail: gtk::Box,
    guild_name: gtk::Label,
    channels: gtk::Box,
    me_label: gtk::Label,
    header: gtk::Label,
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
    present_bar: gtk::Box,
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
            login: login::Page::new(),
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
            reply_bar: gtk::Box::builder().spacing(8).visible(false).css_classes(["reply-bar"]).build(),
            reply_label: gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build(),
            reply_ping: gtk::ToggleButton::builder()
                .label("@ ON")
                .active(true)
                .tooltip_text("Click to disable pinging the original author.")
                .css_classes(["reply-ping"])
                .build(),
            reply: RefCell::default(),
            present_bar: gtk::Box::builder().spacing(8).visible(false).css_classes(["present-bar"]).build(),
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
                        }
                    }
                })
                .build();
            app.add_action_entries([open, friends]);
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

        self.content.add_named(&self.build_chat(), Some("chat"));
        self.content.add_named(&self.build_friends(), Some("friends"));
        self.content.add_named(&self.build_requests(), Some("requests"));
        root.append(&self.content);
        root.upcast()
    }

    fn show_loading(&self, text: &str) {
        self.loading_label.set_label(text);
        self.stack.set_visible_child_name("loading");
    }
}

fn wrap_page(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let page = gtk::Box::builder().css_classes(["page"]).build();
    child.set_hexpand(true);
    child.set_vexpand(true);
    page.append(child);
    page.upcast()
}
