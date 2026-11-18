//! Forward: the box that asks where a message goes, and what sends it. The
//! official client opens it from a message's menu and from the media viewer
//! (for one picture or file); what it lists, what greys a place out and
//! what is sent are [`discord::forward`]'s.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::forward::{self, Destination, MAX_DESTINATIONS, Only, Target};
use discord::switcher::{Hit, Kind as HitKind};
use discord::{Channel, Message, Snowflake};
use gtk::{glib, pango};

use super::App;
use super::chat::MessageTarget;
use super::menus::{Entry, Item, Kind};
use super::state::{State, now_ms, now_unix};

/// Forward in a message's menu, on a message that can be (`canForward`).
pub(super) const MESSAGE_MENU: Entry<MessageTarget> = Entry::Item(Item {
    name: "forward",
    label: "_Forward",
    shows: |app, target| app.account.state.borrow().can_forward(target.shown()),
    kind: Kind::Action(|app, target| app.open_forward(target.shown().clone(), Only::default(), Vec::new(), None)),
});

impl State {
    /// Whether Forward is offered on `message` (see [`forward::can_forward`]).
    pub(super) fn can_forward(&self, message: &Message) -> bool {
        match (self.find_channel(message.channel_id), &self.account) {
            (Some((guild, channel)), Some(me)) => forward::can_forward(guild, channel, message, me, now_unix()),
            _ => false,
        }
    }

    /// Whether `hit` is somewhere a message goes (see [`forward::user_can_receive`]
    /// and [`forward::channel_can_receive`]).
    fn can_receive(&self, hit: &Hit) -> bool {
        let Some(me) = self.account.as_ref() else { return false };
        match hit.kind {
            HitKind::User => {
                let user = self
                    .relationships
                    .get(hit.id)
                    .map(|r| &r.user)
                    .or_else(|| self.private_channels.iter().flat_map(|c| &c.recipients).find(|u| u.id == hit.id));
                user.is_none_or(forward::user_can_receive)
            }
            HitKind::GroupDm | HitKind::Channel | HitKind::Voice => self
                .find_channel(hit.id)
                .is_some_and(|(guild, channel)| forward::channel_can_receive(guild, channel, me, now_unix())),
            HitKind::Server => false,
        }
    }
}

/// The box.
struct Picker {
    dialog: adw::Dialog,
    entry: gtk::Entry,
    list: gtk::ListBox,
    none: gtk::Label,
    subtitle: gtk::Label,
    comment: gtk::Entry,
    slowmode: gtk::Label,
    send: gtk::Button,
    message: Message,
    only: Only,
    /// Where it goes, the last picked first.
    selected: RefCell<Vec<Destination>>,
    /// What each row of `list` is.
    shown: RefCell<Vec<Hit>>,
    /// Runs once the forward is on its way.
    sent: Option<Rc<dyn Fn()>>,
    sending: Cell<bool>,
}

