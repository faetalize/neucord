//! Who reacted: the tooltip over a reaction, and the list View Reactions
//! opens (`112753`). Both read what `discord::reactions::Reactors` holds, and
//! ask Discord for it the way the client does.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::model::Emoji;
use discord::reactions::{Kind, Location, who_reacted};
use discord::{Message, Snowflake, User};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::menus::{Entry, Item, Kind as MenuKind};
use super::polls;
use super::reactions::emoji_label;
use super::widgets::avatar_widget;
use crate::images::Row;
use crate::timelines;

/// How long a pointer rests on a reaction before we ask who reacted (`eM` in
/// 30492).
const DWELL: Duration = Duration::from_millis(500);
/// How many people the tooltip asks for (`e_` there), and a page of the list
/// (`loadMore` in 112753).
const TOOLTIP: u32 = 3;
const PAGE: u32 = 100;
/// How close to the end of the list scrolling asks for the next page.
const NEAR_END: f64 = 52.0;

/// With the reactions, as the client's menu has it (761304). Super
/// reactions aren't listed here, so a message with only those has no entry.
pub(super) const VIEW_REACTIONS: Entry<MessageTarget> = Entry::Item(Item {
    name: "view-reactions",
    label: "_View Reactions",
    shows: |_, target| !tabs(&target.message).is_empty(),
    kind: MenuKind::Action(|app, target| app.view_reactions(&target.message)),
});

/// The reactions the list has tabs for, the most reacted first.
fn tabs(message: &Message) -> Vec<(Emoji, u32)> {
    let mut tabs: Vec<(Emoji, u32)> = message
        .reaction_pills()
        .into_iter()
        .filter(|p| p.kind == Kind::Normal)
        .map(|p| (p.emoji.clone(), p.count))
        .collect();
    tabs.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    tabs
}

/// The list of who reacted, open over the window.
pub(super) struct View {
    dialog: adw::Dialog,
    channel: Snowflake,
    message: Snowflake,
    tabs: gtk::ListBox,
    people: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    /// The emoji each tab row is for.
    emoji: RefCell<Vec<Emoji>>,
    rebuilding: Cell<bool>,
    shown: RefCell<Shown>,
}

#[derive(Default)]
struct Shown {
    emoji: Option<Emoji>,
    /// How many people reacted with it, as the message says.
    count: u32,
    /// The last person a page ended with, which the next starts after.
    last: Option<Snowflake>,
    loading: bool,
    /// Counts the times another reaction was chosen, so a page that was
    /// asked for before doesn't say where the next one starts.
    round: u64,
}

