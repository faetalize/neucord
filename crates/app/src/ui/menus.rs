//! Context menus. Each surface (a message, the message box) has a table
//! here of the entries it offers, in sections; a feature defines its entries
//! next to its own code and lists them in the table. Entries are actions,
//! checkboxes, submenus, or choices made from state as the menu opens. A
//! menu is built afresh each time it opens, with what shows for what was
//! clicked.
//!
//! Menus open as GTK's own do: right-click and long-press where they land,
//! Menu and Shift+F10 from the keyboard.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::App;
use super::chat::{self, MessageTarget};
use super::{
    composer, create_thread, edits, emoji, forum, gifs, picker, poll_composer, polls, profile, reactions, reactors,
    scheduling, sidebar, slash, stickers, threads, uploads, voice,
};

/// One entry of a menu, for what it was opened on: a `T`. `C` is what
/// entries read and act through: the app, or a stand-in in tests.
pub(super) enum Entry<T: 'static, C: 'static = App> {
    Item(Item<T, C>),
    Choices(Choices<T, C>),
}

pub(super) struct Item<T: 'static, C: 'static = App> {
    /// Its action's name, unique within its menu, submenus included.
    pub(super) name: &'static str,
    /// With a mnemonic.
    pub(super) label: &'static str,
    /// Whether it shows: where the official client offers it.
    pub(super) shows: fn(&C, &T) -> bool,
    pub(super) kind: Kind<T, C>,
}

pub(super) enum Kind<T: 'static, C: 'static = App> {
    /// Runs when chosen.
    Action(fn(&Rc<C>, &T)),
    /// A checkbox, ticked as `checked` says; `set` gets its new state.
    #[cfg_attr(not(test), expect(dead_code, reason = "for the entries #10 and #13 bring"))]
    Check { checked: fn(&C, &T) -> bool, set: fn(&Rc<C>, &T, bool) },
    /// More sections, opened from this item. It shows only with something
    /// in them.
    Submenu(Table<T, C>),
    /// A submenu its feature fills as the menu opens and keeps up to date
    /// while it is open, such as the commands of the Apps item. It shows
    /// whenever the item does, empty or not.
    Live(fn(&Rc<C>, &Rc<T>, Live)),
}

/// What a [`Kind::Live`] submenu is made of: its entries change through
/// `menu`, their actions are added to and removed from `actions`, and
/// `widgets` are placed where an item of `menu` has the `custom` attribute
/// with their id (a search box, say).
pub(super) struct Live {
    pub(super) menu: gio::Menu,
    pub(super) actions: gio::SimpleActionGroup,
    pub(super) widgets: Widgets,
}

/// Widgets for the menu's `custom` items, by id.
pub(super) type Widgets = Rc<RefCell<Vec<(&'static str, gtk::Widget)>>>;

/// Items made when the menu opens, one per choice: roles, server folders,
/// recent emoji, mute durations.
pub(super) struct Choices<T: 'static, C: 'static = App> {
    /// Their action's name; checkboxes add `-` and their place.
    pub(super) name: &'static str,
    pub(super) choices: fn(&C, &T) -> Vec<Choice>,
    pub(super) style: Style,
    /// Gets the chosen value, and whether it is now ticked: always, but for
    /// a checkbox being unticked.
    pub(super) choose: fn(&Rc<C>, &T, &str, bool),
}

pub(super) struct Choice {
    pub(super) value: String,
    /// As shown: names from Discord have no mnemonics.
    pub(super) label: String,
    pub(super) ticked: bool,
}

#[cfg_attr(not(test), expect(dead_code, reason = "for the entries #10 and #13 bring"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    Plain,
    /// One of them ticked, such as a notification level.
    Radio,
    /// Each ticked or not, such as roles.
    Check,
}

/// A menu's sections, top to bottom, each with its entries in order.
pub(super) type Table<T, C = App> = &'static [&'static [Entry<T, C>]];