impl Picker {
    fn new(
        app: &Rc<App>,
        message: Message,
        only: Only,
        selected: Vec<Destination>,
        sent: Option<Rc<dyn Fn()>>,
    ) -> Rc<Self> {
        let entry = gtk::Entry::builder()
            .placeholder_text("Search")
            .accessible_role(gtk::AccessibleRole::SearchBox)
            .primary_icon_name("system-search-symbolic")
            .build();
        let subtitle = gtk::Label::builder().xalign(0.0).css_classes(["dimmed"]).wrap(true).build();
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        let scroller =
            gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        let none = gtk::Label::builder()
            .label("We searched far and wide. Unfortunately, no results were found.")
            .css_classes(["dimmed"])
            .wrap(true)
            .margin_top(24)
            .margin_bottom(24)
            .visible(false)
            .build();
        let comment = gtk::Entry::builder().placeholder_text("Add an optional message...").hexpand(true).build();
        let send = gtk::Button::builder().css_classes(["suggested-action"]).build();
        let slowmode =
            gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "warning"]).visible(false).build();

        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&subtitle);
        content.append(&entry);
        content.append(&scroller);
        content.append(&none);
        content.append(&preview(&message, &only));
        content.append(&slowmode);
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&comment);
        line.append(&send);
        content.append(&line);
        let dialog = adw::Dialog::builder()
            .title("Forward To")
            .content_width(480)
            .content_height(620)
            .focus_widget(&entry)
            .child(&content)
            .build();
        let picker = Rc::new(Picker {
            dialog,
            entry,
            list,
            none,
            subtitle,
            comment,
            slowmode,
            send,
            message,
            only,
            selected: RefCell::new(selected),
            shown: RefCell::default(),
            sent,
            sending: Cell::new(false),
        });

        let (weak_app, weak) = (Rc::downgrade(app), Rc::downgrade(&picker));
        let with = move |f: fn(&Rc<Picker>, &Rc<App>)| {
            let (weak_app, weak) = (weak_app.clone(), weak.clone());
            move || {
                if let (Some(app), Some(picker)) = (weak_app.upgrade(), weak.upgrade()) {
                    f(&picker, &app);
                }
            }
        };
        picker.entry.connect_changed({
            let run = with(|p, app| p.refresh(app));
            move |_| run()
        });
        picker.comment.connect_changed({
            let run = with(|p, app| p.update(app));
            move |_| run()
        });
        picker.comment.connect_activate({
            let run = with(Picker::submit);
            move |_| run()
        });
        picker.send.connect_clicked({
            let run = with(Picker::submit);
            move |_| run()
        });
        picker.list.connect_row_activated({
            let (weak_app, weak) = (Rc::downgrade(app), Rc::downgrade(&picker));
            move |_, row| {
                let (Some(app), Some(picker)) = (weak_app.upgrade(), weak.upgrade()) else { return };
                if let Ok(i) = usize::try_from(row.index()) {
                    picker.toggle(&app, i);
                }
            }
        });
        picker
    }

    /// List again for what is typed, and show what is picked.
    fn refresh(self: &Rc<Self>, app: &Rc<App>) {
        let typed = self.entry.text();
        let selected = self.selected.borrow().clone();
        let history = app.account.state.borrow().quick_history.clone();
        let frequent: Vec<Snowflake> = app
            .account
            .state
            .borrow_mut()
            .frecency
            .stores
            .channels
            .history
            .frequently(now_ms() as u64, |key| key.parse::<u64>().is_ok())
            .iter()
            .filter_map(|key| key.parse().ok().map(Snowflake))
            .collect();
        let origin = app
            .account
            .state
            .borrow()
            .find_channel(self.message.channel_id)
            .map_or(Destination::Channel(self.message.channel_id), |(_, c)| Destination::of_channel(c));
        let hits = app
            .with_sources(|sources, state| {
                let eligible = |hit: &Hit| state.can_receive(hit);
                forward::listing(sources, &typed, &selected, &history, &frequent, origin, &eligible)
            })
            .unwrap_or_default();
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let full = selected.len() >= MAX_DESTINATIONS;
        for hit in &hits {
            let picked = selected.contains(&Destination::of_hit(hit));
            self.list.append(&self.row(app, hit, picked, full));
        }
        self.none.set_visible(hits.is_empty());
        *self.shown.borrow_mut() = hits;
        self.update(app);
    }

    /// A result's row: the switcher's, with why it is greyed out and a tick.
    fn row(&self, app: &Rc<App>, hit: &Hit, picked: bool, full: bool) -> gtk::ListBoxRow {
        let row = app.switcher_row(hit);
        let reason = self.reason(app, hit);
        let disabled = reason.is_some() || (full && !picked);
        if let Some(line) = row.child().and_downcast::<gtk::Box>() {
            if let Some(reason) = reason {
                line.append(
                    &gtk::Label::builder()
                        .label(reason)
                        .css_classes(["caption", "dimmed"])
                        .wrap(true)
                        .wrap_mode(pango::WrapMode::WordChar)
                        .max_width_chars(28)
                        .build(),
                );
            }
            let tick = gtk::CheckButton::builder().active(picked).sensitive(false).valign(gtk::Align::Center).build();
            line.append(&tick);
        }
        row.set_sensitive(!disabled);
        row
    }

    /// Why `hit` takes this message in no way (`getDestinationIsUnavailable`).
    fn reason(&self, app: &Rc<App>, hit: &Hit) -> Option<&'static str> {
        let state = app.account.state.borrow();
        let me = state.account.as_ref()?;
        let (_, origin) = state.find_channel(self.message.channel_id)?;
        let target = match hit.kind {
            HitKind::User => Target::User,
            _ => {
                let (guild, channel) = state.find_channel(hit.id)?;
                Target::Channel(guild, channel)
            }
        };
        forward::unavailable(&self.message, origin, target, me, now_unix())
    }

    /// Pick or put back the place at row `index`. Picking one clears the
    /// search, so the places picked lead the list.
    fn toggle(self: &Rc<Self>, app: &Rc<App>, index: usize) {
        let Some(hit) = self.shown.borrow().get(index).cloned() else { return };
        let destination = Destination::of_hit(&hit);
        {
            let mut selected = self.selected.borrow_mut();
            match selected.iter().position(|d| *d == destination) {
                Some(at) => {
                    selected.remove(at);
                }
                None if selected.len() < MAX_DESTINATIONS => selected.insert(0, destination),
                None => return,
            }
        }
        if self.entry.text().is_empty() {
            self.refresh(app);
        } else {
            // Changing the text lists again.
            self.entry.set_text("");
        }
    }

    /// The button, the line under the title and the slowmode warning.
    fn update(&self, app: &Rc<App>) {
        let selected = self.selected.borrow();
        let full = selected.len() >= MAX_DESTINATIONS;
        self.subtitle.set_label(&if full {
            format!("Maximum {MAX_DESTINATIONS} places at once.")
        } else {
            "Select where you want to share this message.".to_owned()
        });
        self.send.set_label(&match selected.len() {
            0 | 1 => "Send".to_owned(),
            n => format!("Send ({n})"),
        });
        self.send.set_sensitive(!selected.is_empty() && !self.sending.get());
        // A comment goes after the forward, which slowmode would refuse.
        let slow = if self.comment.text().is_empty() { Vec::new() } else { slowmode_names(app, &selected) };
        self.slowmode.set_visible(!slow.is_empty());
        self.slowmode.set_label(&format!(
            "The message you added will not be sent to slowmode {}: {}.",
            if slow.len() == 1 { "channel" } else { "channels" },
            slow.join(", ")
        ));
    }

    fn submit(self: &Rc<Self>, app: &Rc<App>) {
        let selected = self.selected.borrow().clone();
        if selected.is_empty() || self.sending.replace(true) {
            return;
        }
        let comment = self.comment.text().to_string();
        self.dialog.force_close();
        if let Some(sent) = &self.sent {
            sent();
        }
        let switch = selected.len() == 1;
        app.forward_to(self.message.clone(), self.only.clone(), selected, comment, switch);
    }
}

