//! Finding threads: the browser a channel's header opens, the box under a
//! message that started one, opening a thread from either or from a "started
//! a thread" line, and joining and leaving. When each request goes is in
//! [`discord::threads`] and [`discord::rest`].

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use discord::permissions::{self, Close, Delete, Lock};
use discord::threads::{self, Archive, MENU_LOCATION};
use discord::{Channel, ChannelKind, Message, Snowflake, system};
use gtk::glib;

use super::App;
use super::friends::confirm;
use super::menus::{self, Entry, Item, Kind};
use super::state::{State, now_ms, now_unix};

/// How close to the end of the list scrolling asks for the next page (the
/// client asks within 200 pixels).
const NEAR_END: f64 = 200.0;

/// What a thread's menu opens on.
pub(super) struct ThreadTarget(pub(super) Snowflake);

/// Join Thread, on a thread we're not in. The client (949488) offers it in
/// the thread's menu, which its sidebar rows, browser rows and message
/// boxes share, and in toolbar menus Neucord has no use for yet.
pub(super) const JOIN_THREAD: Entry<ThreadTarget> = Entry::Item(Item {
    name: "join-thread",
    label: "_Join Thread",
    shows: |app, target| app.account.state.borrow().thread_joined(target.0) == Some(false),
    kind: Kind::Action(|app, target| app.join_thread(target.0)),
});

/// Leave Thread, on one we're in.
pub(super) const LEAVE_THREAD: Entry<ThreadTarget> = Entry::Item(Item {
    name: "leave-thread",
    label: "_Leave Thread",
    shows: |app, target| app.account.state.borrow().thread_joined(target.0) == Some(true),
    kind: Kind::Action(|app, target| app.leave_thread(target.0)),
});

/// Whether the person in `target` may be removed from the open thread by us
/// (`qi` in 406704): the thread's owner for a private one, or anyone who
/// may manage threads there. Not ourselves, nor from a media thread.
fn may_remove(app: &App, target: &super::profile::UserTarget) -> Option<(Snowflake, bool)> {
    let thread = target.channel?;
    let state = app.account.state.borrow();
    let (Some(guild), channel) = state.find_channel(thread)? else { return None };
    let me = state.account.as_ref()?;
    let managers =
        permissions::in_channel(guild, channel, me, now_unix()).contains(permissions::Permissions::MANAGE_THREADS);
    let allowed = (channel.kind == ChannelKind::PrivateThread && channel.owner_id == Some(me.id)) || managers;
    let possible = channel.kind.is_thread() && channel.kind != ChannelKind::MediaThread && target.user.id != me.id;
    let forum = channel.parent_id.and_then(|p| guild.channel(p)).is_some_and(|p| p.kind.is_forum_like());
    (allowed && possible).then_some((thread, forum))
}

/// Remove from Thread, in the menu of a person in a thread's member list (the
/// thread's user menu, 202643). The client's label names the person.
pub(super) const REMOVE_FROM_THREAD: Entry<super::profile::UserTarget> = Entry::Item(Item {
    name: "remove-from-thread",
    label: "Remove from _Thread",
    shows: |app, target| may_remove(app, target).is_some_and(|(_, forum)| !forum),
    kind: Kind::Action(|app, target| app.remove_from_thread(target)),
});

/// ... and from a forum post, where it says Post.
pub(super) const REMOVE_FROM_POST: Entry<super::profile::UserTarget> = Entry::Item(Item {
    name: "remove-from-post",
    label: "Remove from _Post",
    shows: |app, target| may_remove(app, target).is_some_and(|(_, forum)| forum),
    kind: Kind::Action(|app, target| app.remove_from_thread(target)),
});

/// Copy Link on a thread or a forum post (886393).
pub(super) const COPY_THREAD_LINK: Entry<ThreadTarget> = Entry::Item(Item {
    name: "copy-thread-link",
    label: "Copy _Link",
    shows: |_, _| true,
    kind: Kind::Action(|app, target| {
        let link = app.account.state.borrow().find_channel(target.0).map(|(guild, c)| c.link(guild.map(|g| g.id)));
        if let Some(link) = link {
            app.copy(&link);
        }
    }),
});