/// A message's menu: what Neucord has of the official client's (761304),
/// in its order. Create Thread follows Forward, which Neucord has no entry for.
/// Apps follows Copy Text and what Neucord has no entry for (Translate, Pin,
/// Remind Me), and precedes Copy Message Link.
const MESSAGE: Table<MessageTarget> = &[
    &[chat::COPY_SELECTION],
    &[reactions::ADD_REACTION, reactors::VIEW_REACTIONS],
    &[edits::EDIT, composer::REPLY, create_thread::MESSAGE_MENU],
    &[chat::COPY_TEXT, slash::MESSAGE_APPS, chat::COPY_MESSAGE_LINK, polls::END_POLL],
    &[reactions::REMOVE_EMOJI_REACTIONS, reactions::REMOVE_ALL_REACTIONS, edits::DELETE],
    &[chat::COPY_LINK, chat::OPEN_LINK],
    &[chat::COPY_MESSAGE_ID, stickers::MESSAGE_DETAILS],
];

/// [`MESSAGE`] where the media takedown regulation is TIDA: the client (761304)
/// puts the Copy ID group (50268) before the link group (777933). The group
/// between them holds nothing Neucord has.
const MESSAGE_REGULATED: Table<MessageTarget> = &[
    &[chat::COPY_SELECTION],
    &[reactions::ADD_REACTION, reactors::VIEW_REACTIONS],
    &[edits::EDIT, composer::REPLY, create_thread::MESSAGE_MENU],
    &[chat::COPY_TEXT, slash::MESSAGE_APPS, chat::COPY_MESSAGE_LINK, polls::END_POLL],
    &[reactions::REMOVE_EMOJI_REACTIONS, reactions::REMOVE_ALL_REACTIONS, edits::DELETE],
    &[chat::COPY_MESSAGE_ID, stickers::MESSAGE_DETAILS],
    &[chat::COPY_LINK, chat::OPEN_LINK],
];

/// The message menu for an account: `regulated` is READY's media takedown
/// regulation being TIDA, which the client orders the last groups by.
pub(super) fn message(regulated: bool) -> Table<MessageTarget> {
    if regulated { MESSAGE_REGULATED } else { MESSAGE }
}

/// A thread's menu, from its row in the sidebar or the browser and the box
/// under the message that started it: what Neucord has of the client's
/// (949488), and from a forum post's card. Join Thread and Edit Tags are in
/// the group of the thread's actions, Pin Post and Unpin Post in the
/// moderators', Leave Thread after. Edit Tags and the pins show on forum
/// posts only. Remove From Thread is on a thread's members (202643), whose
/// list Neucord doesn't show yet.
pub(super) const THREAD: Table<threads::ThreadTarget> =
    &[&[threads::JOIN_THREAD, forum::EDIT_TAGS], &[forum::PIN_POST, forum::UNPIN_POST], &[threads::LEAVE_THREAD]];

/// A forum's tag, on its filter (`forum-tag`, 960015): Copy Tag ID, in
/// Developer Mode like every copy of an id.
pub(super) const FORUM_TAG: Table<forum::Tagged> = &[&[forum::COPY_TAG_ID]];

/// A channel's menu, from its row in the sidebar: what Neucord has of the
/// client's (428631 for voice and stage channels, 823255 for the others), in
/// its order. Copy Link is among the channel's actions, Open Chat among the
/// voice ones, and Copy Channel ID is last. The rest needs what Neucord
/// doesn't have yet: Mark As Read, Mute and Notification Settings are #13's,
/// joining and what happens in a call is #12's.
pub(super) const CHANNEL: Table<sidebar::ChannelTarget> =
    &[&[sidebar::COPY_CHANNEL_LINK], &[sidebar::OPEN_CHAT], &[sidebar::COPY_CHANNEL_ID]];

/// A person's menu, opened from a message's author, a mention and a member
/// list row: what Neucord has of the official client's (`user-context`,
/// 841549, the server variant's groups). The client's also has call, voice,
/// role and moderation items, which Neucord has no surface for, and Add
/// Friend, Block and Ignore, whose requests Neucord has never sent. Apps is
/// in the group of Invite to Server and Remove Friend, and shows in the
/// channel the menu was opened in.
pub(super) const USER: Table<profile::UserTarget> = &[
    &[profile::PROFILE, profile::MENTION, profile::MESSAGE, profile::NOTE],
    &[slash::USER_APPS, profile::REMOVE_FRIEND],
    &[profile::COPY_USER_ID],
];

