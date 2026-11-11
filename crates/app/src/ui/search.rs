//! Searching messages (Ctrl+F, or the header's search button): a box with the
//! search field, what it suggests, and the results, which open the message
//! in its channel. The grammar, the request and when to ask again are
//! [`discord::search`]; this draws them and sends.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::rest::Searched;
use discord::search::{self, Context, Request, Sort, Status, Suggestion};
use discord::{Channel, ChannelKind, Message, Snowflake, User};
use gtk::{gdk, glib, pango};

use super::App;
use super::chat::MessageTarget;
use super::menus::{self, At};
use super::state::State;
use super::widgets::avatar_widget;

/// What a search remembers while its box is closed.
#[derive(Default)]
pub(super) struct Saved {
    text: String,
    sort: Sort,
    status: Status,
    offset: u32,
    /// `search_session_id`: made when the box opens, kept till the session ends.
    session: Option<String>,
    /// `search_query_id`: made at each search the user starts.
    query: Option<String>,
    /// Which request is current; an older one's answer is dropped, and its
    /// waiting stops (the client cancels the fetcher it replaces).
    generation: u64,
    /// Blocked authors' results the user chose to show.
    show_blocked: bool,
}

/// The box, open over the window.
pub(super) struct View {
    dialog: adw::Dialog,
    context: Context,
    entry: gtk::Entry,
    suggestions: gtk::ListBox,
    offered: RefCell<Vec<Suggestion>>,
    selected: Cell<Option<usize>>,
    summary: gtk::Label,
    sort: gtk::DropDown,
    sort_syncing: Cell<bool>,
    results: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    message: gtk::Label,
    pager: gtk::Box,
    page_label: gtk::Label,
    previous: gtk::Button,
    next: gtk::Button,
    /// Counts Enter presses, so the last one's wait is the one that sends.
    submissions: Cell<u64>,
    /// The message each result row opens.
    rows: RefCell<Vec<Option<Message>>>,
}

/// What the grammar looks people and channels up in.
struct Known {
    users: Vec<User>,
    /// The server's channels and threads by the name a query writes them.
    channels: Vec<(String, Snowflake)>,
    me: Snowflake,
}

impl Known {
    fn lookup<'a>(
        &'a self,
        context: Context,
        zone: &'a glib::TimeZone,
        user: &'a dyn Fn(&str, Option<&str>) -> Option<Snowflake>,
        channels: &'a dyn Fn(&str) -> Vec<Snowflake>,
    ) -> search::Lookup<'a> {
        search::Lookup { context, me: self.me, user, channels, zone, now: glib::real_time() / 1000 }
    }

    fn find_user(&self, name: &str, tag: Option<&str>) -> Option<Snowflake> {
        self.users
            .iter()
            .find(|u| {
                u.username == name
                    && match tag {
                        Some(tag) => u.discriminator.as_deref() == Some(tag),
                        None => u.discriminator.as_deref().is_none_or(|d| d == "0"),
                    }
            })
            .map(|u| u.id)
    }

    fn find_channels(&self, name: &str) -> Vec<Snowflake> {
        self.channels.iter().filter(|(n, _)| n == name).map(|(_, id)| *id).collect()
    }

    fn tokens(&self, context: Context, text: &str) -> Vec<search::Token> {
        let zone = glib::TimeZone::local();
        let (user, channels) = (|n: &str, t: Option<&str>| self.find_user(n, t), |n: &str| self.find_channels(n));
        search::tokenize(text, &self.lookup(context, &zone, &user, &channels))
    }
}

impl App {
    /// Where the open channel is searched: its server, or the DM.
    pub(super) fn search_context(&self) -> Option<Context> {
        let state = self.state.borrow();
        let id = state.channel.filter(|_| state.page.is_none())?;
        match state.find_channel(id)? {
            (Some(guild), _) => Some(Context::Guild(guild.id)),
            (None, channel) if channel.kind.is_private() => Some(Context::Channel(channel.id)),
            _ => None,
        }
    }

    pub(super) fn sync_search_button(&self) {
        self.search_button.set_visible(self.search_context().is_some());
    }