/// Copy Thread ID, only in Developer Mode.
pub(super) const COPY_THREAD_ID: Entry<ThreadTarget> = Entry::Item(Item {
    name: "copy-thread-id",
    label: "Copy Thread _ID",
    shows: |app, _| app.account.state.borrow().settings.developer_mode,
    kind: Kind::Action(|app, target| app.copy(&target.0.to_string())),
});

/// Close or Open on a thread, and Lock or Unlock: a thread's and a forum
/// post's differ in their words (182061, 707792).
macro_rules! manage_item {
    ($name:ident, $id:literal, $label:literal, $rule:ident, $is:expr, $post:literal, $action:expr) => {
        pub(super) const $name: Entry<ThreadTarget> = Entry::Item(Item {
            name: $id,
            label: $label,
            shows: |app, target| {
                app.thread_rule(target.0, |g, t, me| permissions::$rule(g, t, me, now_unix())).is_some_and($is)
                    && app.account.state.borrow().is_forum_post(target.0) == $post
            },
            kind: Kind::Action(|app, target| $action(app, target.0)),
        });
    };
}
manage_item!(
    CLOSE_THREAD,
    "close-thread",
    "Close _Thread",
    thread_close,
    |c| c == Close::Close,
    false,
    App::close_thread
);
manage_item!(CLOSE_POST, "close-post", "Close _Post", thread_close, |c| c == Close::Close, true, App::close_thread);
manage_item!(OPEN_THREAD, "open-thread", "Open _Thread", thread_close, |c| c == Close::Open, false, App::reopen_thread);
manage_item!(OPEN_POST, "open-post", "Open _Post", thread_close, |c| c == Close::Open, true, App::reopen_thread);
manage_item!(LOCK_THREAD, "lock-thread", "_Lock Thread", thread_lock, |l| l == Lock::Lock, false, App::lock_thread);
manage_item!(LOCK_POST, "lock-post", "_Lock Post", thread_lock, |l| l == Lock::Lock, true, App::lock_thread);
manage_item!(
    UNLOCK_THREAD,
    "unlock-thread",
    "Un_lock Thread",
    thread_lock,
    |l| l == Lock::Unlock,
    false,
    App::unlock_thread
);
manage_item!(UNLOCK_POST, "unlock-post", "Un_lock post", thread_lock, |l| l == Lock::Unlock, true, App::unlock_thread);
manage_item!(
    DELETE_THREAD,
    "delete-thread",
    "_Delete Thread",
    thread_delete,
    |d| d == Delete::Thread,
    false,
    App::confirm_delete_thread
);
manage_item!(
    DELETE_POST,
    "delete-post",
    "_Delete Post",
    thread_delete,
    |d| d == Delete::Post,
    true,
    App::confirm_delete_thread
);
manage_item!(
    DELETE_MESSAGE,
    "delete-post-message",
    "_Delete Message",
    thread_delete,
    |d| d == Delete::Message,
    true,
    App::confirm_delete_thread
);

/// The browser, open over the window.
pub(super) struct View {
    dialog: adw::Dialog,
    /// The channel whose threads it lists.
    channel: Snowflake,
    list: gtk::ListBox,
    /// The thread each row of `list` opens, `None` for a heading.
    pub(super) rows: RefCell<Vec<Option<Snowflake>>>,
    /// The name search, which asks when Enter is pressed.
    pub(super) search: gtk::SearchEntry,
    /// What the last search found, shown in place of the browser's lists.
    found: RefCell<Option<Vec<Snowflake>>>,
    /// A search is on its way.
    searching: std::cell::Cell<bool>,
}

/// A row of the list as it is drawn.
enum Line {
    Heading(String),
    Thread {
        id: Snowflake,
        name: String,
        detail: String,
    },
    /// Nothing to list.
    Empty,
    /// More is on its way.
    Loading,
}

