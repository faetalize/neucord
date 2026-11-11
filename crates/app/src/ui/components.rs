//! Message components. Link buttons open their link; the other buttons and
//! the menus of options send an interaction, as the official client's do
//! (964363, 911408, 232430), and show it is on its way until Discord
//! answers. Menus of users, roles and channels, and buttons that buy
//! something, are shown but can't be used.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use discord::component::{Button, ButtonStyle, Component, Media, SelectOption};
use discord::interaction::{self, Failure, Local, Press, Visual};
use discord::permissions;
use discord::{Message, Snowflake};
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use super::App;
use super::embeds::{behind_spoiler, colour_bar, fitted_picture};
use super::polls::emoji_widget;
use super::state::now_unix;
use crate::images::Images;
use crate::render::{self, Context};
use crate::timelines;

/// Media in a gallery, or alone, and thumbnails beside text (the official
/// client's sizes for embeds, which these follow).
const MAX_W: f64 = 400.0;
const MAX_H: f64 = 300.0;
const THUMBNAIL: f64 = 80.0;
/// What a button or menu that can't be used says.
const NOT_YET: &str = "Neucord can't use this yet";

/// What lets a message's buttons and menus act: the message they belong to,
/// what is on its way, and whether we may use them here. A forwarded
/// message's have none.
pub(super) struct Live {
    app: Weak<App>,
    channel: Snowflake,
    message: Snowflake,
    local: Local,
    blocked: bool,
}

impl Live {
    pub(super) fn new(app: &Rc<App>, message: &Message) -> Live {
        let blocked = {
            let state = app.state.borrow();
            match (state.find_channel(message.channel_id), &state.account) {
                (Some((guild, channel)), Some(me)) => permissions::components_blocked(guild, channel, me, now_unix()),
                _ => false,
            }
        };
        Live {
            app: Rc::downgrade(app),
            channel: message.channel_id,
            message: message.id,
            local: message.local.clone(),
            blocked,
        }
    }

    /// Whether `component` can be used now: not turned off by its app, by
    /// another interaction waiting, or by where we are (`isDisabled` and
    /// `visualState` in 911408).
    fn usable(&self, component: &Component, disabled: bool) -> bool {
        !disabled && self.local.visual(component) != Visual::Disabled && !self.blocked
    }

    /// Press or pick: send the interaction of `custom_id`.
    fn act(&self, custom_id: &str, component_type: u8, values: Option<Vec<String>>) {
        if let Some(app) = self.app.upgrade() {
            app.interact(self.channel, self.message, custom_id.to_owned(), component_type, values);
        }
    }

    fn say(&self, text: &str) {
        if let Some(app) = self.app.upgrade() {
            app.show_composer_error(text);
        }
    }
}

/// The components of a message, one under another.
pub(super) fn components(components: &[Component], cx: &Context, live: Option<&Live>) -> gtk::Widget {
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
    for component in components {
        if let Some(widget) = component_widget(component, cx, live) {
            column.append(&widget);
        }
    }
    column.upcast()
}