/// The attach button's menu, opened from the button: the official client's
/// (`channel-attach`, 316576) with what Neucord has of it. Upload a File and
/// Upload Text as File show where files may be attached, Record Voice Message
/// where the experiment and Send Voice Messages allow it (`canRecordVoiceMessage`),
/// Create Thread where threads may be started (`canStartThreads`), Create Poll
/// where polls may be posted (`canPostPolls`), Use Apps where nothing is being
/// replied to (`canUseApplicationCommands`), Schedule Message where we may
/// post (`vK` in 566908), and the rest are other features'.
pub(super) const ATTACH: Table<()> = &[&[
    uploads::UPLOAD_FILE,
    uploads::UPLOAD_TEXT,
    create_thread::ATTACH_MENU,
    poll_composer::CREATE_POLL,
    slash::USE_APPS,
    scheduling::SCHEDULE_MESSAGE,
    voice::RECORD_VOICE,
]];

/// Added below the message box's own menu (GTK's cut, copy, paste and
/// emoji). The official client's (194400) also has spellcheck and more.
pub(super) const COMPOSER: Table<()> = &[];

/// An emoji or sticker of the picker (`expression-picker`, 233503). The
/// client offers Copy Emoji, Copy Emoji ID and Copy Emoji Image Link, or
/// Copy Sticker ID and Copy Sticker Image Link, only in Developer Mode, and
/// a Favorite item first, which needs the favourites the client syncs as
/// protobuf settings.
pub(super) const EXPRESSION_PICKER: Table<picker::Target> = &[&[
    emoji::COPY_EMOJI,
    emoji::COPY_EMOJI_ID,
    emoji::COPY_EMOJI_LINK,
    stickers::COPY_STICKER_ID,
    stickers::COPY_STICKER_LINK,
]];

/// A GIF of the picker's results (`gif-picker`, 400017): the client's menu
/// has Copy Link and nothing else.
pub(super) const GIF_PICKER: Table<picker::Target> = &[&[gifs::COPY_LINK]];

/// Where menu items find their actions.
const PREFIX: &str = "menu";

/// `table`'s menu for `target`, with the entries that show, their actions,
/// to insert as [`PREFIX`], and the widgets for its custom items.
fn build<T: 'static, C: 'static>(
    cx: &Rc<C>,
    table: Table<T, C>,
    target: T,
) -> (gio::Menu, gio::SimpleActionGroup, Widgets) {
    let (menu, actions, widgets) = (gio::Menu::new(), gio::SimpleActionGroup::new(), Widgets::default());
    fill(cx, table, &Rc::new(target), &menu, &actions, &widgets);
    (menu, actions, widgets)
}