impl View {
    fn new(app: &Rc<App>, channel: Snowflake) -> Rc<View> {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        let scroller =
            gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let toolbar = adw::ToolbarView::new();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search for Thread Name")
            .tooltip_text("Search for Thread Name")
            .hexpand(true)
            .build();
        let header = adw::HeaderBar::new();
        // Searching needs the channel's history, as the client's browser does.
        if app.account.state.borrow().can_read_history(channel) {
            header.set_title_widget(Some(&search));
        }
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));
        let dialog =
            adw::Dialog::builder().title("Threads").content_width(480).content_height(560).child(&toolbar).build();
        let view = Rc::new(View {
            dialog,
            channel,
            list,
            rows: RefCell::default(),
            search,
            found: RefCell::default(),
            searching: std::cell::Cell::new(false),
        });
        // Enter asks, with no delay while typing; emptying the box goes back
        // to the lists (`submit` and `setQuery` in 628707).
        view.search.connect_activate({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_| {
                if let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) {
                    view.submit(&app);
                }
            }
        });
        view.search.connect_search_changed({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |entry| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                if entry.text().is_empty() && view.found.take().is_some() {
                    view.refresh(&app);
                }
            }
        });

        view.list.connect_row_activated({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_, row| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                let thread =
                    usize::try_from(row.index()).ok().and_then(|i| view.rows.borrow().get(i).copied().flatten());
                if let Some(thread) = thread {
                    view.dialog.force_close();
                    app.open_thread(thread);
                }
            }
        });
        scroller.vadjustment().connect_value_changed({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |adjustment| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                if adjustment.value() + adjustment.page_size() >= adjustment.upper() - NEAR_END {
                    view.more(&app);
                }
            }
        });
        view
    }

    /// Ask for the threads the box names, if it names any and no search is
    /// out (`submit` in 628707).
    fn submit(self: &Rc<Self>, app: &Rc<App>) {
        let text = self.search.text().to_string();
        if text.is_empty() || self.searching.get() {
            return;
        }
        let Some(rest) = app.account.state.borrow().rest.clone() else { return };
        self.searching.set(true);
        self.refresh(app);
        let (app, view) = (app.clone(), self.clone());
        glib::spawn_future_local(async move {
            let answer = rest.search_thread_names(view.channel, &text, &[], discord::forum::TagMatch::Some).await;
            view.searching.set(false);
            match answer {
                Ok(page) => {
                    let ids: Vec<Snowflake> = page.threads.iter().map(|t| t.id).collect();
                    let guild =
                        app.account.state.borrow().find_channel(view.channel).and_then(|(g, _)| g.map(|g| g.id));
                    if let Some(guild) = guild {
                        app.on_threads(guild, |g| {
                            for thread in page.threads {
                                g.set_thread(thread);
                            }
                            for (thread, member) in page.members {
                                g.set_thread_member(thread, Some(member));
                            }
                        });
                    }
                    *view.found.borrow_mut() = Some(ids);
                }
                Err(e) => eprintln!("threads: the search failed: {e}"),
            }
            view.refresh(&app);
        });
    }

    /// What the list shows now: the threads we're in, the other active
    /// ones, and the archived ones asked for so far, or what a search found.
    fn lines(&self, app: &App) -> Vec<Line> {
        let state = app.account.state.borrow();
        if let Some(found) = &*self.found.borrow() {
            let lines: Vec<Line> = found
                .iter()
                .filter_map(|&id| state.find_channel(id).map(|(_, t)| t))
                .filter(|t| state.can_view(t.id))
                .map(|t| Line::Thread {
                    id: t.id,
                    name: t.name.clone().unwrap_or_else(|| "unnamed".into()),
                    detail: detail(t, &state.users, &state.relationships),
                })
                .collect();
            return if lines.is_empty() { vec![Line::Empty] } else { lines };
        }
        let Some((Some(guild), _)) = state.find_channel(self.channel) else { return vec![Line::Empty] };
        let fallback = Archive::default();
        let archive = state.archives.get(&self.channel).unwrap_or(&fallback);
        let lists = threads::browser(guild, self.channel, archive, |t| state.can_view(t.id));
        let mut lines = Vec::new();
        let mut section = |heading: String, threads: &[&Channel]| {
            if threads.is_empty() {
                return;
            }
            lines.push(Line::Heading(heading));
            lines.extend(threads.iter().map(|t| Line::Thread {
                id: t.id,
                name: t.name.clone().unwrap_or_else(|| "unnamed".into()),
                detail: detail(t, &state.users, &state.relationships),
            }));
        };
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let (joined, others) = (lists.joined.len(), lists.others.len());
        section(format!("{joined} joined thread{}", plural(joined)), &lists.joined);
        section(format!("{others} other active thread{}", plural(others)), &lists.others);
        section("Older threads".into(), &lists.archived);
        let loading = archive.is_loading() || self.searching.get();
        if lists.is_empty() && !loading {
            lines.push(Line::Empty);
        }
        if loading {
            lines.push(Line::Loading);
        }
        lines
    }

    /// Draw the list again, as the threads are now.
    fn refresh(self: &Rc<Self>, app: &Rc<App>) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let mut rows = Vec::new();
        for line in self.lines(app) {
            let (child, thread): (gtk::Widget, _) = match line {
                Line::Heading(text) => (
                    gtk::Label::builder()
                        .label(text)
                        .xalign(0.0)
                        .css_classes(["heading"])
                        .margin_top(12)
                        .margin_start(12)
                        .margin_end(12)
                        .build()
                        .upcast(),
                    None,
                ),
                Line::Thread { id, name, detail } => (thread_line(app, id, &name, &detail), Some(id)),
                Line::Empty => (
                    gtk::Label::builder()
                        .label("There are no threads.")
                        .css_classes(["dimmed"])
                        .margin_top(24)
                        .margin_bottom(24)
                        .build()
                        .upcast(),
                    None,
                ),
                Line::Loading => {
                    (gtk::Spinner::builder().spinning(true).margin_top(12).margin_bottom(12).build().upcast(), None)
                }
            };
            let row = gtk::ListBoxRow::builder().child(&child).activatable(thread.is_some()).selectable(false).build();
            self.list.append(&row);
            rows.push(thread);
        }
        *self.rows.borrow_mut() = rows;
    }

    /// The first page of archived threads, if the browser hasn't asked yet.
    fn first(self: &Rc<Self>, app: &Rc<App>) {
        self.ask(app, Archive::first);
    }

    /// The next page, if the last has landed and there is one.
    fn more(self: &Rc<Self>, app: &Rc<App>) {
        self.ask(app, Archive::more);
    }

    /// Ask for the page `next` says, if we may read the channel's history,
    /// as the client does first (`loadMore` in 424798).
    fn ask(self: &Rc<Self>, app: &Rc<App>, next: fn(&mut Archive) -> Option<u32>) {
        let offset = {
            let mut state = app.account.state.borrow_mut();
            if !state.can_read_history(self.channel) {
                return;
            }
            next(state.archives.entry(self.channel).or_default())
        };
        if let Some(offset) = offset {
            app.load_archived_threads(self.channel, offset);
            // It shows that it's loading.
            self.refresh(app);
        }
    }
}