fn component_widget(component: &Component, cx: &Context, live: Option<&Live>) -> Option<gtk::Widget> {
    let images = cx.images;
    Some(match component {
        Component::Row(children) => {
            let row = gtk::Box::builder().spacing(8).halign(gtk::Align::Start).build();
            for child in children {
                if let Some(widget) = component_widget(child, cx, live) {
                    row.append(&widget);
                }
            }
            row.upcast()
        }
        Component::Button(button) => button_widget(button, component, images, live),
        Component::Select { custom_id, placeholder, options, disabled, min_values, max_values, of_options } => {
            let menu = Menu {
                custom_id: custom_id.as_deref().filter(|_| *of_options),
                placeholder: placeholder.as_deref(),
                options,
                disabled: *disabled,
                min: *min_values,
                max: *max_values,
            };
            menu_widget(&menu, component, images, live)
        }
        Component::Section { components: children, accessory } => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let text = components(children, cx, live);
            text.set_hexpand(true);
            row.append(&text);
            if let Some(widget) = accessory.as_deref().and_then(|a| component_widget(a, cx, live)) {
                widget.set_valign(gtk::Align::Start);
                row.append(&widget);
            }
            row.upcast()
        }
        Component::Text(content) => render::render(&discord::markup::parse(content), cx, false),
        Component::Thumbnail(media) => picture(media, images?, THUMBNAIL, THUMBNAIL),
        Component::Gallery(items) => {
            // One item large; more, two to a row.
            let grid = gtk::Grid::builder().column_spacing(4).row_spacing(4).halign(gtk::Align::Start).build();
            let (w, h) = if items.len() == 1 { (MAX_W, MAX_H) } else { (MAX_W / 2.0 - 2.0, MAX_H / 2.0 - 2.0) };
            for (n, media) in items.iter().enumerate() {
                grid.attach(&picture(media, images?, w, h), n as i32 % 2, n as i32 / 2, 1, 1);
            }
            grid.upcast()
        }
        Component::File { media, name, size } => {
            let name = name.as_deref().unwrap_or_else(|| media.url.rsplit('/').next().unwrap_or_default());
            let mut markup = format!(
                "<a href=\"{}\">📎 {}</a>",
                glib::markup_escape_text(&media.url),
                glib::markup_escape_text(name)
            );
            if let Some(size) = size.filter(|&s| s > 0) {
                markup += &format!("  <span fgalpha=\"55%\" size=\"small\">{}</span>", glib::format_size(size));
            }
            gtk::Label::builder().use_markup(true).label(markup).xalign(0.0).build().upcast()
        }
        Component::Separator { divider, large } => {
            let margin = if *large { 8 } else { 2 };
            let separator = gtk::Separator::builder().margin_top(margin).margin_bottom(margin).build();
            separator.set_opacity(if *divider { 1.0 } else { 0.0 });
            separator.upcast()
        }
        Component::Container { accent, spoiler, components: children } => {
            // Drawn as an embed is, which it stands in for.
            let card = gtk::Box::builder()
                .halign(gtk::Align::Start)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["embed"])
                .build();
            if let Some(accent) = accent {
                card.append(&colour_bar(*accent));
            }
            let body = components(children, cx, live);
            body.add_css_class("embed-body");
            card.append(&body);
            if *spoiler {
                let card = card.upcast::<gtk::Widget>();
                return Some(behind_spoiler(move || card.clone()));
            }
            card.upcast()
        }
        Component::Unknown => return None,
    })
}

fn button_widget(button: &Button, component: &Component, images: Option<&Images>, live: Option<&Live>) -> gtk::Widget {
    let content = gtk::Box::builder().spacing(6).halign(gtk::Align::Center).build();
    if let (Some(emoji), Some(images)) = (&button.emoji, images) {
        content.append(&emoji_widget(emoji, images));
    }
    if let Some(label) = &button.label {
        content.append(&gtk::Label::builder().label(label).ellipsize(pango::EllipsizeMode::End).build());
    }
    let widget = gtk::Button::builder().child(&content).build();
    // libadwaita's own kinds of button, where it has one.
    match button.style {
        ButtonStyle::Primary => widget.add_css_class("suggested-action"),
        ButtonStyle::Destructive => widget.add_css_class("destructive-action"),
        _ => {}
    }
    let link = button.url.as_deref().filter(|url| button.style == ButtonStyle::Link && url.starts_with("https://"));
    match (link, button.custom_id.as_deref(), button.style) {
        (Some(url), _, _) if !button.disabled => {
            widget.set_tooltip_text(Some(url));
            let url = url.to_owned();
            widget.connect_clicked(move |widget| {
                let window = widget.root().and_downcast::<gtk::Window>();
                gtk::UriLauncher::new(&url).launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            });
        }
        // Buying needs Discord's store, which Neucord doesn't have.
        (None, _, ButtonStyle::Premium) => {
            widget.set_sensitive(false);
            widget.set_tooltip_text(Some(NOT_YET));
        }
        (None, Some(id), style) if style != ButtonStyle::Link => match live {
            Some(live) => {
                widget.set_sensitive(live.usable(component, button.disabled));
                if live.local.visual(component) == Visual::Loading {
                    // The label stays to keep the button's size, under the spinner.
                    content.set_opacity(0.0);
                    widget.set_child(gtk::Widget::NONE);
                    let overlay = gtk::Overlay::new();
                    overlay.set_child(Some(&content));
                    overlay.add_overlay(&gtk::Spinner::builder().spinning(true).halign(gtk::Align::Center).build());
                    widget.set_child(Some(&overlay));
                }
                let (live, id) = (live.handle(), id.to_owned());
                widget.connect_clicked(move |_| live.act(&id, interaction::BUTTON, None));
            }
            None => widget.set_sensitive(false),
        },
        _ => {
            widget.set_sensitive(false);
            if !button.disabled {
                widget.set_tooltip_text(Some(NOT_YET));
            }
        }
    }
    widget.upcast()
}

/// A menu's parts, as its component gave them.
struct Menu<'a> {
    /// `None` for a menu of users, roles or channels.
    custom_id: Option<&'a str>,
    placeholder: Option<&'a str>,
    options: &'a [SelectOption],
    disabled: bool,
    min: usize,
    max: usize,
}

/// What a menu's face says, which outlives the component it came from.
struct MenuText {
    placeholder: Option<String>,
    options: Vec<SelectOption>,
}