fn fill<T: 'static, C: 'static>(
    cx: &Rc<C>,
    table: Table<T, C>,
    target: &Rc<T>,
    menu: &gio::Menu,
    actions: &gio::SimpleActionGroup,
    widgets: &Widgets,
) {
    let detailed = |name: &str| format!("{PREFIX}.{name}");
    for entries in table {
        let section = gio::Menu::new();
        for entry in entries.iter() {
            match entry {
                Entry::Item(item) if !(item.shows)(cx, target) => {}
                Entry::Item(item) => match &item.kind {
                    Kind::Action(activate) => {
                        let action = gio::SimpleAction::new(item.name, None);
                        let (cx, target, activate) = (Rc::downgrade(cx), target.clone(), *activate);
                        action.connect_activate(move |_, _| {
                            if let Some(cx) = cx.upgrade() {
                                activate(&cx, &target);
                            }
                        });
                        actions.add_action(&action);
                        section.append(Some(item.label), Some(&detailed(item.name)));
                    }
                    Kind::Check { checked, set } => {
                        let action = check(cx, target, item.name, checked(cx, target), *set);
                        actions.add_action(&action);
                        section.append(Some(item.label), Some(&detailed(item.name)));
                    }
                    Kind::Submenu(table) => {
                        let submenu = gio::Menu::new();
                        fill(cx, table, target, &submenu, actions, widgets);
                        if submenu.n_items() > 0 {
                            section.append_submenu(Some(item.label), &submenu);
                        }
                    }
                    Kind::Live(make) => {
                        let submenu = gio::Menu::new();
                        make(
                            cx,
                            target,
                            Live { menu: submenu.clone(), actions: actions.clone(), widgets: widgets.clone() },
                        );
                        section.append_submenu(Some(item.label), &submenu);
                    }
                },
                Entry::Choices(choices) => {
                    let list = (choices.choices)(cx, target);
                    let choose = choices.choose;
                    // The value as the menu passes it, and to `choose`.
                    let pick = {
                        let (cx, target) = (Rc::downgrade(cx), target.clone());
                        move |value: &glib::Variant| {
                            if let (Some(cx), Some(value)) = (cx.upgrade(), value.str()) {
                                choose(&cx, &target, value, true);
                            }
                        }
                    };
                    let action = match choices.style {
                        Style::Plain => Some(gio::SimpleAction::new(choices.name, Some(glib::VariantTy::STRING))),
                        Style::Radio => {
                            let ticked = list.iter().find(|c| c.ticked).map_or("", |c| c.value.as_str());
                            Some(gio::SimpleAction::new_stateful(
                                choices.name,
                                Some(glib::VariantTy::STRING),
                                &ticked.to_variant(),
                            ))
                        }
                        Style::Check => None,
                    };
                    if let Some(action) = &action {
                        action.connect_activate(move |action, value| {
                            let Some(value) = value else { return };
                            if action.state().is_some() {
                                action.set_state(value);
                            }
                            pick(value);
                        });
                        actions.add_action(action);
                    }
                    for (n, choice) in list.iter().enumerate() {
                        // GTK reads `_` as a mnemonic; `__` is the character.
                        let item = gio::MenuItem::new(Some(&choice.label.replace('_', "__")), None);
                        match &action {
                            Some(_) => item.set_action_and_target_value(
                                Some(&detailed(choices.name)),
                                Some(&choice.value.to_variant()),
                            ),
                            None => {
                                let name = format!("{}-{n}", choices.name);
                                let value = choice.value.clone();
                                let set = move |cx: &Rc<C>, target: &T, on| choose(cx, target, &value, on);
                                actions.add_action(&check(cx, target, &name, choice.ticked, set));
                                item.set_detailed_action(&detailed(&name));
                            }
                        }
                        section.append_item(&item);
                    }
                }
            }
        }
        if section.n_items() > 0 {
            menu.append_section(None, &section);
        }
    }
}

/// A checkbox's action: ticked as `ticked` says, and flipped when chosen.
fn check<T: 'static, C: 'static>(
    cx: &Rc<C>,
    target: &Rc<T>,
    name: &str,
    ticked: bool,
    set: impl Fn(&Rc<C>, &T, bool) + 'static,
) -> gio::SimpleAction {
    let action = gio::SimpleAction::new_stateful(name, None, &ticked.to_variant());
    let (cx, target) = (Rc::downgrade(cx), target.clone());
    action.connect_activate(move |action, _| {
        let on = !action.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
        action.set_state(&on.to_variant());
        if let Some(cx) = cx.upgrade() {
            set(&cx, &target, on);
        }
    });
    action
}

/// Where a menu opens, in its parent's coordinates.
pub(super) enum At {
    /// Starting at a point, as GTK's own context menus do.
    Point(f64, f64),
    /// Under a button, lined up with its end: a button at the window's
    /// edge keeps its menu inside.
    Below(gdk::Rectangle),
}

