//! Finding threads: the browser a channel's header opens, the box under a
//! message that started one, opening a thread from either or from a "started
//! a thread" line, and joining and leaving. When each request goes is in
//! [`discord::threads`] and [`discord::rest`].

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use discord::threads::{self, Archive, MENU_LOCATION};
use discord::{Channel, ChannelKind, Message, Snowflake, system};
use gtk::glib;

use super::App;
use super::menus::{self, At, Entry, Item, Kind};
use super::state::now_ms;

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
    shows: |app, target| app.state.borrow().thread_joined(target.0) == Some(false),
    kind: Kind::Action(|app, target| app.join_thread(target.0)),
});

/// Leave Thread, on one we're in.
pub(super) const LEAVE_THREAD: Entry<ThreadTarget> = Entry::Item(Item {
    name: "leave-thread",
    label: "_Leave Thread",
    shows: |app, target| app.state.borrow().thread_joined(target.0) == Some(true),
    kind: Kind::Action(|app, target| app.leave_thread(target.0)),
});

/// The browser, open over the window.
pub(super) struct View {
    dialog: adw::Dialog,
    /// The channel whose threads it lists.
    channel: Snowflake,
    list: gtk::ListBox,
    /// The thread each row of `list` opens, `None` for a heading.
    rows: RefCell<Vec<Option<Snowflake>>>,
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
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scroller));
        let dialog =
            adw::Dialog::builder().title("Threads").content_width(480).content_height(560).child(&toolbar).build();
        let view = Rc::new(View { dialog, channel, list, rows: RefCell::default() });

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

    /// What the list shows now: the threads we're in, the other active
    /// ones, and the archived ones asked for so far.
    fn lines(&self, app: &App) -> Vec<Line> {
        let state = app.state.borrow();
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
                detail: detail(t, &state.users),
            }));
        };
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let (joined, others) = (lists.joined.len(), lists.others.len());
        section(format!("{joined} joined thread{}", plural(joined)), &lists.joined);
        section(format!("{others} other active thread{}", plural(others)), &lists.others);
        section("Older threads".into(), &lists.archived);
        let loading = archive.is_loading();
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
            let mut state = app.state.borrow_mut();
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
fn detail(thread: &Channel, users: &std::collections::HashMap<u64, String>) -> String {
    let last = thread
        .last_message_id
        .map(|id| id.unix_ms())
        .or_else(|| thread.thread_metadata.as_ref().and_then(|m| m.archive_timestamp))
        .unwrap_or_else(|| thread.id.unix_ms());
    let active = format!("Last active {} ago", system::humanize(((now_ms() - last) / 1000).max(0)));
    match thread.owner_id.and_then(|owner| users.get(&owner.0)).filter(|_| thread.is_archived()) {
        Some(owner) => format!("Started by {owner} · {active}"),
        None => active,
    }
}

impl App {
    /// The threads of `channel`, or of a thread's parent: open the browser,
    /// and ask for archived threads if it never has.
    pub(super) fn open_threads(self: &Rc<Self>, channel: Snowflake) {
        let parent = {
            let state = self.state.borrow();
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
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let page = rest.archived_threads(&discord::forum::Search::plain(channel, offset)).await;
            let guild = this.state.borrow().find_channel(channel).and_then(|(g, _)| g.map(|g| g.id));
            match (page, guild) {
                (Ok(page), Some(guild)) => {
                    let oldest = page.threads.last().map(|t| t.id);
                    this.state.borrow_mut().archives.entry(channel).or_default().landed(offset, page.has_more, oldest);
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
                    this.state.borrow_mut().archives.entry(channel).or_default().failed();
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
            let mut state = self.state.borrow_mut();
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

    /// The threads header button: shown for a channel that takes threads
    /// and for a thread, whose parent's threads it lists (`renderHeaderToolbar`
    /// in 289287).
    pub(super) fn sync_threads_button(&self) {
        let shown = {
            let state = self.state.borrow();
            let channel = state.channel.filter(|_| state.page.is_none()).and_then(|id| state.find_channel(id));
            match channel {
                Some((Some(guild), c)) if c.kind.is_thread() => {
                    c.parent_id.and_then(|p| guild.channel(p)).is_some_and(|p| !p.kind.is_forum_like())
                }
                Some((Some(_), c)) => matches!(c.kind, ChannelKind::Text | ChannelKind::Announcement),
                _ => false,
            }
        };
        self.threads_button.set_visible(shown);
    }

    /// Open `thread`'s menu where `widget` is right-clicked.
    pub(super) fn thread_menu_on(self: &Rc<Self>, widget: &(impl IsA<gtk::Widget> + Clone), thread: Snowflake) {
        let (app, widget_weak) = (Rc::downgrade(self), widget.downgrade());
        menus::on_secondary(widget, move |x, y| {
            let (Some(app), Some(widget)) = (app.upgrade(), widget_weak.upgrade()) else { return };
            menus::popup(&app, menus::THREAD, ThreadTarget(thread), &widget, At::Point(x, y));
        });
    }

    /// The box under a message that started a thread (`r8` in 679636): the
    /// thread's name and how many messages it holds, opening it. Only for a
    /// thread we know, as there.
    pub(super) fn thread_accessory(self: &Rc<Self>, message: &Message) -> Option<gtk::Widget> {
        if !message.has_thread() || message.thread_starter().is_some() {
            return None;
        }
        let (name, detail) = {
            let state = self.state.borrow();
            let (_, thread) = state
                .find_channel(message.id)
                .filter(|(_, t)| t.kind.is_thread() && t.parent_id == Some(message.channel_id))
                .filter(|(_, t)| state.can_view(t.id))?;
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

    /// Join `thread`, as its menu does. What joined comes back as events.
    fn join_thread(self: &Rc<Self>, thread: Snowflake) {
        self.change_membership(thread, true);
    }

    fn leave_thread(self: &Rc<Self>, thread: Snowflake) {
        self.change_membership(thread, false);
    }

    fn change_membership(self: &Rc<Self>, thread: Snowflake, join: bool) {
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = match join {
                true => rest.join_thread(thread, MENU_LOCATION).await,
                false => rest.leave_thread(thread, MENU_LOCATION).await,
            };
            if let Err(e) = result {
                let verb = if join { "join" } else { "leave" };
                this.show_composer_error(&format!("Could not {verb} the thread: {e}"));
            }
        });
    }
}