impl MenuText {
    fn of(menu: &Menu) -> Self {
        MenuText { placeholder: menu.placeholder.map(str::to_owned), options: menu.options.to_vec() }
    }

    /// The options picked, or what to do.
    fn label(&self, picked: &[String]) -> String {
        let names: Vec<&str> =
            self.options.iter().filter(|o| picked.contains(&o.value)).map(|o| o.label.as_str()).collect();
        match (names.is_empty(), &self.placeholder) {
            (false, _) => names.join(", "),
            (true, Some(placeholder)) => placeholder.clone(),
            (true, None) => "Make a selection".to_owned(),
        }
    }
}

/// A menu of options: the button shows what is picked, and a popover lists
/// them. One to pick sends when it is chosen; several, when the list closes
/// with something changed (`nt` in 232430).
fn menu_widget(menu: &Menu, component: &Component, images: Option<&Images>, live: Option<&Live>) -> gtk::Widget {
    // What it holds before anything is picked: the message's own defaults,
    // or what a press left.
    let defaults: Vec<String> = menu.options.iter().filter(|o| o.default).map(|o| o.value.clone()).collect();
    let held = live.zip(menu.custom_id).and_then(|(live, id)| live.local.picked(id)).map(<[String]>::to_vec);
    let picked = Rc::new(RefCell::new(held.unwrap_or(defaults)));
    let loading = live.is_some_and(|l| l.local.visual(component) == Visual::Loading);

    let label = gtk::Label::builder()
        .label(MenuText::of(menu).label(&picked.borrow()))
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    let face = gtk::Box::builder().spacing(6).build();
    face.append(&label);
    if loading {
        face.append(&gtk::Spinner::builder().spinning(true).build());
    }
    let button = gtk::MenuButton::builder().child(&face).build();
    let (Some(live), Some(id)) = (live, menu.custom_id) else {
        button.set_sensitive(false);
        button.set_tooltip_text(Some(NOT_YET));
        return button.upcast();
    };
    button.set_sensitive(live.usable(component, menu.disabled) && !loading);

    let sent = Rc::new(RefCell::new(picked.borrow().clone()));
    let single = menu.max <= 1;
    let (live, id) = (Rc::new(live.handle()), id.to_owned());
    let (min, max) = (menu.min, menu.max);
    let popover = gtk::Popover::new();
    let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&list)
        .propagate_natural_height(true)
        .max_content_height(300)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    popover.set_child(Some(&scroller));
    button.set_popover(Some(&popover));

    // Send what is picked, if it differs from what was last sent and fits.
    let commit = {
        let (picked, sent, live, id) = (picked.clone(), sent.clone(), live.clone(), id.clone());
        move || {
            let now = picked.borrow().clone();
            if now == *sent.borrow() {
                return;
            }
            if let Some(error) = interaction::pick_error(now.len(), min, max) {
                return live.say(&error);
            }
            *sent.borrow_mut() = now.clone();
            live.act(&id, interaction::STRING_SELECT, Some(now));
        }
    };

    let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> = Rc::default();
    for option in menu.options {
        let row = gtk::Box::builder().spacing(8).build();
        if let (Some(emoji), Some(images)) = (&option.emoji, images) {
            row.append(&emoji_widget(emoji, images));
        }
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        text.append(&gtk::Label::builder().label(&option.label).xalign(0.0).wrap(true).build());
        if let Some(description) = &option.description {
            text.append(
                &gtk::Label::builder()
                    .label(description)
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["caption", "dimmed"])
                    .build(),
            );
        }
        row.append(&text);
        let value = option.value.clone();
        if single {
            if picked.borrow().contains(&value) {
                row.append(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
            let choice = gtk::Button::builder().child(&row).css_classes(["flat"]).build();
            let (picked, popover) = (picked.clone(), popover.clone());
            choice.connect_clicked(move |_| {
                *picked.borrow_mut() = vec![value.clone()];
                popover.popdown();
            });
            list.append(&choice);
        } else {
            let check = gtk::CheckButton::builder().child(&row).active(picked.borrow().contains(&value)).build();
            checks.borrow_mut().push((value.clone(), check.clone()));
            let (picked, checks) = (picked.clone(), checks.clone());
            check.connect_toggled(move |check| {
                {
                    let mut picked = picked.borrow_mut();
                    picked.retain(|v| *v != value);
                    if check.is_active() {
                        picked.push(value.clone());
                    }
                }
                // At the limit the rest wait for one to be put back.
                let full = picked.borrow().len() >= max;
                for (v, other) in checks.borrow().iter() {
                    other.set_sensitive(!full || picked.borrow().contains(v));
                }
            });
            list.append(&check);
        }
    }
    if !single {
        let full = picked.borrow().len() >= max;
        for (v, check) in checks.borrow().iter() {
            check.set_sensitive(!full || picked.borrow().contains(v));
        }
    }
    // What was picked goes when the list has closed: for one option that
    // is when it is chosen, for several when it is dismissed. Sending then
    // leaves the popover whole when the row is drawn again.
    let (face_label, menu_copy) = (label.clone(), MenuText::of(menu));
    popover.connect_closed(move |_| {
        face_label.set_label(&menu_copy.label(&picked.borrow()));
        commit();
    });
    button.upcast()
}