/// Open `table`'s menu for `target` over `parent`.
pub(super) fn popup<T: 'static>(app: &Rc<App>, table: Table<T>, target: T, parent: &impl IsA<gtk::Widget>, at: At) {
    let (menu, actions, widgets) = build(app, table, target);
    if menu.n_items() == 0 {
        return;
    }
    let (rect, halign) = match at {
        At::Point(x, y) => (gdk::Rectangle::new(x as i32, y as i32, 1, 1), gtk::Align::Start),
        At::Below(rect) => (rect, gtk::Align::End),
    };
    let popover =
        gtk::PopoverMenu::builder().menu_model(&menu).has_arrow(false).halign(halign).pointing_to(&rect).build();
    popover.insert_action_group(PREFIX, Some(&actions));
    for (id, widget) in widgets.take() {
        popover.add_child(&widget, id);
    }
    popover.set_parent(parent);
    popover.connect_closed(|popover| {
        // Once the chosen item has run, which may come after closing.
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

/// Call `open` with where `widget` is right-clicked or long-pressed. It
/// goes ahead of the widgets inside, so a label's own menu doesn't open.
pub(super) fn on_secondary(widget: &impl IsA<gtk::Widget>, open: impl Fn(f64, f64) + 'static) {
    let open = Rc::new(open);
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    click.connect_pressed({
        let open = open.clone();
        move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open(x, y);
        }
    });
    widget.add_controller(click);
    let press =
        gtk::GestureLongPress::builder().touch_only(true).propagation_phase(gtk::PropagationPhase::Capture).build();
    press.connect_pressed(move |gesture, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        open(x, y);
    });
    widget.add_controller(press);
}

/// Call `open` when the keys for a menu (Menu or Shift+F10, as in GTK) are
/// pressed in `widget`, ahead of the widgets inside. It says whether it
/// opened one; if not, the keys go on.
pub(super) fn on_keys(widget: &impl IsA<gtk::Widget>, open: impl Fn(&gtk::Widget) -> bool + 'static) {
    let trigger = gtk::ShortcutTrigger::parse_string("Menu|<Shift>F10").expect("a valid trigger");
    let action = gtk::CallbackAction::new(move |widget, _| match open(widget) {
        true => glib::Propagation::Stop,
        false => glib::Propagation::Proceed,
    });
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
    shortcuts.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
    widget.add_controller(shortcuts);
}

/// Add `table`'s items to `view`'s own menu, below its items, for `target`
/// as it is when the menu opens.
pub(super) fn extend<T: 'static>(
    app: &Rc<App>,
    view: &gtk::TextView,
    table: Table<T>,
    target: impl Fn() -> T + 'static,
) {
    let extra = gio::Menu::new();
    view.set_extra_menu(Some(&extra));
    // The view builds its menu around ours, so ours is refilled just
    // before, however the menu is opened, and the view goes on to open it.
    let refill = Rc::new({
        let (app, view) = (Rc::downgrade(app), view.downgrade());
        move || {
            let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
            let actions = gio::SimpleActionGroup::new();
            extra.remove_all();
            fill(&app, table, &Rc::new(target()), &extra, &actions, &Widgets::default());
            view.insert_action_group(PREFIX, Some(&actions));
        }
    });
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    click.connect_pressed({
        let refill = refill.clone();
        move |_, _, _, _| refill()
    });
    view.add_controller(click);
    let press =
        gtk::GestureLongPress::builder().touch_only(true).propagation_phase(gtk::PropagationPhase::Capture).build();
    press.connect_pressed({
        let refill = refill.clone();
        move |_, _, _| refill()
    });
    view.add_controller(press);
    on_keys(view, move |_| {
        refill();
        false
    });
}

impl App {
    /// Put `text` on the clipboard.
    pub(super) fn copy(&self, text: &str) {
        self.window.clipboard().set_text(text);
    }