impl View {
    fn new(app: &Rc<App>, message: &Message) -> Rc<View> {
        let tabs = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        let people = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        let tab_scroller = gtk::ScrolledWindow::builder()
            .child(&tabs)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(130)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&people)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&tab_scroller);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&scroller);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&body));
        let dialog =
            adw::Dialog::builder().title("Reactions").content_width(520).content_height(440).child(&toolbar).build();
        let view = Rc::new(View {
            dialog,
            channel: message.channel_id,
            message: message.id,
            tabs,
            people,
            scroller,
            emoji: RefCell::default(),
            rebuilding: Cell::new(false),
            shown: RefCell::default(),
        });

        view.tabs.connect_row_selected({
            let app = Rc::downgrade(app);
            move |_, row| {
                let (Some(app), Some(row)) = (app.upgrade(), row) else { return };
                let Some(view) = app.reactors_view.borrow().clone() else { return };
                if !view.rebuilding.get() {
                    view.choose(&app, usize::try_from(row.index()).unwrap_or_default());
                }
            }
        });
        view.scroller.vadjustment().connect_value_changed({
            let app = Rc::downgrade(app);
            move |adjustment| {
                let (Some(app), true) =
                    (app.upgrade(), adjustment.value() + adjustment.page_size() >= adjustment.upper() - NEAR_END)
                else {
                    return;
                };
                if let Some(view) = app.reactors_view.borrow().clone() {
                    view.more(&app);
                }
            }
        });
        view
    }

    /// Show what the message has now: the tabs, and who is under the chosen
    /// one. The client's list follows the message the same way, and goes when
    /// nothing is left to list.
    pub(super) fn refresh(self: &Rc<Self>, app: &Rc<App>) {
        let tabs = app.message_in(self.channel, self.message).map(|m| tabs(&m)).unwrap_or_default();
        if tabs.is_empty() {
            self.dialog.force_close();
            return;
        }
        let current = {
            let shown = self.shown.borrow();
            let kept = shown.emoji.as_ref().and_then(|e| tabs.iter().position(|(t, _)| t.same(e)));
            kept.unwrap_or(0)
        };
        self.rebuilding.set(true);
        while let Some(row) = self.tabs.first_child() {
            self.tabs.remove(&row);
        }
        for (emoji, count) in &tabs {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            line.append(&polls::emoji_widget(emoji, &app.images));
            line.append(&gtk::Label::builder().label(count.to_string()).hexpand(true).xalign(1.0).build());
            self.tabs.append(&line);
        }
        *self.emoji.borrow_mut() = tabs.iter().map(|(e, _)| e.clone()).collect();
        if let Some(row) = self.tabs.row_at_index(current as i32) {
            self.tabs.select_row(Some(&row));
        }
        self.rebuilding.set(false);
        let (emoji, count) = tabs[current].clone();
        let changed = !self.shown.borrow().emoji.as_ref().is_some_and(|shown| shown.same(&emoji));
        self.shown.borrow_mut().count = count;
        if changed {
            self.choose(app, current);
        } else {
            self.fill(app);
        }
    }

    /// A tab was chosen: its people are asked for from the start.
    fn choose(self: &Rc<Self>, app: &Rc<App>, tab: usize) {
        let Some(emoji) = self.emoji.borrow().get(tab).cloned() else { return };
        {
            let mut shown = self.shown.borrow_mut();
            if shown.emoji.as_ref().is_some_and(|e| e.same(&emoji)) {
                return;
            }
            let count = app
                .message_in(self.channel, self.message)
                .and_then(|m| tabs(&m).into_iter().find(|(e, _)| e.same(&emoji)).map(|(_, count)| count));
            *shown = Shown {
                emoji: Some(emoji),
                count: count.unwrap_or_default(),
                round: shown.round + 1,
                ..Shown::default()
            };
        }
        self.fill(app);
        self.more(app);
    }

    /// Ask for the next page, unless one is on its way or all are here.
    fn more(self: &Rc<Self>, app: &Rc<App>) {
        let (emoji, after, round) = {
            let mut shown = self.shown.borrow_mut();
            let Some(emoji) = shown.emoji.clone() else { return };
            let listed = app.state.borrow_mut().reactors.users(self.message, &emoji, Kind::Normal).len();
            // The first page is asked for whatever is known: events may have
            // listed a few, and the message's count says there are more.
            if shown.loading || (shown.last.is_some() && shown.count as usize <= listed) {
                return;
            }
            shown.loading = true;
            (emoji, shown.last, shown.round)
        };
        let Some(rest) = app.state.borrow().rest.clone() else { return };
        let (view, app) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let page = rest.reactors(view.channel, view.message, &emoji, PAGE, after).await;
            let mut shown = view.shown.borrow_mut();
            let current = shown.round == round;
            if current {
                shown.loading = false;
            }
            let Ok(users) = page else { return };
            if current {
                shown.last = users.last().map(|u| u.id).or(shown.last);
                // Nothing came back: there is no more, however many are said to be.
                if users.is_empty() {
                    shown.count = 0;
                }
            }
            drop(shown);
            app.state.borrow_mut().reactors.arrived(view.message, &emoji, Kind::Normal, users);
            if current {
                view.fill(&app);
            }
        });
    }

    /// The people of the chosen tab, as far as we know them.
    fn fill(self: &Rc<Self>, app: &Rc<App>) {
        while let Some(row) = self.people.first_child() {
            self.people.remove(&row);
        }
        let Some(emoji) = self.shown.borrow().emoji.clone() else { return };
        let users: Vec<User> = app.state.borrow_mut().reactors.users(self.message, &emoji, Kind::Normal).to_vec();
        let (me, manager) = {
            let state = app.state.borrow();
            (state.me(), state.can_remove_reactions(self.channel))
        };
        for user in users {
            let line =
                gtk::Box::builder().spacing(10).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
            line.append(&avatar_widget(&user, 32, Some(&app.images), Row::Parent));
            let name =
                app.state.borrow().users.get(&user.id.0).cloned().unwrap_or_else(|| user.display_name().to_owned());
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(true)
                .valign(gtk::Align::Center)
                .build();
            text.append(
                &gtk::Label::builder()
                    .label(&name)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["heading"])
                    .build(),
            );
            if name != user.username {
                text.append(
                    &gtk::Label::builder().label(&user.username).xalign(0.0).css_classes(["caption", "dimmed"]).build(),
                );
            }
            line.append(&text);
            if manager || me == Some(user.id) {
                let remove = gtk::Button::builder()
                    .icon_name("window-close-symbolic")
                    .tooltip_text("Remove Reaction")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                let (app, emoji, channel, message, who) =
                    (Rc::downgrade(app), emoji.clone(), self.channel, self.message, user.id);
                remove.connect_clicked(move |_| {
                    if let Some(app) = app.upgrade() {
                        app.take_back_reaction(channel, message, emoji.clone(), Some(who), Location::Message);
                    }
                });
                line.append(&remove);
            }
            self.people.append(&line);
        }
    }
}