/// The names of the picked channels whose slowmode we don't bypass.
fn slowmode_names(app: &App, selected: &[Destination]) -> Vec<String> {
    let Some(names) = app.with_sources(|sources, state| {
        let now = now_unix();
        selected
            .iter()
            .filter_map(|d| match d {
                Destination::Channel(id) => state.find_channel(*id),
                Destination::User(_) => None,
            })
            .filter(|(guild, channel)| slows(*guild, channel, state, now))
            .map(|(_, channel)| sources.channel_name(channel))
            .collect::<Vec<_>>()
    }) else {
        return Vec::new();
    };
    names
}

/// Whether slowmode holds back a second message to `channel` (`lP`).
fn slows(guild: Option<&discord::Guild>, channel: &Channel, state: &State, now: i64) -> bool {
    let bypasses = match (guild, state.account.as_ref()) {
        (Some(guild), Some(me)) => discord::cooldowns::bypasses_slowmode(guild, channel, me, now),
        _ => false,
    };
    channel.rate_limit_per_user > 0 && !bypasses
}

/// What goes, in the box: its text and a line for its files.
fn preview(message: &Message, only: &Only) -> gtk::Widget {
    let shown = forward::preview(message, only);
    let card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(0).css_classes(["card"]).build();
    let summary = shown.summary.unwrap_or_default();
    for (text, dim) in [(&shown.text, false), (&summary, true)] {
        if text.is_empty() {
            continue;
        }
        card.append(
            &gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .lines(3)
                .ellipsize(pango::EllipsizeMode::End)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .css_classes(if dim { vec!["dimmed"] } else { vec![] })
                .build(),
        );
    }
    card.upcast()
}