    /// Whether Shift is held, which some items read as the official
    /// client's do.
    pub(super) fn shift_held(&self) -> bool {
        WidgetExt::display(&self.window)
            .default_seat()
            .and_then(|seat| seat.keyboard())
            .is_some_and(|keyboard| keyboard.modifier_state().contains(gdk::ModifierType::SHIFT_MASK))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Stands in for the app: records what entries do.
    #[derive(Default)]
    struct Cx {
        log: RefCell<Vec<String>>,
    }

    type Target = &'static str;

    fn log(cx: &Rc<Cx>, line: String) {
        cx.log.borrow_mut().push(line);
    }

    const SHOWN: Entry<Target, Cx> = Entry::Item(Item {
        name: "shown",
        label: "_Shown",
        shows: |_, _| true,
        kind: Kind::Action(|cx, target| log(cx, format!("shown {target}"))),
    });
    const HIDDEN: Entry<Target, Cx> =
        Entry::Item(Item { name: "hidden", label: "Hidden", shows: |_, _| false, kind: Kind::Action(|_, _| {}) });
    const MUTED: Entry<Target, Cx> = Entry::Item(Item {
        name: "muted",
        label: "Muted",
        shows: |_, _| true,
        kind: Kind::Check { checked: |_, _| true, set: |cx, _, on| log(cx, format!("muted {on}")) },
    });
    const LEVEL: Entry<Target, Cx> = Entry::Choices(Choices {
        name: "level",
        choices: |_, _| {
            ["all", "mentions"].map(|v| Choice { value: v.into(), label: v.into(), ticked: v == "all" }).into()
        },
        style: Style::Radio,
        choose: |cx, _, value, on| log(cx, format!("level {value} {on}")),
    });
    const ROLES: Entry<Target, Cx> = Entry::Choices(Choices {
        name: "roles",
        choices: |_, _| vec![Choice { value: "7".into(), label: "mod_team".into(), ticked: false }],
        style: Style::Check,
        choose: |cx, _, value, on| log(cx, format!("role {value} {on}")),
    });
    const MUTE_FOR: Entry<Target, Cx> = Entry::Choices(Choices {
        name: "mute-for",
        choices: |_, _| vec![Choice { value: "900".into(), label: "For 15 Minutes".into(), ticked: false }],
        style: Style::Plain,
        choose: |cx, _, value, on| log(cx, format!("mute {value} {on}")),
    });
    const SETTINGS: Entry<Target, Cx> =
        Entry::Item(Item { name: "settings", label: "Settings", shows: |_, _| true, kind: Kind::Submenu(&[&[LEVEL]]) });
    const EMPTY: Entry<Target, Cx> =
        Entry::Item(Item { name: "empty", label: "Empty", shows: |_, _| true, kind: Kind::Submenu(&[&[HIDDEN]]) });
    const TABLE: Table<Target, Cx> = &[&[SHOWN, HIDDEN], &[HIDDEN, EMPTY], &[MUTED, SETTINGS], &[ROLES, MUTE_FOR]];

    fn label(menu: &gio::MenuModel, n: i32) -> Option<String> {
        menu.item_attribute_value(n, "label", Some(glib::VariantTy::STRING))?.get()
    }

    fn section(menu: &gio::MenuModel, n: i32) -> gio::MenuModel {
        menu.item_link(n, "section").expect("a section")
    }

    #[test]
    fn shows_what_shows_in_sections() {
        let cx = Rc::new(Cx::default());
        let (menu, _, _) = build(&cx, TABLE, "m");
        // The section of hidden items, and the submenu of them, are gone.
        assert_eq!(menu.n_items(), 3);
        let first = section(menu.upcast_ref(), 0);
        assert_eq!((first.n_items(), label(&first, 0).as_deref()), (1, Some("_Shown")));
        let second = section(menu.upcast_ref(), 1);
        assert_eq!((label(&second, 0).as_deref(), label(&second, 1).as_deref()), (Some("Muted"), Some("Settings")));
        let settings = second.item_link(1, "submenu").expect("a submenu");
        assert_eq!(section(&settings, 0).n_items(), 2);
        // Names from Discord keep their underscores.
        assert_eq!(label(&section(menu.upcast_ref(), 2), 0).as_deref(), Some("mod__team"));
    }

    /// The names of a table's entries, section by section.
    fn names(table: Table<MessageTarget>) -> Vec<Vec<&'static str>> {
        let name = |entry: &Entry<MessageTarget>| match entry {
            Entry::Item(item) => item.name,
            Entry::Choices(choices) => choices.name,
        };
        table.iter().map(|entries| entries.iter().map(name).collect()).collect()
    }

    #[test]
    fn message_menu_puts_the_id_before_the_links_where_media_takedown_is_regulated() {
        let ends = |regulated| {
            let names = names(message(regulated));
            names[names.len() - 2..].to_vec()
        };
        let id = || vec!["copy-message-id", "message-details"];
        assert_eq!(ends(false), [vec!["copy-link", "open-link"], id()]);
        assert_eq!(ends(true), [id(), vec!["copy-link", "open-link"]]);
        // The two groups swap and nothing else moves.
        let mut swapped = names(message(false));
        let n = swapped.len();
        swapped.swap(n - 2, n - 1);
        assert_eq!(names(message(true)), swapped);
    }

