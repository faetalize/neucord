//! The post list that takes a forum or media channel's place: the posts
//! there are, newest first or as we sorted them, as cards in a list or a
//! grid, with the tags to filter them by.
//!
//! What a list holds, when more is asked for and what is asked is
//! `discord::forum`'s, from the official client's. Here is what shows, and
//! the requests that follow from showing it: a card that comes into view asks
//! for its post's first message, a forum that opens asks for its first page
//! of archived posts, and scrolling to the end asks for the next.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use discord::forum::{self, Archive, FirstMessages, Layout, Listing, Search, Sort, TagMatch, Unreads, View};
use discord::model::{Emoji, Tag};
use discord::permissions;
use discord::reactions::{Kind as ReactionKind, Pill};
use discord::threads::ArchivedPage;
use discord::{Channel, Message, Snowflake};
use gtk::{gio, glib};

use super::menus::{self, At, Choice, Choices, Entry, Item, Kind, Style};
use super::state::{now_ms, now_unix};
use super::threads::ThreadTarget;
use super::widgets::Title;
use super::{App, polls};
use crate::render;

/// Rows the list shows while a page is on its way.
const LOADING_ROWS: usize = 3;

/// How near the end of the list scrolling goes before the next page is asked
/// for, in pixels (`onScroll` in 745431).
const NEAR_END: f64 = 200.0;

/// How long after the rows are made scrolling is the list settling.
const SETTLING: Duration = Duration::from_millis(500);

/// What the list is made of: the client's sections, in a row each.
#[derive(Clone, Debug)]
enum Row {
    Post(Snowflake),
    /// Between the posts that are active and those that aren't.
    Older,
    /// A card's place while its page loads.
    Loading,
    /// Where the posts we can't read the history of would be.
    Denied(String),
}

/// What the list keeps beside the widgets.
#[derive(Default)]
pub(super) struct Data {
    /// How each forum we've opened is shown, until we quit.
    views: HashMap<Snowflake, View>,
    listing: Listing,
    archive: Archive,
    first: FirstMessages,
    unreads: Unreads,
    /// The servers whose active threads we have all of.
    synced: HashSet<Snowflake>,
    /// Requests for first messages are going out one after another.
    pumping: bool,
    /// The members of authors are asked for after the cards that name them.
    members_due: bool,
    /// The active posts, in the order a card looks ahead through.
    ahead: Vec<Snowflake>,
    /// When the rows were last made: the list moves by itself after, and
    /// only scrolling asks for more.
    rebuilt: Option<Instant>,
}

pub(super) struct Page {
    pub(super) root: adw::ToolbarView,
    title: Title,
    chips: adw::WrapBox,
    sort_button: gtk::MenuButton,
    new_posts: gtk::Button,
    /// Opens the form that starts a post, where we may.
    post_button: gtk::Button,
    stack: gtk::Stack,
    model: gio::ListStore,
    list: gtk::ListView,
    grid: gtk::GridView,
    actions: gio::SimpleActionGroup,
    pub(super) data: RefCell<Data>,
}

impl Page {
    pub(super) fn set_title(&self, title: &str, subtitle: Option<&str>) {
        self.title.set(title, subtitle);
    }

    pub(super) fn new() -> Self {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        Page {
            root: adw::ToolbarView::new(),
            title: Title::new(""),
            chips: adw::WrapBox::builder().child_spacing(6).line_spacing(6).hexpand(true).build(),
            sort_button: gtk::MenuButton::builder()
                .icon_name("view-sort-descending-symbolic")
                .tooltip_text("Sort and View")
                .valign(gtk::Align::Start)
                .build(),
            new_posts: gtk::Button::builder().visible(false).valign(gtk::Align::Start).build(),
            post_button: gtk::Button::builder()
                .label("New Post")
                .visible(false)
                .valign(gtk::Align::Start)
                .css_classes(["suggested-action"])
                .build(),
            stack: gtk::Stack::new(),
            list: gtk::ListView::builder().model(&selection).single_click_activate(true).build(),
            grid: gtk::GridView::builder()
                .model(&selection)
                .single_click_activate(true)
                .min_columns(1)
                .max_columns(6)
                .build(),
            model,
            actions: gio::SimpleActionGroup::new(),
            data: RefCell::default(),
        }
    }
}

/// A tag's menu opens on this.
pub(super) struct Tagged(Snowflake);

pub(super) const PIN_POST: Entry<ThreadTarget> = Entry::Item(Item {
    name: "pin-post",
    label: "_Pin Post",
    shows: |app, post| app.forum_may_pin(post.0) && !app.forum_is_pinned(post.0),
    kind: Kind::Action(|app, post| app.forum_pin(post.0, true)),
});

pub(super) const UNPIN_POST: Entry<ThreadTarget> = Entry::Item(Item {
    name: "unpin-post",
    label: "_Unpin Post",
    shows: |app, post| app.forum_may_pin(post.0) && app.forum_is_pinned(post.0),
    kind: Kind::Action(|app, post| app.forum_pin(post.0, false)),
});

/// One check item for each of the forum's tags. Past five, the client leaves
/// the ones not ticked greyed out; here they do nothing.
const POST_TAGS: Entry<ThreadTarget> = Entry::Choices(Choices {
    name: "post-tags",
    choices: |app, post| app.forum_tag_choices(post.0),
    style: Style::Check,
    choose: |app, post, tag, on| {
        if let Some(tag) = tag.parse().ok().map(Snowflake) {
            app.forum_set_tag(post.0, tag, on);
        }
    },
});

pub(super) const EDIT_TAGS: Entry<ThreadTarget> = Entry::Item(Item {
    name: "edit-tags",
    label: "Edit _Tags",
    shows: |app, post| app.forum_may_edit_tags(post.0),
    kind: Kind::Submenu(&[&[POST_TAGS]]),
});