impl App {
    /// Open the list of who reacted to `message`.
    pub(super) fn view_reactions(self: &Rc<Self>, message: &Message) {
        if let Some(old) = self.reactors_view.take() {
            old.dialog.force_close();
        }
        let view = View::new(self, message);
        *self.reactors_view.borrow_mut() = Some(view.clone());
        view.dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |closed| {
                let Some(this) = this.upgrade() else { return };
                let ours = this.reactors_view.borrow().as_ref().is_some_and(|v| v.dialog == *closed);
                if ours {
                    this.reactors_view.take();
                }
            }
        });
        view.dialog.present(Some(&self.window));
        view.refresh(self);
    }

    /// The list follows the message it shows.
    pub(super) fn refresh_reactors_view(self: &Rc<Self>, channel: Snowflake, message: Snowflake) {
        let view = self.reactors_view.borrow().clone();
        if let Some(view) = view.filter(|v| (v.channel, v.message) == (channel, message)) {
            view.refresh(self);
        }
    }

    /// What we hold of `message`, in a channel's messages.
    pub(super) fn message_in(&self, channel: Snowflake, message: Snowflake) -> Option<Message> {
        let store = self.timelines.borrow().get(channel)?.store.clone();
        timelines::find(&store, message).map(|(_, message)| message)
    }

    /// Have `button`, which shows `emoji`'s reactions, say who made them: the
    /// tooltip, and asking Discord once the pointer has rested on it.
    pub(super) fn attach_reactors_tooltip(
        self: &Rc<Self>,
        button: &gtk::ToggleButton,
        channel: Snowflake,
        message: Snowflake,
        emoji: Emoji,
        count: u32,
    ) {
        button.set_has_tooltip(true);
        button.connect_query_tooltip({
            let (app, emoji) = (Rc::downgrade(self), emoji.clone());
            move |_, _, _, _, tooltip| {
                let Some(text) = app.upgrade().and_then(|app| app.reactors_tooltip(message, &emoji, count)) else {
                    return false;
                };
                tooltip.set_text(Some(&text));
                true
            }
        });
        let resting = Rc::new(Cell::new(0u64));
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let (app, resting, button) = (Rc::downgrade(self), resting.clone(), button.downgrade());
            move |_, _, _| {
                resting.set(resting.get() + 1);
                let (turn, app, resting, button, emoji) =
                    (resting.get(), app.clone(), resting.clone(), button.clone(), emoji.clone());
                glib::timeout_add_local_once(DWELL, move || {
                    let (Some(app), true) = (app.upgrade(), resting.get() == turn) else { return };
                    app.ask_who_reacted(channel, message, emoji, button);
                });
            }
        });
        motion.connect_leave(move |_| resting.set(resting.get() + 1));
        button.add_controller(motion);
    }

    /// Ask for the first few who reacted, the first time they are wanted
    /// (`getReactions` in 956703), and show the tooltip when they come.
    fn ask_who_reacted(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        emoji: Emoji,
        button: glib::WeakRef<gtk::ToggleButton>,
    ) {
        let first = self.state.borrow_mut().reactors.first_ask(message, &emoji, Kind::Normal);
        let Some(rest) = self.state.borrow().rest.clone().filter(|_| first) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(users) = rest.reactors(channel, message, &emoji, TOOLTIP, None).await {
                this.state.borrow_mut().reactors.arrived(message, &emoji, Kind::Normal, users);
                if let Some(button) = button.upgrade() {
                    button.trigger_tooltip_query();
                }
            }
        });
    }

    /// "🔥 reacted by A, B, and 2 others": up to three of those we know made
    /// it, leaving out people we blocked (`e_` in 30492).
    fn reactors_tooltip(&self, message: Snowflake, emoji: &Emoji, count: u32) -> Option<String> {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let names: Vec<String> = state
            .reactors
            .users(message, emoji, Kind::Normal)
            .iter()
            .filter(|u| !state.relationships.is_blocked(u.id))
            .take(3)
            .map(|u| state.users.get(&u.id.0).cloned().unwrap_or_else(|| u.display_name().to_owned()))
            .collect();
        who_reacted(&emoji_label(emoji), &names, count)
    }
}