/// A thread's row in the browser: its name, and when it was active.
fn thread_line(app: &Rc<App>, thread: Snowflake, name: &str, detail: &str) -> gtk::Widget {
    let line = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    line.append(
        &gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build(),
    );
    line.append(
        &gtk::Label::builder()
            .label(detail)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption", "dimmed"])
            .build(),
    );
    app.thread_menu_on(&line, thread);
    line.upcast()
}

/// "Started by A · Last active 3 hours ago" for an archived thread, whose
/// starter the client names; for an active one, only when it was last active.
/// A thread row's second line (`ThreadBrowserRowSubtext`, 28579): for an
/// active thread its newest message with who wrote it, or that there are none,
/// and when; for an archived one who started it and when it was last active.
fn detail(
    thread: &Channel,
    users: &std::collections::HashMap<u64, String>,
    relationships: &discord::relationships::Relationships,
) -> String {
    let last = thread
        .last_message_id
        .map(|id| id.unix_ms())
        .or_else(|| thread.thread_metadata.as_ref().and_then(|m| m.archive_timestamp))
        .unwrap_or_else(|| thread.id.unix_ms());
    let ago = system::humanize(((now_ms() - last) / 1000).max(0));
    if thread.is_active_thread() {
        let written = |m: &discord::Message| {
            let (blocked, ignored) = (m.is_blocked_by(|id| relationships.is_blocked(id)), false);
            let text = m.thread_preview(blocked, ignored);
            let text = discord::markup::parse(&text);
            let plain = crate::render::plain_text(&text, &crate::render::Names::default());
            let name = m.author.display_name();
            format!("{name}: {}", plain.lines().next().unwrap_or_default())
        };
        return match &thread.recent_message {
            Some(message) => {
                let when = message.id.unix_ms();
                let ago = system::humanize(((now_ms() - when) / 1000).max(0));
                format!("{} · {ago} ago", written(message))
            }
            None => format!("No recent messages · {ago} ago"),
        };
    }
    let active = format!("Last active {ago} ago");
    match thread.owner_id.and_then(|owner| users.get(&owner.0)).filter(|_| thread.is_archived()) {
        Some(owner) => format!("Started by {owner} · {active}"),
        None => active,
    }
}

