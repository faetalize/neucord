//! The age gate: opening an age-restricted channel, or one of an
//! age-restricted server, shows a page asking before it shows the channel,
//! which isn't loaded meanwhile. What decides and what it says is
//! [`discord::age_gate`]; Continue is kept for the server, as the client keeps
//! it for the browser, and Go Back takes us to where we were.

use std::rc::Rc;

use adw::prelude::*;
use discord::Snowflake;
use discord::age_gate::{self, Kind, Screen, Standing};
use gtk::pango;

use super::App;
use super::state::State;
use super::widgets::Title;

/// Where what we agreed to is kept: a server's id, and 1.
const AGREED: &str = "NsfwAgree";

/// The page.
pub(super) struct GatePage {
    pub(super) root: adw::ToolbarView,
    title: Title,
}

impl GatePage {
    pub(super) fn new() -> Self {
        GatePage { root: adw::ToolbarView::new(), title: Title::new("") }
    }
}

impl State {
    /// The gate in front of channel `id`, if there is one: its server and
    /// what it says (see [`age_gate::gated`]). A thread is as age-restricted
    /// as its parent, and a DM is never.
    pub(super) fn age_gate(&self, id: Snowflake, agreed: &[Snowflake]) -> Option<(Snowflake, Screen)> {
        let (guild, channel) = self.find_channel(id)?;
        let guild = guild?;
        let me = self.account.as_ref()?;
        let nsfw = channel.nsfw
            || channel.kind.is_thread() && channel.parent_id.and_then(|p| guild.channel(p)).is_some_and(|p| p.nsfw);
        let restricted = guild.age_restricted(self.limits().servers_by_nsfw_level());
        let standing = Standing::of(me, &self.gates);
        age_gate::gated(standing, agreed.contains(&guild.id), nsfw, restricted)
            .then(|| (guild.id, age_gate::screen(standing, restricted)))
    }
}

impl App {
    pub(super) fn build_age_gate(&self) -> gtk::Widget {
        let page = &self.age_gate;
        page.root.add_top_bar(&page.title.header_bar(&[self.sidebar_toggle().upcast_ref()]));
        page.root.clone().upcast()
    }

    /// The servers we agreed to go into.
    fn age_agreed(&self) -> Vec<Snowflake> {
        age_gate::agreed_from(&self.remembered.numbers(AGREED))
    }

    /// The gate for `channel`, if it has one.
    pub(super) fn age_gate_for(&self, channel: Snowflake) -> Option<(Snowflake, Screen)> {
        self.account.state.borrow().age_gate(channel, &self.age_agreed())
    }

    /// Show the gate for `channel` where the channel would be.
    pub(super) fn show_age_gate(self: &Rc<Self>, channel: Snowflake, guild: Snowflake, screen: &Screen, title: &str) {
        self.age_gate.title.set(title, None);
        let label = |text: &str, classes: &[&str]| {
            gtk::Label::builder()
                .label(text)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .css_classes(classes)
                .build()
        };
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        column.append(&gtk::Image::builder().icon_name("dialog-warning-symbolic").pixel_size(48).build());
        column.append(&label(screen.title, &["title-3"]));
        let description = label("", &["dimmed"]);
        description.set_use_markup(true);
        description.set_markup(&screen.description);
        column.append(&description);
        let buttons = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).margin_top(6).build();
        let back = gtk::Button::builder().label(age_gate::DISAGREEMENT).build();
        buttons.append(&back);
        back.connect_clicked({
            let this = self.clone();
            move |_| this.age_gate_back(channel, guild)
        });
        // Confirming an age is for a verification Neucord has none of.
        if let (Some(text), Kind::Verified) = (screen.agreement, screen.kind) {
            let go = gtk::Button::builder().label(text).css_classes(["suggested-action"]).build();
            go.connect_clicked({
                let this = self.clone();
                move |_| this.age_gate_agree(channel, guild)
            });
            buttons.append(&go);
        }
        column.append(&buttons);
        let clamp = adw::Clamp::builder().maximum_size(480).child(&column).margin_start(12).margin_end(12).build();
        self.age_gate.root.set_content(Some(&clamp));
        self.content.set_visible_child(&self.age_gate.root);
    }

    /// Continue: agree to go into `guild` for good, and open `channel`.
    fn age_gate_agree(self: &Rc<Self>, channel: Snowflake, guild: Snowflake) {
        let mut agreed = self.age_agreed();
        if !agreed.contains(&guild) {
            agreed.push(guild);
        }
        if let Err(e) = self.remembered.set_numbers(AGREED, &age_gate::agreed_entries(&agreed)) {
            eprintln!("could not save what we agreed to: {e}");
        }
        self.select_channel(channel);
    }

    /// Go Back (`nsfwReturnToSafety`): to the last place we were that isn't
    /// gated, else the server's default channel if that isn't, else Home.
    fn age_gate_back(self: &Rc<Self>, leaving: Snowflake, guild: Snowflake) {
        let agreed = self.age_agreed();
        let target = {
            let state = self.account.state.borrow();
            let open = |c: Snowflake| state.find_channel(c).is_some() && state.age_gate(c, &agreed).is_none();
            age_gate::safety(&state.quick_history, leaving, state.default_channel(guild), &open)
        };
        match target {
            Some(channel) => self.open_channel(channel),
            None => self.select_home(),
        }
    }
}