    /// People we hold the details of: friends, the people in our DMs, and
    /// the authors and mentions of the messages the open channel has.
    fn known_users(&self, state: &State) -> Vec<User> {
        let mut users: Vec<User> =
            state.relationships.list(discord::relationships::List::All).iter().map(|r| r.user.clone()).collect();
        users.extend(state.private_channels.iter().flat_map(|c| c.recipients.iter().cloned()));
        let store = self.messages();
        for i in 0..store.n_items() {
            if let Some(message) = crate::timelines::message_at(&store, i) {
                users.push(message.author);
                users.extend(message.mentions);
            }
        }
        let mut seen = std::collections::HashSet::new();
        users.retain(|u| seen.insert(u.id));
        users
    }

    fn search_known(&self, context: Context) -> Known {
        let state = self.state.borrow();
        let mut channels = Vec::new();
        if let Context::Guild(id) = context
            && let Some(guild) = state.guilds.iter().find(|g| g.id == id)
        {
            // A name taken twice is `name`, then `name~1`, `name~2`, in the
            // sidebar's order (`getTextChannelNameDisambiguations`, 808728).
            let mut text: Vec<&Channel> = guild
                .channels
                .iter()
                .filter(|c| {
                    matches!(
                        c.kind,
                        ChannelKind::Text | ChannelKind::Announcement | ChannelKind::Forum | ChannelKind::Media
                    )
                })
                .filter(|c| state.can_view(c.id))
                .collect();
            text.sort_by_key(|c| c.position.unwrap_or(0));
            let mut taken: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            for c in text {
                let name = c.name.clone().unwrap_or_default();
                let count = taken.entry(name.clone()).or_insert(0);
                channels.push((if *count == 0 { name } else { format!("{name}~{}", *count - 1) }, c.id));
                *count += 1;
            }
            channels.extend(
                guild
                    .channels
                    .iter()
                    .filter(|c| c.kind.is_vocal() && state.can_view(c.id))
                    .chain(&guild.threads)
                    .filter_map(|c| Some((c.name.clone()?, c.id))),
            );
        }
        Known { users: self.known_users(&state), channels, me: state.me().unwrap_or_default() }
    }

    /// Open the search box, or close it.
    pub(super) fn toggle_search(self: &Rc<Self>) {
        if let Some(open) = self.search_view.take() {
            open.dialog.force_close();
            return;
        }
        let Some(context) = self.search_context() else { return };
        let view = View::new(self, context);
        *self.search_view.borrow_mut() = Some(view.clone());
        view.dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |closed| {
                let Some(this) = this.upgrade() else { return };
                let ours = this.search_view.borrow().as_ref().is_some_and(|v| v.dialog == *closed);
                if ours {
                    this.search_view.take();
                }
            }
        });
        let text = {
            let mut state = self.state.borrow_mut();
            let saved = state.searches.entry(context).or_default();
            // A search session starts when the box gets focus (`initialize`).
            saved.session.get_or_insert_with(|| glib::uuid_string_random().to_string());
            saved.text.clone()
        };
        // Setting the text suggests, which reads the session.
        view.entry.set_text(&text);
        view.dialog.present(Some(&self.window));
        view.refresh(self);
    }

    /// Ask Discord, `submitted` being the user's Enter rather than a change
    /// of sort or page.
    fn run_search(self: &Rc<Self>, view: &Rc<View>, offset: u32, submitted: bool) {
        let context = view.context;
        let text = view.entry.text().to_string();
        let known = self.search_known(context);
        let tokens = known.tokens(context, &text);
        // Filters given nothing are cut out before asking (`K` in 586944).
        let text = search::cleaned(&text, &tokens);
        let tokens = known.tokens(context, &text);
        let fields = search::fields(&tokens);
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        if !search::askable(&tokens) {
            return;
        }
        let (request, generation) = {
            let mut state = self.state.borrow_mut();
            let discoverable = match context {
                Context::Guild(id) => state
                    .guilds
                    .iter()
                    .find(|g| g.id == id)
                    .is_some_and(|g| g.features.iter().any(|f| f == "DISCOVERABLE")),
                Context::Channel(_) => false,
            };
            let saved = state.searches.entry(context).or_default();
            if submitted {
                saved.sort = Sort::Newest;
                saved.query = Some(glib::uuid_string_random().to_string());
            }
            saved.text = text.clone();
            saved.offset = offset;
            saved.generation += 1;
            saved.status = Status::Searching;
            let discovery = discoverable.then(|| Some((saved.session.clone()?, saved.query.clone()?))).flatten();
            (Request { context, fields, sort: saved.sort, offset, discovery }, saved.generation)
        };
        view.refresh(self);
        let (this, view) = (self.clone(), view.clone());
        glib::spawn_future_local(async move {
            let mut attempts = 0;
            loop {
                let answer = rest.search_messages(&request, attempts).await;
                if this.state.borrow().searches.get(&context).is_none_or(|s| s.generation != generation) {
                    return;
                }
                let status = match answer {
                    Ok(Searched::Page(page)) => Status::Done(page),
                    Ok(Searched::Indexing { retry_after }) => {
                        let Some((next, delay)) = search::after_indexing(attempts, retry_after.as_deref()) else {
                            return;
                        };
                        attempts = next;
                        this.set_search_status(context, Status::Indexing, &view);
                        glib::timeout_future(delay).await;
                        if this.state.borrow().searches.get(&context).is_none_or(|s| s.generation != generation) {
                            return;
                        }
                        continue;
                    }
                    Err(e) => {
                        eprintln!("search: {e}");
                        Status::Failed
                    }
                };
                this.set_search_status(context, status, &view);
                return;
            }
        });
    }

    fn set_search_status(self: &Rc<Self>, context: Context, status: Status, view: &Rc<View>) {
        if let Some(saved) = self.state.borrow_mut().searches.get_mut(&context) {
            saved.status = status;
        }
        view.refresh(self);
    }

    /// Open `message` in its channel and scroll to it.
    fn open_search_result(self: &Rc<Self>, message: &Message) {
        self.open_channel_at(message.channel_id, Some(message.id));
    }
}

