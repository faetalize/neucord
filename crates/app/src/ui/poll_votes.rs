//! Who voted for what: the list a poll's vote count opens (`PollVotesModal`,
//! 637806). Its answers are on the left, each with its votes, and the people
//! who chose the selected one on the right, a hundred at a time as the
//! list is scrolled (`ao` in 649963, with the vote type).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::{Message, Snowflake, User};
use gtk::glib;

use super::App;
use super::polls;
use super::widgets::avatar_widget_in;
use crate::images::Row;

/// A page of the list (`limit` in `loadMore` there).
const PAGE: u32 = 100;
/// How close to the end of the list scrolling asks for the next page.
const NEAR_END: f64 = 52.0;

/// The dialog's answers and people.
struct View {
    dialog: adw::Dialog,
    channel: Snowflake,
    message: Snowflake,
    /// The answer of each row of `answers`.
    ids: Vec<u32>,
    answers: gtk::ListBox,
    people: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    shown: RefCell<Shown>,
    /// Counts the answers chosen, so a page asked for under another doesn't land in this one.
    round: Cell<u64>,
}

#[derive(Default)]
struct Shown {
    answer: u32,
    /// Who voted for it, as far as we have them.
    users: Vec<User>,
    /// How many voted for it, as the poll says.
    count: u32,
    loading: bool,
}

impl App {
    /// Open the list of who voted on `message`'s poll, at `answer`.
    pub(super) fn view_poll_votes(self: &Rc<Self>, message: &Message, answer: u32) {
        let Some(poll) = message.poll.as_ref().filter(|p| !p.answers.is_empty()) else { return };
        let ids: Vec<u32> = poll.answers.iter().map(|a| a.answer_id).collect();
        let answers = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        for answer in &poll.answers {
            let line =
                gtk::Box::builder().spacing(6).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
            if let Some(emoji) = &answer.poll_media.emoji {
                line.append(&polls::emoji_widget(emoji, &self.images));
            }
            line.append(
                &gtk::Label::builder()
                    .label(answer.poll_media.text.as_deref().unwrap_or_default())
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            line.append(
                &gtk::Label::builder()
                    .label(format!("({})", poll.votes(answer.answer_id)))
                    .css_classes(["numeric", "dimmed"])
                    .build(),
            );
            answers.append(&line);
        }
        let people = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        let answer_scroller = gtk::ScrolledWindow::builder()
            .child(&answers)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(240)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&people)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&answer_scroller);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&scroller);
        let total = poll.total_votes();
        let heading =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(8).margin_bottom(8).build();
        heading.append(
            &gtk::Label::builder()
                .label(poll.question.text.as_deref().unwrap_or_default())
                .css_classes(["heading"])
                .wrap(true)
                .build(),
        );
        heading.append(
            &gtk::Label::builder()
                .label(match total {
                    1 => "1 vote".to_owned(),
                    n => format!("{n} votes"),
                })
                .css_classes(["caption", "dimmed"])
                .build(),
        );
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&heading));
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body));
        let dialog = adw::Dialog::builder()
            .title(poll.question.text.as_deref().unwrap_or("Poll"))
            .content_width(560)
            .content_height(440)
            .child(&toolbar)
            .build();
        let view = Rc::new(View {
            dialog,
            channel: message.channel_id,
            message: message.id,
            ids,
            answers,
            people,
            scroller,
            shown: RefCell::default(),
            round: Cell::new(0),
        });
        view.answers.connect_row_selected({
            let (app, view) = (Rc::downgrade(self), Rc::downgrade(&view));
            move |_, row| {
                let (Some(app), Some(view), Some(row)) = (app.upgrade(), view.upgrade(), row) else { return };
                if let Some(&answer) = usize::try_from(row.index()).ok().and_then(|i| view.ids.get(i)) {
                    view.choose(&app, answer);
                }
            }
        });
        view.scroller.vadjustment().connect_value_changed({
            let (app, view) = (Rc::downgrade(self), Rc::downgrade(&view));
            move |adjustment| {
                let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                if adjustment.value() + adjustment.page_size() >= adjustment.upper() - NEAR_END {
                    view.more(&app);
                }
            }
        });
        view.dialog.present(Some(&self.window));
        let start = view.ids.iter().position(|&id| id == answer).unwrap_or(0);
        if let Some(row) = view.answers.row_at_index(start as i32) {
            view.answers.select_row(Some(&row));
        }
    }
}

impl View {
    /// An answer was chosen: its people are asked for from the start.
    fn choose(self: &Rc<Self>, app: &Rc<App>, answer: u32) {
        let count = app
            .message_in(self.channel, self.message)
            .and_then(|m| m.poll.as_ref().map(|p| p.votes(answer)))
            .unwrap_or_default();
        self.round.set(self.round.get() + 1);
        *self.shown.borrow_mut() = Shown { answer, count, ..Shown::default() };
        self.fill(app);
        self.more(app);
    }

    /// Ask for the next page, unless one is on its way or all are here.
    fn more(self: &Rc<Self>, app: &Rc<App>) {
        let (answer, after, round) = {
            let mut shown = self.shown.borrow_mut();
            if shown.count == 0
                || shown.loading
                || (!shown.users.is_empty() && shown.count as usize <= shown.users.len())
            {
                return;
            }
            shown.loading = true;
            (shown.answer, shown.users.last().map(|u| u.id), self.round.get())
        };
        let Some(rest) = app.account.state.borrow().rest.clone() else { return };
        let (view, app) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let page = rest.poll_voters(view.channel, view.message, answer, PAGE, after).await;
            if view.round.get() != round {
                return;
            }
            {
                let mut shown = view.shown.borrow_mut();
                shown.loading = false;
                match page {
                    // Nothing came back: there is no more, however many are said to be.
                    Ok(users) if users.is_empty() => shown.count = shown.users.len() as u32,
                    Ok(users) => shown.users.extend(users),
                    Err(e) => {
                        eprintln!("poll votes: {e}");
                        return;
                    }
                }
            }
            view.fill(&app);
        });
    }

    /// The people of the chosen answer, as far as we have them.
    fn fill(&self, app: &Rc<App>) {
        while let Some(row) = self.people.first_child() {
            self.people.remove(&row);
        }
        let shown = self.shown.borrow();
        if shown.count == 0 && !shown.loading {
            let empty = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .valign(gtk::Align::Center)
                .margin_top(48)
                .build();
            empty.append(&gtk::Label::builder().label("Nothing to see here").css_classes(["heading"]).build());
            empty.append(
                &gtk::Label::builder().label("There are no votes for this answer").css_classes(["dimmed"]).build(),
            );
            self.people.append(&empty);
            return;
        }
        for user in &shown.users {
            let line =
                gtk::Box::builder().spacing(10).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
            let name = {
                let state = app.account.state.borrow();
                let server = state.avatar_in(self.channel, user.id);
                line.append(&avatar_widget_in(user, 32, Some(&app.images), server, Row::None));
                state.name_in(self.channel, user)
            };
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
            self.people.append(&line);
        }
        if shown.loading {
            self.people.append(&gtk::Spinner::builder().spinning(true).margin_top(12).margin_bottom(12).build());
        }
    }
}