impl State {
    /// The threads header button: shown for a channel that takes threads and
    /// for a thread, whose parent's threads it lists (`renderHeaderToolbar` in
    /// 420018), and whether it can be pressed, which it can't while the channel
    /// it lists is behind the age gate (494208, by `ni` in 323073). `None` is
    /// no button.
    pub(super) fn threads_button(&self, agreed: &[Snowflake]) -> Option<bool> {
        let id = self.channel.filter(|_| self.page.is_none())?;
        let (guild, channel) = self.find_channel(id)?;
        let guild = guild?;
        let listed = if channel.kind.is_thread() {
            channel.parent_id.and_then(|p| guild.channel(p)).filter(|p| !p.kind.is_forum_like())
        } else {
            Some(channel).filter(|c| matches!(c.kind, ChannelKind::Text | ChannelKind::Announcement))
        };
        listed.map(|c| self.age_gate(c.id, agreed).is_none())
    }
}

impl App {
    /// The threads of `channel`, or of a thread's parent: open the browser,
    /// and ask for archived threads if it never has.
    pub(super) fn open_threads(self: &Rc<Self>, channel: Snowflake) {
        let parent = {
            let state = self.account.state.borrow();
            match state.find_channel(channel) {
                Some((_, thread)) if thread.kind.is_thread() => thread.parent_id,
                Some((_, channel)) => Some(channel.id),
                None => None,
            }
        };
        let Some(parent) = parent else { return };
        if let Some(old) = self.threads_view.take() {
            old.dialog.force_close();
        }
        let view = View::new(self, parent);
        *self.threads_view.borrow_mut() = Some(view.clone());
        view.dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |closed| {
                let Some(this) = this.upgrade() else { return };
                let ours = this.threads_view.borrow().as_ref().is_some_and(|v| v.dialog == *closed);
                if ours {
                    this.threads_view.take();
                }
            }
        });
        view.dialog.present(Some(&self.window));
        view.refresh(self);
        view.first(self);
    }

    /// Fill the open channel's rows in again, for the boxes under messages
    /// that started a thread: what we know of those threads changed.
    pub(super) fn refresh_thread_boxes(self: &Rc<Self>) {
        let messages = self.messages();
        self.rebind(&messages, 0, messages.n_items());
    }

    /// The browser follows the threads it lists.
    pub(super) fn refresh_threads_view(self: &Rc<Self>) {
        let view = self.threads_view.borrow().clone();
        if let Some(view) = view {
            view.refresh(self);
        }
    }

    /// The page of archived threads that starts at `offset`. It lands in the
    /// server's threads, as the client's does in its channel store.
    fn load_archived_threads(self: &Rc<Self>, channel: Snowflake, offset: u32) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let page = rest.archived_threads(&discord::forum::Search::plain(channel, offset)).await;
            let guild = this.account.state.borrow().find_channel(channel).and_then(|(g, _)| g.map(|g| g.id));
            match (page, guild) {
                (Ok(page), Some(guild)) => {
                    let oldest = page.threads.last().map(|t| t.id);
                    this.account.state.borrow_mut().archives.entry(channel).or_default().landed(
                        offset,
                        page.has_more,
                        oldest,
                    );
                    this.on_threads(guild, |g| {
                        for thread in page.threads {
                            g.set_thread(thread);
                        }
                        for (thread, member) in page.members {
                            g.set_thread_member(thread, Some(member));
                        }
                    });
                }
                (result, _) => {
                    if let Err(e) = result {
                        eprintln!("threads: couldn't load archived threads: {e}");
                    }
                    this.account.state.borrow_mut().archives.entry(channel).or_default().failed();
                    this.refresh_threads_view();
                }
            }
        });
    }

    /// Open `thread`, closing the browser. One we have no record of is asked
    /// for first, once: the client does so for a link to a thread
    /// (`loadThread` in 353202).
    pub(super) fn open_thread(self: &Rc<Self>, thread: Snowflake) {
        if let Some(view) = self.threads_view.take() {
            view.dialog.force_close();
        }
        let (known, first, rest) = {
            let mut state = self.account.state.borrow_mut();
            let known = state.find_channel(thread).is_some();
            (known, !known && state.fetched_threads.insert(thread), state.rest.clone())
        };
        if known {
            self.open_channel(thread);
            return;
        }
        let Some(rest) = rest.filter(|_| first) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rest.thread(thread).await {
                Ok((guild, found)) => {
                    this.on_threads(guild, |g| g.set_thread(found));
                    this.open_channel(thread);
                }
                Err(e) => eprintln!("threads: couldn't load a thread: {e}"),
            }
        });
    }

    /// The threads header button, for where we are.
    pub(super) fn sync_threads_button(&self) {
        let offered = self.account.state.borrow().threads_button(&self.age_agreed());
        self.threads_button.set_visible(offered.is_some());
        self.threads_button.set_sensitive(offered == Some(true));
        self.sync_search_button();
    }

    /// Open `thread`'s menu where `widget` is right-clicked, or from the
    /// keyboard while the focus is in it.
    pub(super) fn thread_menu_on(self: &Rc<Self>, widget: &(impl IsA<gtk::Widget> + Clone), thread: Snowflake) {
        let app = Rc::downgrade(self);
        menus::on_menu(widget, move |widget, at| {
            if let Some(app) = app.upgrade() {
                menus::popup(&app, menus::THREAD, ThreadTarget(thread), widget, at);
            }
        });
    }

    /// Whether `message` started a thread we know and may view, which is
    /// the message's own id as a channel's (`castMessageIdAsChannelId`):
    /// where the box under it, View Thread and the `t` key have a thread to open.
    pub(super) fn has_viewable_thread(&self, message: &Message) -> bool {
        if !message.has_thread() || message.thread_starter().is_some() {
            return false;
        }
        let state = self.account.state.borrow();
        state
            .find_channel(message.id)
            .is_some_and(|(_, t)| t.kind.is_thread() && t.parent_id == Some(message.channel_id) && state.can_view(t.id))
    }

    /// The box under a message that started a thread (`r8` in 190854): the
    /// thread's name and how many messages it holds, opening it. Only for a
    /// thread we know, as there.
    pub(super) fn thread_accessory(self: &Rc<Self>, message: &Message) -> Option<gtk::Widget> {
        if !self.has_viewable_thread(message) {
            return None;
        }
        let (name, detail) = {
            let state = self.account.state.borrow();
            let (_, thread) = state.find_channel(message.id)?;
            let count = match thread.message_count {
                Some(0) => Some("No messages yet".to_owned()),
                Some(1) => Some("1 message".to_owned()),
                Some(n) => Some(format!("{n} messages")),
                None => None,
            };
            let archived = thread.is_archived().then_some("Archived".to_owned());
            let detail: Vec<String> = count.into_iter().chain(archived).collect();
            (thread.name.clone().unwrap_or_else(|| "unnamed".into()), detail.join(" · "))
        };
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&gtk::Image::from_icon_name("chat-message-new-symbolic"));
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        text.append(
            &gtk::Label::builder()
                .label(&name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["heading"])
                .build(),
        );
        if !detail.is_empty() {
            text.append(&gtk::Label::builder().label(&detail).xalign(0.0).css_classes(["caption", "dimmed"]).build());
        }
        line.append(&text);
        let button = gtk::Button::builder().child(&line).halign(gtk::Align::Start).build();
        let thread = message.id;
        button.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.open_thread(thread);
                }
            }
        });
        self.thread_menu_on(&button, thread);
        Some(button.upcast())
    }

    /// What `rule` makes of `thread` for us: none without an account or a server.
    fn thread_rule<T>(
        &self,
        thread: Snowflake,
        rule: impl Fn(&discord::Guild, &Channel, &discord::verification::Account) -> Option<T>,
    ) -> Option<T> {
        let state = self.account.state.borrow();
        match (state.find_channel(thread), &state.account) {
            (Some((Some(guild), thread)), Some(me)) => rule(guild, thread, me),
            _ => None,
        }
    }

    fn close_thread(self: &Rc<Self>, thread: Snowflake) {
        self.edit_thread(thread, vec![threads::close_edit(true)], false);
    }

    fn reopen_thread(self: &Rc<Self>, thread: Snowflake) {
        self.edit_thread(thread, vec![threads::close_edit(false)], true);
    }

    fn lock_thread(self: &Rc<Self>, thread: Snowflake) {
        self.lock_edit(thread, true);
    }

    fn unlock_thread(self: &Rc<Self>, thread: Snowflake) {
        self.lock_edit(thread, false);
    }

    fn lock_edit(self: &Rc<Self>, thread: Snowflake, lock: bool) {
        let archived = self.account.state.borrow().find_channel(thread).is_some_and(|(_, t)| t.is_archived());
        self.edit_thread(thread, threads::lock_edits(archived, lock), archived);
    }

    /// Send the edits one after another; the thread's new state arrives as an
    /// event. A refused reopening gets the client's alert (`unarchiveThread`
    /// in 7909), which only the first edit of a reopen or a lock can be.
    fn edit_thread(self: &Rc<Self>, thread: Snowflake, edits: Vec<serde_json::Value>, reopens: bool) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let post = self.account.state.borrow().is_forum_post(thread);
        let this = self.clone();
        glib::spawn_future_local(async move {
            for (n, edit) in edits.into_iter().enumerate() {
                if let Err(e) = rest.edit_thread(thread, edit).await {
                    eprintln!("thread {thread}: {}", e.for_log());
                    let (title, body) = if reopens && n == 0 {
                        threads::reopen_alert(&e, post)
                    } else {
                        ("Error", "An error occurred, please try again.")
                    };
                    let dialog = adw::AlertDialog::new(Some(title), Some(body));
                    dialog.add_response("ok", "_OK");
                    dialog.present(Some(&this.window));
                    return;
                }
            }
        });
    }

    /// Ask before deleting a thread or a post, in the client's words
    /// (894533), then delete it, or only a post's own message.
    fn confirm_delete_thread(self: &Rc<Self>, thread: Snowflake) {
        let (what, name, manager) = {
            let state = self.account.state.borrow();
            let Some((Some(guild), channel)) = state.find_channel(thread) else { return };
            let Some(me) = &state.account else { return };
            let manager = permissions::in_channel(guild, channel, me, now_unix())
                .contains(permissions::Permissions::MANAGE_THREADS);
            let what = permissions::thread_delete(guild, channel, me, now_unix());
            (what, channel.name.clone().unwrap_or_default(), manager)
        };
        let Some(what) = what else { return };
        let (title, body) = match what {
            Delete::Thread => (
                "Delete Thread",
                format!("Are you sure you want to delete <b>{}</b>? This cannot be undone.", glib::markup_escape_text(&name)),
            ),
            Delete::Post if !manager => (
                "Delete Post",
                format!("Are you sure you want to delete <b>{}</b>? This cannot be undone.", glib::markup_escape_text(&name)),
            ),
            Delete::Post => (
                "Delete Post",
                format!(
                    "Are you sure you want to delete <b>{}</b>? All message replies will also be deleted. This cannot be undone.",
                    glib::markup_escape_text(&name)
                ),
            ),
            Delete::Message => (
                "Delete Message",
                "Are you sure you want to delete your message? Since other members have replied to your post, only your original message will be deleted. This cannot be undone.".to_owned(),
            ),
        };
        let dialog = confirm(title, &body, &format!("_{title}"), adw::ResponseAppearance::Destructive);
        dialog.set_body_use_markup(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != "confirm" {
                return;
            }
            let Some(rest) = this.account.state.borrow().rest.clone() else { return };
            let result = match what {
                // The post's own message has the post's id.
                Delete::Message => rest.delete_message(thread, thread).await,
                _ => rest.delete_thread(thread).await,
            };
            if let Err(e) = result {
                eprintln!("delete {thread}: {}", e.for_log());
                this.show_composer_error(&format!("Could not delete it: {e}"));
            }
        });
    }

    /// Join `thread`, as its menu does. What joined comes back as events.
    fn join_thread(self: &Rc<Self>, thread: Snowflake) {
        self.change_membership(thread, true);
    }

    /// Take the person out of the thread (`removeMember` in 7909); the
    /// thread's members change as the gateway says.
    fn remove_from_thread(self: &Rc<Self>, target: &super::profile::UserTarget) {
        let (Some(thread), Some(rest)) = (target.channel, self.account.state.borrow().rest.clone()) else { return };
        let user = target.user.id;
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = rest.remove_thread_member(thread, user, MENU_LOCATION).await {
                this.show_composer_error(&format!("Could not remove them from the thread: {e}"));
            }
        });
    }

    fn leave_thread(self: &Rc<Self>, thread: Snowflake) {
        self.change_membership(thread, false);
    }

    fn change_membership(self: &Rc<Self>, thread: Snowflake, join: bool) {
        let (rest, forum_guild) = {
            let state = self.account.state.borrow();
            let in_forum = |guild: &discord::Guild, c: &discord::Channel| {
                c.parent_id.and_then(|p| guild.channel(p)).is_some_and(|p| p.kind.is_forum_like())
            };
            let forum_guild = state.find_channel(thread).and_then(|(guild, c)| guild.filter(|g| in_forum(g, c)));
            (state.rest.clone(), forum_guild.map(|g| g.id))
        };
        let Some(rest) = rest else { return };
        // A forum post shows the change at once, and the join is taken back if
        // Discord refuses it (`joinThread` and `leaveThread`, 7909).
        if let Some(guild) = forum_guild {
            self.on_threads(guild, |g| g.set_local_membership(thread, join, now_ms()));
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = match join {
                true => rest.join_thread(thread, MENU_LOCATION).await,
                false => rest.leave_thread(thread, MENU_LOCATION).await,
            };
            if let (Err(_), true, Some(guild)) = (&result, join, forum_guild) {
                this.on_threads(guild, |g| g.set_local_membership(thread, false, now_ms()));
            }
            if let Err(e) = result {
                let verb = if join { "join" } else { "leave" };
                this.show_composer_error(&format!("Could not {verb} the thread: {e}"));
            }
        });
    }
}