impl Live {
    /// What a widget's handlers keep: no widgets, so no cycles.
    fn handle(&self) -> Live {
        Live {
            app: self.app.clone(),
            channel: self.channel,
            message: self.message,
            local: Local::default(),
            blocked: self.blocked,
        }
    }
}

impl App {
    /// Send the interaction of a button pressed or a menu picked on
    /// `message`, and show it pending (`sendComponentInteraction` in 964363).
    /// Only one goes at a time per message; a thread is reopened first, and
    /// if that fails nothing is sent.
    pub(super) fn interact(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        custom_id: String,
        component_type: u8,
        values: Option<Vec<String>>,
    ) {
        let Some(store) = self.timelines.borrow().get(channel).map(|t| t.store.clone()) else { return };
        let Some((_, held)) = timelines::find(&store, message) else { return };
        if !held.local.can_queue() {
            return;
        }
        let (rest, session, guild) = {
            let state = self.state.borrow();
            let guild = state.find_channel(channel).and_then(|(g, _)| g.map(|g| g.id));
            (state.rest.clone(), state.session_id.clone(), guild)
        };
        let Some(rest) = rest else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            if this.reopen(&rest, channel).await.is_err() {
                return;
            }
            let nonce = Snowflake::now();
            // Another press may have gone while the thread was reopening.
            let mut free = false;
            let at = timelines::edit(&store, message, |m| {
                free = m.local.can_queue();
                if free {
                    m.local.queue(nonce, &custom_id, values.as_deref());
                }
            });
            let Some(at) = at.filter(|_| free) else { return };
            this.state.borrow_mut().interactions.insert(nonce, (channel, message));
            this.rebind(&store, at, 1);
            let sent = Press {
                guild,
                channel,
                message,
                message_flags: held.flags,
                application: held.application_id.unwrap_or(held.author.id),
                session: &session,
                component_type,
                custom_id: &custom_id,
                values: values.as_deref(),
            };
            if let Err(failure) = rest.interact(&sent, nonce).await {
                this.interaction_ended(nonce, false, Some(failure));
            }
        });
    }

    /// Discord answered the interaction of ours sent as `nonce`.
    pub(super) fn on_interaction(self: &Rc<Self>, nonce: Snowflake, outcome: discord::gateway::Outcome) {
        use discord::gateway::Outcome;
        self.interaction_ended(nonce, outcome != Outcome::Failure, None);
        if outcome == Outcome::Form {
            // The client opens it.
            self.show_composer_error("This app wants a form filled in, which Neucord can't show yet.");
        }
    }

    /// The interaction is over: answered, or `answered` false, refused. Ours
    /// that the request itself failed (`failure`) is told, when the client does.
    fn interaction_ended(self: &Rc<Self>, nonce: Snowflake, answered: bool, failure: Option<Failure>) {
        let Some((channel, message)) = self.state.borrow_mut().interactions.remove(&nonce) else { return };
        let Some(store) = self.timelines.borrow().get(channel).map(|t| t.store.clone()) else { return };
        let at = timelines::edit(&store, message, |m| {
            _ = if answered { m.local.answered(nonce) } else { m.local.refused(nonce) };
        });
        if let Some(at) = at {
            self.rebind(&store, at, 1);
        }
        if failure.is_some_and(|f| f.is_announced()) {
            self.show_composer_error("Your message could not be delivered.");
        }
    }
}

/// An image a component shows, fitted into `max_w` × `max_h`, behind a
/// cover if it is a spoiler.
fn picture(media: &Media, images: &Images, max_w: f64, max_h: f64) -> gtk::Widget {
    let build = {
        let (media, images) = (media.clone(), images.clone());
        move || {
            let fitted = fitted_picture(media.width, media.height, max_w, max_h);
            if let Some(description) = &media.description {
                fitted.picture.set_alternative_text(Some(description));
                fitted.picture.set_tooltip_text(Some(description));
            }
            let original = media.width.zip(media.height);
            images.set_sized(&fitted.picture, &media.url, fitted.size, original, false);
            fitted.frame.upcast()
        }
    };
    if media.spoiler { behind_spoiler(build) } else { build() }
}