/// Only in Developer Mode (`devmode-copy-id`, 50268).
pub(super) const COPY_TAG_ID: Entry<Tagged> = Entry::Item(Item {
    name: "copy-tag-id",
    label: "Copy Tag _ID",
    shows: |app, _| app.account.state.borrow().settings.developer_mode,
    kind: Kind::Action(|app, tag| app.copy(&tag.0.to_string())),
});

impl App {
    /// The page, to put among the others.
    pub(super) fn build_forum(self: &Rc<Self>) -> gtk::Widget {
        let page = &self.forum;
        let header = page.title.header_bar(&[self.sidebar_toggle().upcast_ref()]);
        header.pack_end(&self.members_toggle());
        page.root.add_top_bar(&header);

        for layout in [Layout::List, Layout::Grid] {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_bind({
                let this = Rc::downgrade(self);
                move |_, item| {
                    let (Some(this), Some(item)) = (this.upgrade(), item.downcast_ref::<gtk::ListItem>()) else {
                        return;
                    };
                    this.bind_forum_row(item, layout);
                }
            });
            match layout {
                Layout::List => page.list.set_factory(Some(&factory)),
                Layout::Grid => page.grid.set_factory(Some(&factory)),
            }
        }
        let activate = {
            let this = Rc::downgrade(self);
            move |position: u32| {
                let Some(this) = this.upgrade() else { return };
                let row = this.forum.model.item(position).and_downcast::<glib::BoxedAnyObject>();
                if let Some(Row::Post(post)) = row.map(|row| row.borrow::<Row>().clone()) {
                    this.select_channel(post);
                }
            }
        };
        page.list.connect_activate({
            let activate = activate.clone();
            move |_, position| activate(position)
        });
        page.grid.connect_activate(move |_, position| activate(position));

        let scrolled = |child: &gtk::Widget| {
            let scroller = gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(child)
                .build();
            scroller.vadjustment().connect_value_changed({
                let this = Rc::downgrade(self);
                move |adjustment| {
                    if let Some(this) = this.upgrade() {
                        this.forum_scrolled(adjustment);
                    }
                }
            });
            scroller
        };
        page.stack.add_named(&scrolled(page.list.upcast_ref()), Some("list"));
        page.stack.add_named(&scrolled(page.grid.upcast_ref()), Some("grid"));
        page.stack.add_named(&adw::Spinner::new(), Some("loading"));
        page.stack.add_named(
            &adw::StatusPage::builder().icon_name("view-list-symbolic").title("No Posts").vexpand(true).build(),
            Some("empty"),
        );

        page.sort_button.set_popover(Some(&gtk::PopoverMenu::from_model(None::<&gio::MenuModel>)));
        page.sort_button.insert_action_group("forum", Some(&page.actions));
        self.forum_actions();
        page.post_button.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade()
                    && let Some(id) = this.forum_shown()
                {
                    this.open_post_form(id);
                }
            }
        });
        page.new_posts.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.forum_resort();
                }
            }
        });

        let bar = gtk::Box::builder().spacing(6).margin_top(6).margin_bottom(6).margin_start(12).margin_end(12).build();
        bar.append(&page.chips);
        bar.append(&page.new_posts);
        bar.append(&page.post_button);
        bar.append(&page.sort_button);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&bar);
        column.append(&page.stack);
        page.root.set_content(Some(&column));
        page.root.clone().upcast()
    }

    /// The menu's actions: they change the open forum's view.
    fn forum_actions(self: &Rc<Self>) {
        let radio = |name: &str, change: fn(&mut View, &str)| {
            let action = gio::SimpleAction::new_stateful(name, Some(glib::VariantTy::STRING), &"".to_variant());
            let this = Rc::downgrade(self);
            action.connect_activate(move |action, value| {
                let (Some(this), Some(value)) = (this.upgrade(), value.and_then(|v| v.get::<String>())) else { return };
                action.set_state(&value.to_variant());
                this.forum_change_view(|view, _| change(view, &value));
            });
            self.forum.actions.add_action(&action);
        };
        radio("sort", |view, value| {
            view.sort = if value == "created" { Sort::CreationDate } else { Sort::LatestActivity };
        });
        radio("layout", |view, value| view.layout = if value == "grid" { Layout::Grid } else { Layout::List });
        radio("match", |view, value| view.tag_match = if value == "all" { TagMatch::All } else { TagMatch::Some });
        let reset = gio::SimpleAction::new("reset", None);
        reset.connect_activate({
            let this = Rc::downgrade(self);
            move |_, _| {
                if let Some(this) = this.upgrade() {
                    this.forum_change_view(|view, forum| view.reset(forum));
                }
            }
        });
        self.forum.actions.add_action(&reset);
    }

    /// The forum on the page, if it's what the window shows.
    pub(super) fn forum_shown(&self) -> Option<Snowflake> {
        let state = self.account.state.borrow();
        let id = state.channel.filter(|_| state.page.is_none())?;
        let (_, channel) = state.find_channel(id)?;
        (channel.kind.is_forum_like() && self.content.visible_child_name().as_deref() == Some("forum")).then_some(id)
    }

    /// A forum was opened.
    pub(super) fn open_forum(self: &Rc<Self>, id: Snowflake) {
        {
            let state = self.account.state.borrow();
            let Some((Some(guild), forum)) = state.find_channel(id) else { return };
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            let view = data.views.entry(id).or_insert_with(|| View::new(forum)).clone();
            data.listing.refresh(guild, id, &view);
            data.ahead = forum::sorted_posts(guild, id, view.sort);
        }
        self.forum_header();
        self.forum_rebuild();
        self.forum_effects();
    }

    /// What follows from the list changing: the first page of archived posts
    /// for a list that has none, and what's asked of the gateway.
    fn forum_effects(self: &Rc<Self>) {
        let Some(id) = self.forum_shown() else { return };
        let initial = {
            let data = self.forum.data.borrow();
            data.views.get(&id).is_some_and(|view| data.archive.is_initial(id, view))
        };
        if initial {
            self.forum_load_archived();
        }
        self.forum_request_unreads();
        self.forum_ack();
    }

    /// The view of the open forum changed, as `change` makes it.
    fn forum_change_view(self: &Rc<Self>, change: impl FnOnce(&mut View, &Channel)) {
        let Some(id) = self.forum_shown() else { return };
        {
            let state = self.account.state.borrow();
            let Some((Some(guild), forum)) = state.find_channel(id) else { return };
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            let Some(view) = data.views.get_mut(&id) else { return };
            let before = view.sort;
            change(view, forum);
            let view = view.clone();
            data.listing.arrange(guild, &view);
            if before != view.sort {
                data.ahead = forum::sorted_posts(guild, id, view.sort);
            }
        }
        self.forum_header();
        self.forum_rebuild();
        self.forum_effects();
    }

    /// Show the new posts, as the client's button does (`RESORT_THREADS`).
    pub(super) fn forum_resort(self: &Rc<Self>) {
        let Some(id) = self.forum_shown() else { return };
        {
            let state = self.account.state.borrow();
            let Some((Some(guild), _)) = state.find_channel(id) else { return };
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            let Some(view) = data.views.get(&id).cloned() else { return };
            data.listing.refresh(guild, id, &view);
            data.archive.resort_all(guild, Some(id));
            data.ahead = forum::sorted_posts(guild, id, view.sort);
        }
        self.forum_rebuild();
        self.forum_effects();
    }

    /// The sort and view menu, and the tags: what the forum has, and how it
    /// is shown now.
    fn forum_header(self: &Rc<Self>) {
        let page = &self.forum;
        let state = self.account.state.borrow();
        let data = page.data.borrow();
        let Some(id) = state.channel else { return };
        let (Some((_, forum)), Some(view)) = (state.find_channel(id), data.views.get(&id)) else { return };

        // The menu: sort, layout (a media channel is always a grid), and how
        // tags match where there are tags.
        let menu = gio::Menu::new();
        let radio = |action: &str, entries: &[(&str, &str)]| {
            let section = gio::Menu::new();
            for (label, value) in entries {
                let item = gio::MenuItem::new(Some(label), None);
                item.set_action_and_target_value(Some(&format!("forum.{action}")), Some(&value.to_variant()));
                section.append_item(&item);
            }
            section
        };
        menu.append_section(
            Some("Sort by"),
            &radio("sort", &[("Recently Active", "active"), ("Date Posted", "created")]),
        );
        if forum.kind != discord::ChannelKind::Media {
            menu.append_section(Some("View as"), &radio("layout", &[("List", "list"), ("Gallery", "grid")]));
        }
        if !forum.available_tags.is_empty() {
            menu.append_section(Some("Tag Matching"), &radio("match", &[("Match Some", "some"), ("Match All", "all")]));
        }
        let reset = gio::Menu::new();
        reset.append(Some("Reset to default"), Some("forum.reset"));
        menu.append_section(None, &reset);
        page.sort_button.set_menu_model(Some(&menu));
        let set = |name: &str, value: &str| {
            if let Some(action) = page.actions.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_state(&value.to_variant());
            }
        };
        set("sort", if view.sort == Sort::CreationDate { "created" } else { "active" });
        set("layout", if view.layout == Layout::Grid { "grid" } else { "list" });
        set("match", if view.tag_match == TagMatch::All { "all" } else { "some" });

        page.post_button.set_visible(state.can_post(id));

        // The tags, each a toggle that filters by it.
        while let Some(child) = page.chips.first_child() {
            page.chips.remove(&child);
        }
        for tag in &forum.available_tags {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            if let Some(emoji) = tag_emoji(tag) {
                content.append(&polls::emoji_widget(&emoji, &self.images));
            }
            content.append(&gtk::Label::new(Some(&tag.name)));
            let chip = gtk::ToggleButton::builder()
                .child(&content)
                .active(view.tags.contains(&tag.id))
                .css_classes(["pill"])
                .build();
            let (this, tag_id) = (Rc::downgrade(self), tag.id);
            chip.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.forum_change_view(|view, _| view.toggle_tag(tag_id));
                }
            });
            let (this, parent) = (Rc::downgrade(self), chip.downgrade());
            menus::on_secondary(&chip, move |x, y| {
                if let (Some(this), Some(parent)) = (this.upgrade(), parent.upgrade()) {
                    menus::popup(&this, super::menus::FORUM_TAG, Tagged(tag_id), &parent, At::Point(x, y));
                }
            });
            page.chips.append(&chip);
        }
        drop((state, data));
        self.forum_new_posts();
    }

    /// The button that offers the posts others made.
    fn forum_new_posts(&self) {
        let new = self.forum.data.borrow().listing.new_posts();
        self.forum.new_posts.set_visible(new > 0);
        self.forum.new_posts.set_label(&match new {
            1 => "1 New Post".to_owned(),
            n => format!("{n} New Posts"),
        });
    }

    /// Make the rows the list shows from what we hold.
    pub(super) fn forum_rebuild(self: &Rc<Self>) {
        let Some(id) = self.forum_shown() else { return };
        let (rows, layout, loading) = {
            let state = self.account.state.borrow();
            let data = self.forum.data.borrow();
            let (Some((Some(guild), forum)), Some(view), Some(me)) =
                (state.find_channel(id), data.views.get(&id), state.account.as_ref())
            else {
                return;
            };
            let mut rows: Vec<Row> = data.listing.shown().iter().map(|&post| Row::Post(post)).collect();
            let active = !rows.is_empty();
            let synced = data.synced.contains(&guild.id);
            let mut loading = !synced || data.archive.is_loading(id, view);
            if permissions::can_read_post_history(guild, forum, me, now_unix()) {
                let archived: Vec<Snowflake> = data
                    .archive
                    .threads(id, view)
                    .iter()
                    .copied()
                    .filter(|&post| {
                        guild.thread(post).is_some_and(|t| {
                            t.kind != discord::ChannelKind::MediaThread && guild.can_view(t, me, now_unix())
                        })
                    })
                    .collect();
                if active && (!archived.is_empty() || loading) {
                    rows.push(Row::Older);
                }
                rows.extend(archived.into_iter().map(Row::Post));
            } else if !forum.is_moderator_report() {
                rows.push(Row::Denied(format!(
                    "You do not have permission to view the post history of #{}",
                    forum.name.as_deref().unwrap_or_default()
                )));
                loading = !synced;
            } else {
                loading = !synced;
            }
            if loading {
                rows.extend((0..LOADING_ROWS).map(|_| Row::Loading));
            }
            (rows, view.layout, loading)
        };
        let page = &self.forum;
        page.data.borrow_mut().rebuilt = Some(Instant::now());
        let empty = rows.is_empty();
        let items: Vec<glib::BoxedAnyObject> = rows.into_iter().map(glib::BoxedAnyObject::new).collect();
        page.model.splice(0, page.model.n_items(), &items);
        page.stack.set_visible_child_name(match (empty, loading, layout) {
            (true, true, _) => "loading",
            (true, false, _) => "empty",
            (false, _, Layout::List) => "list",
            (false, _, Layout::Grid) => "grid",
        });
        self.forum_new_posts();
    }

    /// Draw the cards again, for what they say having changed.
    pub(super) fn forum_refresh_cards(&self) {
        let n = self.forum.model.n_items();
        self.forum.model.items_changed(0, n, n);
    }

    /// A row came into view.
    fn bind_forum_row(self: &Rc<Self>, item: &gtk::ListItem, layout: Layout) {
        let Some(row) = item.item().and_downcast::<glib::BoxedAnyObject>().map(|o| o.borrow::<Row>().clone()) else {
            return;
        };
        item.set_selectable(false);
        item.set_activatable(matches!(row, Row::Post(_)));
        let child: Option<gtk::Widget> = match row {
            Row::Post(post) => {
                self.forum_card_shown(post);
                self.post_card(post, layout)
            }
            Row::Older => Some(
                gtk::Label::builder()
                    .label("Older Posts")
                    .xalign(0.0)
                    .margin_top(12)
                    .margin_start(12)
                    .css_classes(["heading"])
                    .build()
                    .upcast(),
            ),
            Row::Loading => Some(adw::Spinner::builder().margin_top(24).margin_bottom(24).build().upcast()),
            Row::Denied(text) => Some(
                gtk::Label::builder()
                    .label(text)
                    .wrap(true)
                    .margin_top(12)
                    .margin_start(12)
                    .css_classes(["dimmed"])
                    .build()
                    .upcast(),
            ),
        };
        item.set_child(child.as_ref());
    }

    /// A card came into view: it needs its post's first message and its
    /// author's member, which the client asks for together with others.
    fn forum_card_shown(self: &Rc<Self>, post: Snowflake) {
        let (ask, author) = {
            let state = self.account.state.borrow();
            let Some((Some(guild), thread)) = state.find_channel(post) else { return };
            let Some(forum) = thread.parent_id else { return };
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            let missing = data.first.get(post).is_none_or(|slot| !slot.loaded && slot.message.is_none());
            let ask = missing && data.first.card(forum, post, &data.ahead);
            (ask, thread.owner_id.map(|owner| (guild.id, owner)))
        };
        if let Some((guild, owner)) = author {
            self.account.state.borrow_mut().members.request(guild, owner);
            let due = std::mem::replace(&mut self.forum.data.borrow_mut().members_due, true);
            if !due {
                let this = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::ZERO, move || {
                    if let Some(this) = this.upgrade() {
                        this.forum.data.borrow_mut().members_due = false;
                        this.send_member_requests();
                    }
                });
            }
        }
        if ask {
            self.forum_pump();
        }
    }

    /// Send the requests for first messages that cards made, one after
    /// another (`I` in 473503).
    fn forum_pump(self: &Rc<Self>) {
        if std::mem::replace(&mut self.forum.data.borrow_mut().pumping, true) {
            return;
        }
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::ZERO, move || {
            let Some(this) = this.upgrade() else { return };
            glib::spawn_future_local(async move {
                loop {
                    let next = this.forum.data.borrow().first.next_request();
                    let rest = this.account.state.borrow().rest.clone();
                    let (Some((forum, posts)), Some(rest)) = (next, rest) else { break };
                    let answer = rest.post_data(forum, &posts).await;
                    {
                        let mut data = this.forum.data.borrow_mut();
                        if let Ok(value) = &answer {
                            data.first.from_post_data(value);
                        }
                        data.first.finished(forum, &posts);
                    }
                    if answer.is_ok() {
                        this.forum_learn_authors();
                        this.forum_refresh_cards();
                    }
                }
                this.forum.data.borrow_mut().pumping = false;
            });
        });
    }

    /// The people who wrote the first messages are names we can show.
    fn forum_learn_authors(self: &Rc<Self>) {
        let authors: Vec<discord::User> = {
            let data = self.forum.data.borrow();
            data.first.authors()
        };
        self.learn_users(authors.iter());
    }

    /// Ask for the next page of archived posts (`loadArchivedThreads`).
    fn forum_load_archived(self: &Rc<Self>) {
        let Some(id) = self.forum_shown() else { return };
        let search = {
            let state = self.account.state.borrow();
            let (Some((Some(guild), forum)), Some(me)) = (state.find_channel(id), state.account.as_ref()) else {
                return;
            };
            if !permissions::can_read_post_history(guild, forum, me, now_unix()) {
                return;
            }
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            let Some(view) = data.views.get(&id) else { return };
            data.archive.begin(id, view)
        };
        let (Some(search), Some(rest)) = (search, self.account.state.borrow().rest.clone()) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let answer = rest.archived_threads(&search).await;
            this.forum_archived(&search, answer);
        });
        self.forum_rebuild();
    }

    /// A page of archived posts came, or didn't.
    fn forum_archived(self: &Rc<Self>, search: &Search, answer: Result<ArchivedPage, discord::Error>) {
        {
            let mut state = self.account.state.borrow_mut();
            let state = &mut *state;
            let Some(guild) = state.guilds.iter_mut().find(|g| g.channel(search.channel).is_some()) else {
                return;
            };
            let mut data = self.forum.data.borrow_mut();
            match &answer {
                Ok(page) => {
                    for thread in &page.threads {
                        guild.set_thread(thread.clone());
                    }
                    for (thread, member) in &page.members {
                        guild.set_thread_member(*thread, Some(member.clone()));
                    }
                    data.first.from_page(page);
                    data.archive.arrived(guild, search, page);
                }
                Err(e) => {
                    eprintln!("forum {}: {e}", search.channel);
                    data.archive.failed(search);
                }
            }
        }
        if answer.is_ok() {
            self.forum_learn_authors();
        }
        self.forum_rebuild();
        // Posts we joined may show in the sidebar.
        if answer.is_ok() {
            self.rebuild_sidebar();
        }
    }

    /// The list scrolled: near its end, another page of archived posts is
    /// asked for if there is one.
    fn forum_scrolled(self: &Rc<Self>, adjustment: &gtk::Adjustment) {
        let Some(id) = self.forum_shown() else { return };
        let more = {
            let data = self.forum.data.borrow();
            let settled = data.rebuilt.is_none_or(|at| at.elapsed() > SETTLING);
            settled && data.views.get(&id).is_some_and(|view| data.archive.can_load_more(id, view))
        };
        let left = adjustment.upper() - (adjustment.value() + adjustment.page_size());
        if more && left < NEAR_END {
            self.forum_load_archived();
        }
    }

    /// Ask Discord how many messages of the posts we haven't read, which
    /// shows on their cards (`REQUEST_FORUM_UNREADS`, 435470): once the
    /// server's threads are all here, for the posts we've read some of and
    /// that have more.
    fn forum_request_unreads(self: &Rc<Self>) {
        let Some(id) = self.forum_shown() else { return };
        let request = {
            let state = self.account.state.borrow();
            let Some((Some(guild), _)) = state.find_channel(id) else { return };
            let data = self.forum.data.borrow();
            if !data.synced.contains(&guild.id) {
                return;
            }
            let asking: Vec<(Snowflake, Snowflake)> = data
                .unreads
                .missing(data.listing.shown().iter().copied())
                .into_iter()
                .filter_map(|post| {
                    let thread = guild.thread(post)?;
                    let acked = state.read.acked(post)?;
                    (discord::threads::is_ackable(thread) && discord::threads::is_unread(guild, thread, &state.read))
                        .then_some((post, acked))
                })
                .collect();
            forum::UnreadRequest::new(guild.id, id, &asking)
        };
        let Some(request) = request else { return };
        self.forum.data.borrow_mut().unreads.asked(&request.posts());
        if let Some(gateway) = &self.account.state.borrow().gateway {
            gateway.request_forum_unreads(&request);
        }
    }

    /// Looking at a forum reads it (`ACK_FORUM_ACTIVE_THREADS`, 435470): the
    /// first time, and whenever a post has more than the forum was read up to.
    fn forum_ack(self: &Rc<Self>) {
        let Some(id) = self.forum_shown() else { return };
        let ack = {
            let state = self.account.state.borrow();
            let Some((Some(guild), _)) = state.find_channel(id) else { return };
            let data = self.forum.data.borrow();
            let newer = state.read.acked(id).is_some_and(|acked| {
                guild
                    .threads
                    .iter()
                    .filter(|t| t.parent_id == Some(id) && t.is_active_thread())
                    .any(|t| t.last_message_id.is_some_and(|last| last > acked))
            });
            data.listing.can_ack() || newer
        };
        if !ack {
            return;
        }
        // The client reads a forum up to now, whatever its last message is.
        let now = Snowflake::now();
        self.account.state.borrow_mut().read.ack(id, now);
        self.forum.data.borrow_mut().listing.acked(id);
        self.queue_ack(id, now);
        self.refresh_badges();
    }

    /// A thread of a server was created: a post may have been.
    pub(super) fn forum_thread_created(self: &Rc<Self>, guild: Snowflake, thread: &Channel, newly_created: bool) {
        let mine = self.account.state.borrow().me() == thread.owner_id;
        let subscribed = self.account.state.borrow().subscriptions.threads(guild);
        {
            let mut data = self.forum.data.borrow_mut();
            if newly_created && data.synced.contains(&guild) {
                data.unreads.created(thread.id);
            }
            data.first.thread_created(thread.id, subscribed);
            data.listing.created(thread, newly_created, mine);
        }
        self.forum_new_posts();
    }

    /// A thread of a server changed.
    pub(super) fn forum_thread_updated(self: &Rc<Self>, thread: &Channel) {
        let changed = {
            let state = self.account.state.borrow();
            let Some((Some(guild), now)) = state.find_channel(thread.id) else { return };
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            if now.is_pinned() {
                data.archive.pinned(now);
            }
            match data.views.get(&now.parent_id.unwrap_or_default()).cloned() {
                Some(view) if data.listing.forum() == now.parent_id => {
                    data.listing.updated(guild, now, &view);
                    data.ahead = forum::sorted_posts(guild, now.parent_id.unwrap_or_default(), view.sort);
                    true
                }
                _ => false,
            }
        };
        if changed && self.forum_shown() == thread.parent_id {
            self.forum_rebuild();
        }
    }

    /// A thread was deleted; `post` is what it was.
    pub(super) fn forum_thread_deleted(self: &Rc<Self>, post: &Channel) {
        {
            let state = self.account.state.borrow();
            let mut data = self.forum.data.borrow_mut();
            let data = &mut *data;
            data.archive.deleted(post.id);
            let guild = state.guilds.iter().find(|g| post.parent_id.is_some_and(|forum| g.channel(forum).is_some()));
            if let (Some(guild), Some(view)) = (guild, data.views.get(&post.parent_id.unwrap_or_default()).cloned()) {
                data.listing.deleted(guild, post, &view);
            }
        }
        if self.forum_shown() == post.parent_id {
            self.forum_rebuild();
        }
    }

    /// Every active thread of a server arrived, or some channels'.
    pub(super) fn forum_synced(self: &Rc<Self>, guild: Snowflake, complete: bool) {
        if complete {
            self.forum.data.borrow_mut().synced.insert(guild);
        }
        // As `THREAD_LIST_SYNC` does the list of the forum that's open.
        if let Some(id) = self.forum_shown() {
            let refresh = {
                let state = self.account.state.borrow();
                let mut data = self.forum.data.borrow_mut();
                let data = &mut *data;
                match (state.find_channel(id), data.views.get(&id).cloned()) {
                    (Some((Some(found), _)), Some(view)) if found.id == guild => {
                        data.listing.refresh(found, id, &view);
                        data.ahead = forum::sorted_posts(found, id, view.sort);
                        true
                    }
                    _ => false,
                }
            };
            if refresh {
                self.forum_rebuild();
                self.forum_effects();
            }
        }
    }

    /// A message arrived in a post, or began one.
    pub(super) fn forum_message(self: &Rc<Self>, message: &Message) {
        let parent = {
            let state = self.account.state.borrow();
            let Some((Some(_), thread)) = state.find_channel(message.channel_id) else { return };
            let mut data = self.forum.data.borrow_mut();
            data.first.created(message);
            data.unreads.message(message.channel_id);
            thread.parent_id
        };
        if parent.is_some() && self.forum_shown() == parent {
            self.forum_refresh_cards();
            self.forum_ack();
        }
    }

    /// A message of a post was edited.
    pub(super) fn forum_message_edited(self: &Rc<Self>, edit: &discord::MessageEdit) {
        let changed = {
            let mut data = self.forum.data.borrow_mut();
            let mut changed = false;
            data.first.change(edit.channel_id, edit.id, |first| {
                edit.clone().apply(first);
                changed = true;
            });
            changed
        };
        if changed && self.forum_shown().is_some() {
            self.forum_refresh_cards();
        }
    }

    /// A message of a post was deleted.
    pub(super) fn forum_message_deleted(self: &Rc<Self>, id: Snowflake, channel: Snowflake) {
        self.forum.data.borrow_mut().first.deleted(channel, id);
        if self.forum_shown().is_some() {
            self.forum_refresh_cards();
        }
    }

    /// A first message's reactions changed, as `change` makes them.
    pub(super) fn forum_change_first(
        self: &Rc<Self>,
        post: Snowflake,
        message: Snowflake,
        change: impl FnOnce(&mut Message),
    ) {
        self.forum.data.borrow_mut().first.change(post, message, change);
        if self.forum_shown().is_some() {
            self.forum_refresh_cards();
        }
    }

    /// We read a post, here or elsewhere.
    pub(super) fn forum_read(&self, post: Snowflake) {
        let forum_post = {
            let state = self.account.state.borrow();
            state
                .find_channel(post)
                .and_then(|(g, t)| g.zip(t.parent_id))
                .is_some_and(|(g, parent)| g.channel(parent).is_some_and(|p| p.kind.is_forum_like()))
        };
        if forum_post {
            self.forum.data.borrow_mut().unreads.read(post);
        }
    }

    /// A new session: everything we held may have missed events.
    pub(super) fn forum_new_session(&self) {
        let mut data = self.forum.data.borrow_mut();
        data.archive.clear();
        data.first.clear();
        data.unreads.clear();
        data.synced.clear();
    }

    /// Ask for the counts of the posts' unread messages was answered.
    pub(super) fn forum_unreads_answered(self: &Rc<Self>, threads: &[(Snowflake, Option<u32>)]) {
        self.forum.data.borrow_mut().unreads.answered(threads);
        if self.forum_shown().is_some() {
            self.forum_refresh_cards();
        }
    }

    /// What a card says, from what we hold.
    fn card_facts(&self, post: Snowflake) -> Option<Card> {
        let state = self.account.state.borrow();
        let data = self.forum.data.borrow();
        let (Some(guild), thread) = state.find_channel(post)? else { return None };
        let forum = guild.channel(thread.parent_id?)?;
        let view = data.views.get(&forum.id)?;
        let slot = data.first.get(post);
        let first = slot.and_then(|s| s.message.as_ref());

        let unread = !thread.is_archived() && discord::threads::is_unread(guild, thread, &state.read);
        let applied: Vec<&Tag> =
            thread.applied_tags.iter().filter_map(|id| forum.available_tags.iter().find(|t| t.id == *id)).collect();
        let tags = applied.iter().take(3).map(|&t| (t.clone(), view.tags.contains(&t.id))).collect();

        let blocked = first.is_some_and(|m| state.relationships.is_blocked(m.author.id));
        let preview = match (slot, first) {
            (_, Some(_)) if blocked => Some(escape("Blocked message")),
            (_, Some(message)) => {
                let text = render::plain_text(&message.markup(), &self.message_names(message));
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                (!text.is_empty())
                    .then(|| format!("<b>{}</b>: {}", escape(message.author.display_name()), escape(&text)))
            }
            (Some(slot), None) if slot.loaded => Some(escape("Original message was deleted")),
            _ => None,
        };
        let media = first.filter(|_| !blocked).and_then(|m| {
            m.attachments
                .iter()
                .find(|a| a.content_type.as_deref().is_some_and(|t| t.starts_with("image/")) && !a.is_spoiler())
                .map(|a| (a.proxy_url.clone().unwrap_or_else(|| a.url.clone()), a.width.zip(a.height)))
        });
        // The reaction most people chose (`lQ` in 435470).
        let reaction = first.and_then(|m| {
            m.reactions
                .iter()
                .max_by_key(|r| (r.count + r.burst_count, r.burst_count, std::cmp::Reverse(0)))
                .filter(|r| r.count + r.burst_count > 0)
                .map(|r| {
                    let burst = r.burst_count > 0;
                    let pill = if burst {
                        (ReactionKind::Burst, r.burst_count, r.me_burst)
                    } else {
                        (ReactionKind::Normal, r.count, r.me)
                    };
                    (m.clone(), r.emoji.clone(), pill)
                })
        });

        let messages = thread.message_count.unwrap_or(0);
        let unread_count = (unread && state.read.acked(post).is_some())
            .then(|| forum::unread_text(data.unreads.count(post), messages));
        let now = now_ms();
        let time = match view.sort {
            Sort::CreationDate => format!("Posted {}", forum::ago(now, post.unix_ms())),
            Sort::LatestActivity => forum::ago(now, forum::last_activity_ms(thread)),
        };
        Some(Card {
            title: thread.name.clone().unwrap_or_default(),
            pinned: thread.is_pinned(),
            unread,
            tags,
            more_tags: applied.len().saturating_sub(3),
            preview,
            media,
            reaction,
            reactable: state.settings.media.render_reactions && state.reacting(post).update,
            count: forum::message_count_text(messages, post),
            unread_count,
            time,
        })
    }

    /// The card of `post`, for the layout the forum is in.
    fn post_card(self: &Rc<Self>, post: Snowflake, layout: Layout) -> Option<gtk::Widget> {
        let card = self.card_facts(post)?;
        let inner = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).hexpand(true).build();
        let widget = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["card"])
            .orientation(if layout == Layout::Grid { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal })
            .build();
        inner.set_margin_top(12);
        inner.set_margin_bottom(12);
        inner.set_margin_start(12);
        inner.set_margin_end(12);

        let picture = |size: (i32, i32)| {
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .width_request(size.0)
                .height_request(size.1)
                .overflow(gtk::Overflow::Hidden)
                .valign(gtk::Align::Center)
                .build();
            if let Some((url, original)) = &card.media {
                self.images.set_sized(&picture, url, size, *original);
            }
            picture
        };
        if layout == Layout::Grid && card.media.is_some() {
            widget.append(&picture((260, 160)));
        }
        widget.append(&inner);
        if layout == Layout::List && card.media.is_some() {
            widget.append(&picture((72, 72)));
        }

        inner.append(
            &gtk::Label::builder()
                .label(&card.title)
                .xalign(0.0)
                .wrap(true)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(if card.unread { vec!["heading"] } else { vec!["heading", "dimmed"] })
                .build(),
        );

        if card.pinned || !card.tags.is_empty() {
            let tags = adw::WrapBox::builder().child_spacing(6).line_spacing(4).build();
            if card.pinned {
                let pin = gtk::Image::from_icon_name("view-pin-symbolic");
                pin.set_tooltip_text(Some("Pinned"));
                tags.append(&pin);
            }
            for (tag, filtered) in &card.tags {
                let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                if let Some(emoji) = tag_emoji(tag) {
                    chip.append(&polls::emoji_widget(&emoji, &self.images));
                }
                let class = if *filtered { "accent" } else { "dimmed" };
                chip.append(&gtk::Label::builder().label(&tag.name).css_classes(["caption", class]).build());
                tags.append(&chip);
            }
            if card.more_tags > 0 {
                tags.append(
                    &gtk::Label::builder()
                        .label(format!("+{}", card.more_tags))
                        .css_classes(["caption", "dimmed"])
                        .build(),
                );
            }
            inner.append(&tags);
        }

        if let Some(text) = &card.preview {
            inner.append(
                &gtk::Label::builder()
                    .use_markup(true)
                    .label(text)
                    .xalign(0.0)
                    .wrap(true)
                    .lines(2)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["dimmed"])
                    .build(),
            );
        }

        let footer = gtk::Box::builder().spacing(8).build();
        if let Some((message, emoji, (kind, count, me))) = &card.reaction {
            let pill = Pill { emoji, kind: *kind, count: *count, me: *me };
            footer.append(&self.pill(message, &pill, card.reactable));
        }
        let messages = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        messages.append(&gtk::Image::from_icon_name("chat-message-new-symbolic"));
        messages.append(&gtk::Label::builder().label(&card.count).css_classes(["caption"]).build());
        if let Some(unread) = &card.unread_count {
            messages.append(
                &gtk::Label::builder().label(format!("({unread} New)")).css_classes(["caption", "accent"]).build(),
            );
        }
        footer.append(&messages);
        footer.append(&gtk::Label::builder().label(&card.time).css_classes(["caption", "dimmed"]).build());
        inner.append(&footer);

        let this = Rc::downgrade(self);
        menus::on_secondary(&widget, {
            let (this, parent) = (this.clone(), widget.downgrade());
            move |x, y| {
                if let (Some(this), Some(parent)) = (this.upgrade(), parent.upgrade()) {
                    menus::popup(&this, menus::THREAD, ThreadTarget(post), &parent, At::Point(x, y));
                }
            }
        });
        menus::on_keys(&widget, {
            let parent = widget.downgrade();
            move |_| {
                let (Some(this), Some(parent)) = (this.upgrade(), parent.upgrade()) else { return false };
                menus::popup(&this, menus::THREAD, ThreadTarget(post), &parent, At::Point(12.0, 12.0));
                true
            }
        });
        Some(widget.upcast())
    }

    /// Whether Pin Post and Unpin Post show.
    pub(super) fn forum_may_pin(&self, post: Snowflake) -> bool {
        let state = self.account.state.borrow();
        match (state.find_channel(post), &state.account) {
            (Some((Some(guild), thread)), Some(me)) => permissions::can_pin_post(guild, thread, me, now_unix()),
            _ => false,
        }
    }

    pub(super) fn forum_is_pinned(&self, post: Snowflake) -> bool {
        self.account.state.borrow().find_channel(post).is_some_and(|(_, t)| t.is_pinned())
    }

    /// Pin `post`, or unpin it (`pin` and `unpin` in 7909). Pinning one
    /// replaces the post that's pinned, after asking.
    pub(super) fn forum_pin(self: &Rc<Self>, post: Snowflake, pin: bool) {
        let other = {
            let state = self.account.state.borrow();
            let Some((Some(guild), thread)) = state.find_channel(post) else { return };
            guild
                .threads
                .iter()
                .find(|t| t.parent_id == thread.parent_id && t.is_active_thread() && t.is_pinned() && t.id != post)
                .map(|t| t.id)
        };
        match other.filter(|_| pin) {
            None => self.forum_set_pinned(post, pin),
            Some(other) => {
                let dialog = adw::AlertDialog::new(
                    Some("Pin this post?"),
                    Some(
                        "There\u{2019}s a post that\u{2019}s pinned already. Pinning this post will remove the existing pinned post.",
                    ),
                );
                dialog.add_responses(&[("cancel", "_Cancel"), ("pin", "_Pin")]);
                dialog.set_default_response(Some("pin"));
                dialog.set_close_response("cancel");
                let this = self.clone();
                glib::spawn_future_local(async move {
                    if dialog.choose_future(Some(&this.window)).await == "pin" {
                        this.forum_set_pinned(other, false);
                        this.forum_set_pinned(post, true);
                    }
                });
            }
        }
    }

    /// Show the post pinned or not at once, and ask Discord; if it
    /// refuses, put it back (`updateFlags`).
    fn forum_set_pinned(self: &Rc<Self>, post: Snowflake, pin: bool) {
        const PINNED: u64 = 1 << 1;
        let (before, rest) = {
            let state = self.account.state.borrow();
            let Some((_, thread)) = state.find_channel(post) else { return };
            (thread.clone(), state.rest.clone())
        };
        let Some(rest) = rest else { return };
        let flags = if pin { before.flags | PINNED } else { before.flags & !PINNED };
        let mut after = before.clone();
        after.flags = flags;
        let guild = self.account.state.borrow().find_channel(post).and_then(|(g, _)| g.map(|g| g.id));
        let Some(guild) = guild else { return };
        self.on_threads(guild, |g| g.set_thread(after.clone()));
        self.forum_thread_updated(&after);
        let reopen = pin && before.is_archived();
        let this = self.clone();
        glib::spawn_future_local(async move {
            if rest.set_thread_flags(post, flags, reopen).await.is_err() {
                this.on_threads(guild, |g| g.set_thread(before.clone()));
                this.forum_thread_updated(&before);
            }
        });
    }

    /// Whether Edit Tags shows.
    pub(super) fn forum_may_edit_tags(&self, post: Snowflake) -> bool {
        let state = self.account.state.borrow();
        match (state.find_channel(post), &state.account) {
            (Some((Some(guild), thread)), Some(me)) => permissions::can_edit_post_tags(guild, thread, me, now_unix()),
            _ => false,
        }
    }

    /// The forum's tags, ticked where the post has them.
    pub(super) fn forum_tag_choices(&self, post: Snowflake) -> Vec<Choice> {
        let state = self.account.state.borrow();
        let Some((Some(guild), thread)) = state.find_channel(post) else { return Vec::new() };
        let Some(forum) = thread.parent_id.and_then(|id| guild.channel(id)) else { return Vec::new() };
        forum
            .available_tags
            .iter()
            .map(|tag| Choice {
                value: tag.id.to_string(),
                label: tag.name.clone(),
                ticked: thread.applied_tags.contains(&tag.id),
            })
            .collect()
    }

    /// Add `tag` to the post, or take it off (`updateForumPostTags`): the
    /// post is reopened first if it has to be, and nothing shows until the
    /// gateway says the post changed.
    pub(super) fn forum_set_tag(self: &Rc<Self>, post: Snowflake, tag: Snowflake, on: bool) {
        let (mut tags, rest) = {
            let state = self.account.state.borrow();
            let Some((_, thread)) = state.find_channel(post) else { return };
            (thread.applied_tags.clone(), state.rest.clone())
        };
        let Some(rest) = rest else { return };
        match (on, tags.iter().position(|&t| t == tag)) {
            (true, None) if tags.len() < forum::MAX_TAGS => tags.push(tag),
            (false, Some(at)) => {
                tags.remove(at);
            }
            _ => return,
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let done = match this.reopen(&rest, post).await {
                Ok(()) => rest.set_post_tags(post, &tags).await,
                Err(e) => Err(e),
            };
            if let Err(e) = done {
                eprintln!("tags of {post}: {e}");
                this.toasts.add_toast(adw::Toast::new("Couldn't change the tags"));
            }
        });
    }
}

/// What a card says.
struct Card {
    title: String,
    pinned: bool,
    unread: bool,
    /// The first three of its tags, and whether the filter has each.
    tags: Vec<(Tag, bool)>,
    more_tags: usize,
    /// In markup.
    preview: Option<String>,
    media: Option<(String, Option<(u32, u32)>)>,
    reaction: Option<(Message, Emoji, (ReactionKind, u32, bool))>,
    reactable: bool,
    count: String,
    unread_count: Option<String>,
    time: String,
}

/// A tag's emoji, if it has one.
pub(super) fn tag_emoji(tag: &Tag) -> Option<Emoji> {
    (tag.emoji_id.is_some() || tag.emoji_name.is_some()).then(|| Emoji {
        id: tag.emoji_id,
        name: tag.emoji_name.clone(),
        animated: false,
    })
}

fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}