    #[test]
    fn create_thread_sits_where_the_client_puts_it() {
        let flat = |table: Table<MessageTarget>| names(table).concat();
        let place = |table, before: &str, after: &str| {
            let names = flat(table);
            let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
            assert_eq!(at("create-thread"), at(before) + 1);
            assert!(at("create-thread") < at(after));
        };
        // After Reply, which the client follows with Forward and then it.
        place(message(false), "reply", "copy-text");
        place(message(true), "reply", "copy-text");
        // The attach menu: after the uploads, before the poll.
        let attach: Vec<_> = ATTACH[0]
            .iter()
            .map(|entry| match entry {
                Entry::Item(item) => item.name,
                Entry::Choices(choices) => choices.name,
            })
            .collect();
        let (uploads, thread, poll) = (attach.iter().position(|n| *n == "upload-text"), "create-thread", "create-poll");
        let at = |name| attach.iter().position(|n| *n == name).unwrap();
        assert_eq!(Some(at(thread)), uploads.map(|u| u + 1));
        assert_eq!(at(poll), at(thread) + 1);
    }

    #[test]
    fn a_live_submenu_is_made_by_its_feature_and_shows_even_when_empty() {
        const FILLED: Entry<Target, Cx> = Entry::Item(Item {
            name: "filled",
            label: "Filled",
            shows: |_, _| true,
            kind: Kind::Live(|cx, target, live| {
                let action = gio::SimpleAction::new("inner", None);
                let (cx, target) = (Rc::downgrade(cx), *target.as_ref());
                action.connect_activate(move |_, _| {
                    if let Some(cx) = cx.upgrade() {
                        log(&cx, format!("inner {target}"));
                    }
                });
                live.actions.add_action(&action);
                live.menu.append(Some("Inner"), Some("menu.inner"));
            }),
        });
        const EMPTY_LIVE: Entry<Target, Cx> =
            Entry::Item(Item { name: "bare", label: "Bare", shows: |_, _| true, kind: Kind::Live(|_, _, _| {}) });
        const NOT_SHOWN: Entry<Target, Cx> =
            Entry::Item(Item { name: "no", label: "No", shows: |_, _| false, kind: Kind::Live(|_, _, _| panic!()) });
        const LIVE: Table<Target, Cx> = &[&[FILLED, EMPTY_LIVE, NOT_SHOWN]];
        let cx = Rc::new(Cx::default());
        let (menu, actions, _) = build(&cx, LIVE, "m");
        let section = section(menu.upcast_ref(), 0);
        assert_eq!(section.n_items(), 2, "both show, the one that doesn't isn't made");
        let filled = section.item_link(0, "submenu").expect("a submenu");
        assert_eq!(label(&filled, 0).as_deref(), Some("Inner"));
        assert_eq!(section.item_link(1, "submenu").expect("a submenu").n_items(), 0);
        actions.activate_action("inner", None);
        assert_eq!(*cx.log.borrow(), ["inner m"]);
    }

    #[test]
    fn entries_act_and_keep_their_state() {
        let cx = Rc::new(Cx::default());
        let (_, actions, _) = build(&cx, TABLE, "m");
        let state = |name: &str| actions.action_state(name);
        assert_eq!(state("muted").and_then(|s| s.get::<bool>()), Some(true));
        assert_eq!(state("level").and_then(|s| s.get::<String>()).as_deref(), Some("all"));
        assert!(!actions.has_action("hidden"));

        actions.activate_action("shown", None);
        actions.activate_action("muted", None);
        actions.activate_action("level", Some(&"mentions".to_variant()));
        actions.activate_action("roles-0", None);
        actions.activate_action("mute-for", Some(&"900".to_variant()));
        assert_eq!(*cx.log.borrow(), ["shown m", "muted false", "level mentions true", "role 7 true", "mute 900 true"]);
        assert_eq!(state("muted").and_then(|s| s.get::<bool>()), Some(false));
        assert_eq!(state("level").and_then(|s| s.get::<String>()).as_deref(), Some("mentions"));
    }
}