impl App {
    /// Ask where `message` goes (all of it, or `only` what of it), with
    /// `selected` picked already. `sent` runs when it is on its way.
    pub(super) fn open_forward(
        self: &Rc<Self>,
        message: Message,
        only: Only,
        selected: Vec<Destination>,
        sent: Option<Rc<dyn Fn()>>,
    ) {
        if self.account.state.borrow().me().is_none() {
            return;
        }
        let picker = Picker::new(self, message, only, selected, sent);
        picker.dialog.present(Some(&self.window));
        picker.refresh(self);
    }

    /// Forward `message` to each of `destinations`, and what was typed after
    /// it as a message of its own (`sendForwards`). With one destination we
    /// go there. A forward that fails to any is offered again.
    fn forward_to(
        self: &Rc<Self>,
        message: Message,
        only: Only,
        destinations: Vec<Destination>,
        comment: String,
        switch: bool,
    ) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let (text, flags) = forward::comment(&comment);
            let guild = {
                let state = this.account.state.borrow();
                state.find_channel(message.channel_id).and_then(|(g, _)| g.map(|g| g.id)).or(message.guild_id)
            };
            let reference = forward::reference(guild, &message, &only);
            let mut channels = Vec::new();
            let mut failed = Vec::new();
            for destination in destinations {
                let channel = match destination {
                    Destination::Channel(id) => Ok(id),
                    Destination::User(user) => this.dm_with(user).await,
                };
                match channel {
                    Ok(id) => channels.push((destination, id)),
                    Err(e) => {
                        eprintln!("forward: {e}");
                        failed.push(destination);
                    }
                }
            }
            if let (true, [(_, only_channel)]) = (switch, channels.as_slice()) {
                this.open_channel(*only_channel);
            }
            for (destination, channel) in channels {
                if let Err(e) = this.forward_one(channel, &reference, &text, flags).await {
                    eprintln!("forward: {e}");
                    failed.push(destination);
                }
            }
            if failed.is_empty() {
                this.toast("Message forwarded!");
            } else {
                this.forward_failed(message, only, failed);
            }
        });
    }

    /// The forward, then the words with it unless slowmode would refuse them.
    async fn forward_one(
        &self,
        channel: Snowflake,
        reference: &serde_json::Value,
        text: &str,
        flags: u64,
    ) -> Result<(), discord::Error> {
        let rest = self.account.state.borrow().rest.clone();
        let Some(rest) = rest else { return Err(discord::Error::Protocol("not connected".to_owned())) };
        rest.send_forward(channel, reference.clone(), flags).await?;
        let goes = {
            let state = self.account.state.borrow();
            state
                .find_channel(channel)
                .is_none_or(|(guild, c)| forward::comment_goes(text, c, !slows(guild, c, &state, now_unix())))
        };
        if goes {
            // The silent flag rides the text again, as the message is sent.
            let content = discord::scheduled::with_silent(text, flags);
            rest.send_message(channel, &content, None, &[], None, false).await?;
        }
        Ok(())
    }

    /// Say where it didn't go, and offer to ask again (`ForwardFailedAlertModal`).
    fn forward_failed(self: &Rc<Self>, message: Message, only: Only, failed: Vec<Destination>) {
        let names: Vec<String> = self
            .with_sources(|sources, _| {
                failed
                    .iter()
                    .filter_map(|d| match d {
                        Destination::User(id) => sources.user_hit(*id),
                        Destination::Channel(id) => sources.as_hit(*id),
                    })
                    .map(|hit| hit.comparator)
                    .collect()
            })
            .unwrap_or_default();
        let places = if failed.len() == 1 { "location" } else { "locations" };
        let dialog = adw::AlertDialog::new(
            Some("Forward Failed"),
            Some(&format!("The message could not be forwarded to the following {places}:\n\n{}", names.join("\n"))),
        );
        dialog.add_response("cancel", "_Cancel");
        dialog.add_response("retry", "_Retry");
        dialog.set_response_appearance("retry", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("retry"));
        dialog.set_close_response("cancel");
        let this = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "retry" {
                this.open_forward(message, only, failed, None);
            }
        });
    }
}