impl View {
    fn new(app: &Rc<App>, context: Context) -> Rc<View> {
        let placeholder = {
            let state = app.state.borrow();
            match context {
                Context::Guild(id) => state.guilds.iter().find(|g| g.id == id).map(|g| format!("Search {}", g.name)),
                Context::Channel(id) => state.find_channel(id).map(|(_, c)| format!("Search {}", c.title())),
            }
            .unwrap_or_else(|| "Search".to_owned())
        };
        let entry = gtk::Entry::builder()
            .placeholder_text(placeholder)
            .max_length(search::MAX_QUERY as i32)
            .accessible_role(gtk::AccessibleRole::SearchBox)
            .build();
        let suggestions = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).visible(false).build();
        let summary = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["heading"]).build();
        let sort = gtk::DropDown::from_strings(&Sort::ALL.map(Sort::label));
        let results = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&results)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let message = gtk::Label::builder().wrap(true).css_classes(["dimmed"]).margin_top(24).margin_bottom(24).build();
        let (previous, next) =
            (gtk::Button::from_icon_name("go-previous-symbolic"), gtk::Button::from_icon_name("go-next-symbolic"));
        let page_label = gtk::Label::new(None);
        let pager = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).visible(false).build();
        pager.append(&previous);
        pager.append(&page_label);
        pager.append(&next);

        let head = gtk::Box::builder().spacing(12).build();
        head.append(&summary);
        head.append(&sort);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&entry);
        content.append(&suggestions);
        content.append(&head);
        content.append(&scroller);
        content.append(&message);
        content.append(&pager);
        let dialog = adw::Dialog::builder()
            .title("Search")
            .content_width(640)
            .content_height(640)
            .focus_widget(&entry)
            .child(&content)
            .build();
        let view = Rc::new(View {
            dialog,
            context,
            entry,
            suggestions,
            offered: RefCell::default(),
            selected: Cell::new(None),
            summary,
            sort,
            sort_syncing: Cell::new(false),
            results,
            scroller,
            message,
            pager,
            page_label,
            previous,
            next,
            submissions: Cell::new(0),
            rows: RefCell::default(),
        });

        view.entry.connect_changed({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                view.suggest(&app);
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_, key, _, modifiers| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else {
                    return glib::Propagation::Proceed;
                };
                view.key(&app, key, modifiers)
            }
        });
        view.entry.add_controller(keys);
        view.entry.connect_activate({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                view.enter(&app);
            }
        });
        view.suggestions.connect_row_activated({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_, row| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                if let Ok(i) = usize::try_from(row.index()) {
                    view.choose(&app, i);
                }
            }
        });
        view.sort.connect_selected_notify({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |dropdown| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                if view.sort_syncing.get() {
                    return;
                }
                let sort = Sort::ALL[dropdown.selected() as usize];
                let (changed, busy) = {
                    let mut state = app.state.borrow_mut();
                    let saved = state.searches.entry(view.context).or_default();
                    (saved.sort != sort, saved.status == Status::Searching)
                };
                // Not while a search is under way (`E` in 252715).
                if changed && !busy {
                    if let Some(saved) = app.state.borrow_mut().searches.get_mut(&view.context) {
                        saved.sort = sort;
                    }
                    app.run_search(&view, 0, false);
                } else if changed {
                    view.refresh(&app);
                }
            }
        });
        for (button, step) in [(&view.previous, -1_i64), (&view.next, 1)] {
            button.connect_clicked({
                let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
                move |_| {
                    let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                    let page = app
                        .state
                        .borrow()
                        .searches
                        .get(&view.context)
                        .map_or(0, |s| i64::from(s.offset / search::PAGE));
                    let page = (page + step).max(0) as u32;
                    app.run_search(&view, page * search::PAGE, false);
                }
            });
        }
        view.results.connect_row_activated({
            let (app, view) = (Rc::downgrade(app), Rc::downgrade(&view));
            move |_, row| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                let message =
                    usize::try_from(row.index()).ok().and_then(|i| view.rows.borrow().get(i).cloned().flatten());
                if let Some(message) = message {
                    view.dialog.force_close();
                    app.open_search_result(&message);
                }
            }
        });
        view
    }

    fn key(self: &Rc<Self>, app: &Rc<App>, key: gdk::Key, _: gdk::ModifierType) -> glib::Propagation {
        let showing = !self.offered.borrow().is_empty();
        match key {
            gdk::Key::Down | gdk::Key::Up if showing => {
                let len = self.offered.borrow().len();
                let next = match (self.selected.get(), key == gdk::Key::Down) {
                    (None, true) => 0,
                    (None, false) => len - 1,
                    (Some(i), true) => (i + 1) % len,
                    (Some(i), false) => (i + len - 1) % len,
                };
                self.select(Some(next));
            }
            // Escape clears the box, then closes it.
            gdk::Key::Escape if !self.entry.text().is_empty() => {
                self.entry.set_text("");
                self.suggest(app);
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn select(&self, index: Option<usize>) {
        self.selected.set(index);
        match index.and_then(|i| self.suggestions.row_at_index(i as i32)) {
            Some(row) => self.suggestions.select_row(Some(&row)),
            None => self.suggestions.unselect_all(),
        }
    }

    /// Enter: take the suggestion chosen, else search.
    fn enter(self: &Rc<Self>, app: &Rc<App>) {
        if let Some(i) = self.selected.get().filter(|_| !self.offered.borrow().is_empty()) {
            return self.choose(app, i);
        }
        self.suggestions.set_visible(false);
        // After half a second, and only the last Enter sends (`te`).
        let n = self.submissions.get() + 1;
        self.submissions.set(n);
        let (app, view) = (app.clone(), self.clone());
        glib::spawn_future_local(async move {
            glib::timeout_future(search::DEBOUNCE).await;
            if view.submissions.get() != n {
                return;
            }
            let busy = app.state.borrow().searches.get(&view.context).is_some_and(|s| s.status == Status::Searching);
            if !busy {
                app.run_search(&view, 0, true);
            }
        });
    }

    fn choose(self: &Rc<Self>, app: &Rc<App>, index: usize) {
        let Some(suggestion) = self.offered.borrow().get(index).cloned() else { return };
        let text = self.entry.text().to_string();
        let cursor = usize::try_from(self.entry.position()).unwrap_or(0);
        let known = app.search_known(self.context);
        let tokens = known.tokens(self.context, &text);
        let (text, cursor) = search::apply(&text, cursor, &tokens, &suggestion);
        self.entry.set_text(&text);
        self.entry.set_position(cursor as i32);
        self.entry.grab_focus_without_selecting();
        self.suggest(app);
    }

    /// What the box offers for what is typed.
    fn suggest(self: &Rc<Self>, app: &Rc<App>) {
        let text = self.entry.text().to_string();
        let cursor = usize::try_from(self.entry.position()).unwrap_or(0);
        let known = app.search_known(self.context);
        let tokens = known.tokens(self.context, &text);
        let names: Vec<String> = known.channels.iter().map(|(n, _)| n.clone()).collect();
        let offered = search::suggest(
            &text,
            cursor,
            &tokens,
            self.context,
            &search::Known { users: &known.users, channels: &names },
        );
        while let Some(row) = self.suggestions.first_child() {
            self.suggestions.remove(&row);
        }
        for suggestion in &offered {
            let (label, detail) = match suggestion {
                Suggestion::Filter(f) => (describe(*f).to_owned(), f.key().to_owned()),
                Suggestion::Value { label, .. } => (label.clone(), String::new()),
            };
            let line =
                gtk::Box::builder().spacing(12).margin_top(4).margin_bottom(4).margin_start(8).margin_end(8).build();
            line.append(
                &gtk::Label::builder()
                    .label(label)
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(pango::EllipsizeMode::End)
                    .build(),
            );
            if !detail.is_empty() {
                line.append(&gtk::Label::builder().label(detail).css_classes(["caption", "dimmed"]).build());
            }
            self.suggestions.append(&gtk::ListBoxRow::builder().child(&line).build());
        }
        self.suggestions.set_visible(!offered.is_empty());
        *self.offered.borrow_mut() = offered;
        self.select(None);
    }

    /// Draw what the search holds now.
    fn refresh(self: &Rc<Self>, app: &Rc<App>) {
        let (status, sort, offset, show_blocked) = {
            let state = app.state.borrow();
            let saved = state.searches.get(&self.context);
            saved.map_or((Status::Idle, Sort::Newest, 0, false), |s| {
                (s.status.clone(), s.sort, s.offset, s.show_blocked)
            })
        };
        self.sort_syncing.set(true);
        self.sort.set_selected(Sort::ALL.iter().position(|&s| s == sort).unwrap_or(0) as u32);
        self.sort_syncing.set(false);
        while let Some(row) = self.results.first_child() {
            self.results.remove(&row);
        }
        let mut rows = Vec::new();
        let (summary, message, total) = match &status {
            Status::Idle => (String::new(), String::new(), 0),
            Status::Searching => ("Searching…".to_owned(), String::new(), 0),
            Status::Indexing => ("Indexing".to_owned(), self.indexing_text(), 0),
            Status::Failed => {
                (String::new(), "We dropped the magnifying glass. Can you try searching again?".to_owned(), 0)
            }
            Status::Done(page) => {
                let summary = match page.total {
                    0 => "No Results".to_owned(),
                    1 => "1 Result".to_owned(),
                    n => format!("{n} Results"),
                };
                let (shown, blocked) = {
                    let state = app.state.borrow();
                    let mut blocked = 0;
                    let shown: Vec<&Message> = page
                        .messages
                        .iter()
                        .filter(|m| {
                            let hide = state.relationships.is_blocked(m.author.id) && !show_blocked;
                            blocked += usize::from(hide);
                            !hide
                        })
                        .collect();
                    (shown.into_iter().cloned().collect::<Vec<_>>(), blocked)
                };
                for message in shown {
                    self.results.append(&app.search_result_row(&message, self.context));
                    rows.push(Some(message));
                }
                if blocked > 0 {
                    let button = gtk::Button::builder()
                        .label(format!("Show {blocked} blocked message{}", if blocked == 1 { "" } else { "s" }))
                        .margin_top(6)
                        .margin_bottom(6)
                        .build();
                    button.connect_clicked({
                        let (app, view) = (Rc::downgrade(app), Rc::downgrade(self));
                        move |_| {
                            let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                            if let Some(saved) = app.state.borrow_mut().searches.get_mut(&view.context) {
                                saved.show_blocked = true;
                            }
                            view.refresh(&app);
                        }
                    });
                    self.results.append(
                        &gtk::ListBoxRow::builder().child(&button).activatable(false).selectable(false).build(),
                    );
                    rows.push(None);
                }
                let message = if page.total == 0 {
                    "We searched far and wide. Unfortunately, no results were found.".to_owned()
                } else if page.indexing {
                    format!("{} indexed so far. We’re still indexing older messages.", count(page.documents_indexed))
                } else {
                    String::new()
                };
                (summary, message, page.total)
            }
        };
        *self.rows.borrow_mut() = rows;
        self.summary.set_label(&summary);
        self.message.set_label(&message);
        self.message.set_visible(!message.is_empty());
        let pages = search::pages(total);
        let page = u64::from(offset / search::PAGE);
        self.pager.set_visible(total > u64::from(search::PAGE));
        self.page_label.set_label(&format!("Page {} of {pages}", page + 1));
        self.previous.set_sensitive(page > 0 && status != Status::Searching);
        self.next.set_sensitive(page + 1 < pages && status != Status::Searching);
        self.scroller.vadjustment().set_value(0.0);
    }

    fn indexing_text(&self) -> String {
        match self.context {
            Context::Guild(_) => "Before searching, we need to index this server. Give us a bit.",
            Context::Channel(_) => "Before searching, we need to index this DM. Give us a bit.",
        }
        .to_owned()
    }
}

fn count(n: u64) -> String {
    match n {
        0 => "No results".to_owned(),
        1 => "1 result".to_owned(),
        n => format!("{n} results"),
    }
}

/// What a filter row says about it (`ktr6z5` and its neighbours).
fn describe(filter: search::Filter) -> &'static str {
    match filter {
        search::Filter::From => "From a specific user",
        search::Filter::In => "Sent in a specific channel",
        search::Filter::Has => "Includes a specific type of data",
        search::Filter::Mentions => "Mentions a specific user",
        _ => "",
    }
}

impl App {
    /// A result: who wrote it and when, where, and the text.
    fn search_result_row(self: &Rc<Self>, message: &Message, context: Context) -> gtk::ListBoxRow {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        let place = {
            let state = self.state.borrow();
            match (context, state.find_channel(message.channel_id)) {
                (Context::Guild(_), Some((_, channel))) => {
                    let name = channel.name.clone().unwrap_or_default();
                    Some(if channel.kind.is_thread() { name } else { format!("#{name}") })
                }
                _ => None,
            }
        };
        if let Some(place) = place {
            column.append(&gtk::Label::builder().label(place).xalign(0.0).css_classes(["caption", "dimmed"]).build());
        }
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&avatar_widget(&message.author, 24, Some(&self.images)));
        let name = self
            .state
            .borrow()
            .relationships
            .get(message.author.id)
            .map_or_else(|| message.author.display_name().to_owned(), |r| r.name().to_owned());
        line.append(&gtk::Label::builder().label(name).css_classes(["heading"]).build());
        if let Some(time) = super::chat::parse_time(&message.timestamp) {
            line.append(
                &gtk::Label::builder()
                    .label(super::chat::format_time(&time))
                    .css_classes(["caption", "dimmed"])
                    .build(),
            );
        }
        column.append(&line);
        let text = crate::render::plain_text(&message.markup(), &self.message_names(message));
        let mut text = if text.is_empty() && !message.attachments.is_empty() { "Attachment".to_owned() } else { text };
        if text.chars().count() > 600 {
            text = text.chars().take(600).collect::<String>() + "…";
        }
        column.append(
            &gtk::Label::builder().label(text).xalign(0.0).wrap(true).wrap_mode(pango::WrapMode::WordChar).build(),
        );
        let row = gtk::ListBoxRow::builder().child(&column).build();
        let (app, message) = (Rc::downgrade(self), message.clone());
        menus::on_secondary(&row, {
            let row = row.downgrade();
            move |x, y| {
                let (Some(app), Some(row)) = (app.upgrade(), row.upgrade()) else { return };
                let target = MessageTarget {
                    message: message.clone(),
                    row: row.clone().upcast(),
                    selection: None,
                    link: None,
                    sticker: None,
                };
                menus::popup(&app, menus::SEARCH_RESULT, target, &row, At::Point(x, y));
            }
        });
        row
    }
}
