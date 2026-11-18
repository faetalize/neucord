#!/usr/bin/env python3
"""Point a scratch copy of Neucord at the loopback fake and give it a command file."""
import os, re, sys
root = sys.argv[1]

def edit(path, old, new, count=1):
    p = f"{root}/{path}"
    s = open(p).read()
    assert old in s, (path, old)
    open(p, "w").write(s.replace(old, new, count))

edit("crates/discord/src/identity.rs", 'pub const API_BASE: &str = "https://discord.com/api/v9";',
     'pub const API_BASE: &str = "http://127.0.0.1:8099/api/v9";')
edit("crates/discord/src/identity.rs",
     'pub const GATEWAY_URL: &str = "wss://gateway.discord.gg/?encoding=json&v=9&compress=zlib-stream";',
     'pub const GATEWAY_URL: &str = "ws://127.0.0.1:8099/?encoding=json&v=9&compress=zlib-stream";')
edit("crates/discord/src/cdn.rs", 'host == "media.discordapp.net" ||', 'host == "127.0.0.1:8099" || host == "media.discordapp.net" ||')
edit("crates/discord/src/cdn.rs", 'let host = url.strip_prefix("https://").and_then(|rest| rest.split([\'/\', \'?\']).next()).unwrap_or_default();', 'let host = url.strip_prefix("http://").and_then(|rest| rest.split([\'/\', \'?\']).next()).unwrap_or_default();')
edit("crates/app/src/ui/uploads.rs", "    attach: gtk::Button,", "    pub(super) attach: gtk::Button,")
edit("crates/app/src/ui/members.rs", "\n    scroller: gtk::ScrolledWindow,", "\n    pub(super) scroller: gtk::ScrolledWindow,")
edit("crates/app/src/ui/mod.rs", "\n    members: members::Pane,", "\n    pub(super) members: members::Pane,")
for field in ("stack: adw::ViewStack", "entry: gtk::Entry", "note: gtk::Entry", "note_left: gtk::Label", "note_hint: gtk::Label",
              "send: gtk::Button", "add_status: gtk::Label"):
    edit("crates/app/src/ui/friends.rs", f"\n    {field},", f"\n    pub(super) {field},")
edit("crates/app/src/ui/mod.rs", "mod emoji;", "mod e2e;\nmod emoji;")
# What a right-click does to the message box's menu before GTK opens it, for `composer-menu`.
edit("crates/app/src/ui/menus.rs", "    view.add_controller(press);\n    on_keys(view, move |_| {",
     "    view.add_controller(press);\n    super::e2e::REFILL.with(|r| r.borrow_mut().push({ let refill = refill.clone(); Rc::new(move |at| refill(at)) }));\n    on_keys(view, move |_| {")
edit("crates/app/src/ui/mod.rs", "glib::spawn_future_local(async move { this.resume_session().await });",
     "let _ = &this; self.e2e_start();")
open(f"{root}/crates/app/src/ui/e2e.rs", "w").write(r"""
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::Snowflake;
use gtk::{gdk, gio, glib};

use super::App;
use super::chat::{Clicked, emoji_at, media_at};
use super::composer::Completion;
use super::menus::At;
use super::message_row::MessageRow;

thread_local! {
    /// Each `menus::extend`'s refill, the message box's first.
    pub(super) static REFILL: RefCell<Vec<Rc<dyn Fn(Option<(f64, f64)>)>>> = RefCell::default();
}

pub(super) fn shot(widget: &impl IsA<gtk::Widget>, path: &str) {
    let w = widget.upcast_ref::<gtk::Widget>();
    let paintable = gtk::WidgetPaintable::new(Some(w));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w.width() as f64, w.height() as f64);
    let Some(node) = snapshot.to_node() else { return eprintln!("e2e: nothing to draw") };
    let Some(renderer) = w.native().and_then(|n| n.renderer()) else { return eprintln!("e2e: no renderer") };
    let texture = renderer.render_texture(&node, None);
    if let Err(e) = texture.save_to_png(path) {
        eprintln!("e2e: {e}");
    }
    eprintln!("e2e: shot {path} {}x{}", w.width(), w.height());
}

/// The buttons below `widget`, in order, that `wanted` picks: the packs page's Pin and Unpin buttons, one for
/// each pack, or the picker's button to the page.
fn buttons_below(widget: &gtk::Widget, wanted: impl Fn(&gtk::Button) -> bool + Copy) -> Vec<gtk::Button> {
    let mut found = Vec::new();
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(button) = c.downcast_ref::<gtk::Button>().filter(|b| wanted(b)) {
            found.push(button.clone());
        }
        found.extend(buttons_below(&c, wanted));
        child = c.next_sibling();
    }
    found
}

/// The popover showing on `widget`, of those parented to it.
fn popover_on(widget: &gtk::Widget) -> Option<gtk::Popover> {
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Ok(p) = c.clone().downcast::<gtk::Popover>()
            && p.is_mapped()
        {
            return Some(p);
        }
        child = c.next_sibling();
    }
    None
}

/// The popover open on `widget` or on what holds it (the attach menu is parented to the box around its button).
fn popover_above(widget: &gtk::Widget) -> Option<gtk::Popover> {
    let mut at = Some(widget.clone());
    while let Some(w) = at {
        if let Some(popover) = popover_on(&w) {
            return Some(popover);
        }
        at = w.parent();
    }
    None
}

fn buttons(widget: &gtk::Widget, found: &mut Vec<gtk::Button>) {
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Ok(b) = c.clone().downcast::<gtk::Button>() {
            found.push(b);
        }
        buttons(&c, found);
        child = c.next_sibling();
    }
}

impl App {
    pub(super) fn e2e_start(self: &Rc<Self>) {
        let token = std::env::var("E2E_TOKEN").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "fake-token-1".into());
        self.window.present();
        // As resuming a stored account does: its id is known before READY.
        self.account.state.borrow_mut().target = Some(Snowflake(100));
        self.connect(token, true);
        let this = Rc::downgrade(self);
        let path = std::env::var("E2E_CMD").expect("E2E_CMD");
        glib::timeout_add_local(Duration::from_millis(200), move || {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            if let Ok(text) = std::fs::read_to_string(&path) {
                let _ = std::fs::remove_file(&path);
                for line in text.lines() {
                    this.e2e_run(line);
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn e2e_run(self: &Rc<Self>, line: &str) {
        eprintln!("e2e: {line}");
        let mut parts = line.split_whitespace();
        // What follows the command word, spaces kept, for text.
        let rest = line.trim_start().split_once(' ').map_or("", |(_, rest)| rest);
        let id = |s: Option<&str>| Snowflake(s.and_then(|s| s.parse().ok()).unwrap_or(0));
        match parts.next() {
            Some("channel") => self.select_channel(id(parts.next())),
            Some("guild") => self.select_guild(id(parts.next())),
            Some("home") => self.select_home(),
            Some("attach") => self.drafts.attach.emit_clicked(),
            // Create Thread for the open channel, as the attach menu's entry opens it.
            // The open forum's search box, as typed.
            Some("forum-search") => self.forum.search.set_text(rest),
            // The list of who voted on a poll: its message and the answer to open at.
            Some("poll-votes") => {
                let message = id(parts.next());
                let answer = parts.next().and_then(|a| a.parse().ok()).unwrap_or(1);
                let channel = self.account.state.borrow().channel;
                if let Some(message) = channel.and_then(|c| self.message_in(c, message)) {
                    self.view_poll_votes(&message, answer);
                }
            }
            // The emoji picker, from the message box's button.
            Some("emoji-picker") => self.emoji_button.emit_clicked(),
            // The friends page: `friends <tab>` shows it on a tab (online, all, pending, blocked, add),
            // `friend-entry <text>` and `friend-note <text>` fill the Add Friend form, `friend-send` sends it,
            // `view-request` presses the first View Request, `friend-state` prints what the form says.
            Some("friends") => {
                self.show_friends();
                if let Some(tab) = parts.next() {
                    self.friends.stack.set_visible_child_name(tab);
                }
            }
            Some("friend-entry") => self.friends.entry.set_text(rest),
            Some("friend-note") => self.friends.note.set_text(rest),
            Some("friend-send") => self.friends.send.emit_clicked(),
            Some("view-request") => {
                match buttons_below(self.friends.root.upcast_ref(), |b| b.child().and_downcast::<adw::ButtonContent>().is_some_and(|c| c.label() == "View Request")).into_iter().next() {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no View Request"),
                }
            }
            Some("friend-state") => {
                let page = &self.friends;
                eprintln!(
                    "e2e: friend-state status={:?} shown={} hint={} left={:?} note={:?}",
                    page.add_status.label(), page.add_status.is_visible(), page.note_hint.is_visible(),
                    page.note_left.label(), page.note.text()
                );
            }
            // The emoji packs page, as the open picker's button opens it (the packs must be fetched),
            // `packs-pin <n>` clicks the Pin or Unpin button of its nth pack, `packs-shot <png>` shoots it and
            // `packs-close` closes it.
            Some("emoji-packs") => {
                let popover = popover_on(self.emoji_button.upcast_ref());
                let button = popover.and_then(|p| buttons_below(p.upcast_ref(), |b| b.tooltip_text().as_deref() == Some("Emoji Packs")).pop());
                match button {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no packs button"),
                }
            }
            Some("packs-pin") => {
                let n: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let dialog = self.window.visible_dialog();
                match dialog.and_then(|d| buttons_below(d.upcast_ref(), |b| matches!(b.label().as_deref(), Some("Pin" | "Unpin"))).into_iter().nth(n)) {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no pin button {n}"),
                }
            }
            Some("packs-shot") => match self.window.visible_dialog() {
                Some(dialog) => shot(&dialog, parts.next().unwrap_or("packs.png")),
                None => eprintln!("e2e: no dialog"),
            },
            Some("packs-close") => {
                if let Some(dialog) = self.window.visible_dialog() {
                    dialog.close();
                }
            }
            // Click the nth quick reaction on the bar of the first row that has them.
            Some("hover-react") => {
                let n: i32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let rows: Vec<_> = self.rows.borrow().iter().filter_map(|r| r.upgrade()).collect();
                let button = rows.iter().find_map(|row| {
                    let quick = &row.parts().quick;
                    quick.is_visible().then(|| quick.observe_children().item(n as u32).and_downcast::<gtk::Button>()).flatten()
                });
                match button {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no quick reaction {n}"),
                }
            }
            // The thread browser, its name search (as typed, then Enter) and the threads it lists.
            Some("threads") => {
                let channel = self.account.state.borrow().channel;
                if let Some(channel) = channel {
                    self.open_threads(channel);
                }
            }
            Some("thread-search") => {
                let view = self.threads_view.borrow().clone();
                if let Some(view) = view {
                    view.search.set_text(rest);
                    view.search.emit_activate();
                }
            }
            Some("thread-rows") => {
                let view = self.threads_view.borrow().clone();
                if let Some(view) = view {
                    let rows: Vec<String> = view.rows.borrow().iter().map(|r| format!("{r:?}")).collect();
                    eprintln!("e2e: thread rows {}", rows.join(" "));
                }
            }
            Some("thread-form") => {
                if let Some(channel) = self.thread_channel() {
                    self.open_thread_form(channel, None, "Plus Button");
                }
            }
            Some("shot") => shot(&self.window, parts.next().unwrap_or("shot.png")),
            Some("shot-popover") => {
                let anchor: Option<gtk::Widget> = match parts.next() {
                    Some("composer") => Some(self.composer.clone().upcast()),
                    Some("composer_box") => Some(self.composer_box.clone().upcast()),
                    Some("gif_button") => Some(self.gif_button.clone().upcast()),
                    Some("emoji_button") => Some(self.emoji_button.clone().upcast()),
                    Some("members_pane") => Some(self.members.scroller.clone().upcast()),
                    Some(row) if row.starts_with("row:") => {
                        let wanted = id(row.strip_prefix("row:"));
                        self.rows.borrow().iter().filter_map(|r| r.upgrade()).find(|r| {
                            r.parts().item.upgrade().and_then(|i| i.item().and_downcast::<glib::BoxedAnyObject>())
                                .is_some_and(|o| o.borrow::<discord::Message>().id == wanted)
                        }).map(|r| r.parts().row.clone().upcast())
                    }
                    Some("attach_button") => Some(self.drafts.attach.clone().upcast()),
                    Some("pins_button") => Some(self.pins_button.clone().upcast()),
                    Some("inbox_button") => Some(self.inbox_button.clone().upcast()),
                    other => return eprintln!("e2e: no widget {other:?}"),
                };
                match anchor.and_then(|w| popover_above(&w)) {
                    Some(popover) => {
                        shot(&popover, parts.next().unwrap_or("popover.png"));
                        // A third word, `close`, closes it after.
                        if parts.next() == Some("close") {
                            popover.popdown();
                        }
                    }
                    None => eprintln!("e2e: no popover"),
                }
            }
            // As typed (gtk_text_view_commit_text): over the selection, in one user action, where editable.
            Some("type") => {
                let buffer = self.composer.buffer();
                let editable = self.composer.is_editable();
                buffer.begin_user_action();
                buffer.delete_selection(true, editable);
                if !buffer.insert_interactive_at_cursor(rest, editable) {
                    eprintln!("e2e: not editable");
                }
                buffer.end_user_action();
            }
            Some("set") => self.set_composer_text(rest),
            // GTK's own Backspace at the caret, as the key does when the box leaves it alone.
            Some("backspace") => {
                let buffer = self.composer.buffer();
                let mut at = buffer.iter_at_offset(buffer.property::<i32>("cursor-position"));
                buffer.backspace(&mut at, true, self.composer.is_editable());
            }
            Some("suggestions") => {
                let shown = self.mention_popover.is_visible();
                let selected = self.mention_list.selected_row().map(|r| r.index());
                let rows: Vec<String> = self
                    .mention
                    .borrow()
                    .candidates
                    .iter()
                    .filter(|_| shown)
                    .map(|c| match c {
                        Completion::Command(s) => format!("{} ({})", s.name, s.app),
                        Completion::Status(text) => format!("[{text}]"),
                        other => other.label(),
                    })
                    .collect();
                let headers = self.mention.borrow().headers.clone();
                eprintln!("e2e: suggestions {}", rows.len());
                for (i, row) in rows.iter().enumerate() {
                    if let Some((_, header)) = headers.iter().find(|(at, _)| *at == i) {
                        eprintln!("e2e: header {} {}", header.words, header.query.as_deref().unwrap_or_default());
                    }
                    let mark = if selected == Some(i as i32) { " *" } else { "" };
                    eprintln!("e2e: suggestion {i} {row}{mark}");
                }
                if let Some(path) = parts.next().filter(|_| shown) {
                    shot(&self.mention_popover, path);
                }
            }
            Some("pick") => self.accept_mention(parts.next().and_then(|n| n.parse().ok()).unwrap_or(0)),
            // A key pressed in the message box, by its GDK name, with modifiers after it.
            Some("key") => {
                let Some(key) = parts.next().and_then(gtk::gdk::Key::from_name) else {
                    return eprintln!("e2e: no such key");
                };
                let mut modifiers = gtk::gdk::ModifierType::empty();
                for word in parts {
                    modifiers |= match word {
                        "shift" => gtk::gdk::ModifierType::SHIFT_MASK,
                        "ctrl" => gtk::gdk::ModifierType::CONTROL_MASK,
                        "alt" => gtk::gdk::ModifierType::ALT_MASK,
                        other => return eprintln!("e2e: no modifier {other}"),
                    };
                }
                let handled = self.composer_key(key, modifiers) == glib::Propagation::Stop;
                eprintln!("e2e: key {}", if handled { "handled" } else { "not handled" });
            }
            Some("reply") => {
                let wanted = parts.next().map(|s| id(Some(s)));
                let messages = self.messages();
                let found = (0..messages.n_items())
                    .rev()
                    .filter_map(|position| crate::timelines::message_at(&messages, position))
                    .find(|m| wanted.is_none_or(|w| m.id == w));
                match found {
                    Some(message) => self.start_reply(&message, false),
                    None => eprintln!("e2e: no message to reply to"),
                }
            }
            // A person's menu from the member pane, as a right click on their row opens it: `member-menu <id>`
            // (the open channel's), `shot-popover members_pane <png> [close]`, and `member-menu-item <name>`.
            Some("member-menu") => {
                let user = self.account.state.borrow().whole_users.get(&id(parts.next()).0).cloned();
                let (guild, channel) = {
                    let state = self.account.state.borrow();
                    (state.guild, state.channel)
                };
                let anchor: gtk::Widget = self.members.scroller.clone().upcast();
                match user {
                    Some(user) => self.user_menu(
                        crate::ui::profile::UserTarget { user, guild, channel, presence: None, anchor: anchor.clone() },
                        &anchor,
                        crate::ui::menus::At::Point(20.0, 20.0),
                    ),
                    None => eprintln!("e2e: no such user"),
                }
            }
            Some("member-menu-item") => {
                let name = parts.next().unwrap_or_default();
                let anchor: gtk::Widget = self.members.scroller.clone().upcast();
                match popover_above(&anchor) {
                    Some(popover) => match popover.activate_action(&format!("menu.{name}"), None) {
                        Ok(()) => eprintln!("e2e: chose {name}"),
                        Err(e) => eprintln!("e2e: {e}"),
                    },
                    None => eprintln!("e2e: no menu"),
                }
            }
            // `t` on a message (the shortcut's own action: what the key handler runs for a focused row):
            // `thread-key <id>` says whether it did something. `message-more <id>` presses the row's More
            // button, and `shot-popover row:<id> <png>` shoots the menu it opens.
            Some("thread-key") => {
                let wanted = id(parts.next());
                let messages = self.messages();
                let found = (0..messages.n_items())
                    .filter_map(|position| crate::timelines::message_at(&messages, position))
                    .find(|m| m.id == wanted);
                match found {
                    Some(message) => eprintln!("e2e: thread-key {}", self.thread_shortcut(&message)),
                    None => eprintln!("e2e: no such message"),
                }
            }
            Some("message-more") => {
                let wanted = id(parts.next());
                let row = self.rows.borrow().iter().filter_map(|r| r.upgrade()).find(|row| {
                    row.parts().item.upgrade().and_then(|i| i.item().and_downcast::<glib::BoxedAnyObject>())
                        .is_some_and(|o| o.borrow::<discord::Message>().id == wanted)
                });
                match row {
                    Some(row) => row.parts().more.emit_clicked(),
                    None => eprintln!("e2e: no row"),
                }
            }
            // The nudge's Create Thread, and the open thread form's Create.
            Some("nudge-click") => {
                match buttons_below(self.reply_nudge.upcast_ref(), |b| b.label().as_deref() == Some("Create Thread")).pop() {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no nudge button"),
                }
            }
            // The open thread form's starter message, as typed.
            Some("thread-message") => {
                fn text_view(widget: &gtk::Widget) -> Option<gtk::TextView> {
                    let mut child = widget.first_child();
                    while let Some(c) = child {
                        if let Some(view) = c.downcast_ref::<gtk::TextView>() {
                            return Some(view.clone());
                        }
                        if let Some(view) = text_view(&c) {
                            return Some(view);
                        }
                        child = c.next_sibling();
                    }
                    None
                }
                match self.window.visible_dialog().and_then(|d| text_view(d.upcast_ref())) {
                    Some(view) => view.buffer().set_text(rest),
                    None => eprintln!("e2e: no message field"),
                }
            }
            Some("thread-create") => {
                let found = self.window.visible_dialog().and_then(|d| {
                    buttons_below(d.upcast_ref(), |b| b.label().as_deref() == Some("Create")).pop()
                });
                match found {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no Create button"),
                }
            }
            Some("reply-nudge") => {
                eprintln!("e2e: reply-nudge shown={} text={:?}", self.reply_nudge.is_visible(), self.reply_nudge_label.label());
            }
            Some("confirm") => {
                let Some(dialog) = self.window.visible_dialog().and_downcast::<adw::AlertDialog>() else {
                    return eprintln!("e2e: no alert");
                };
                // The response named, else the last that isn't the close response, as its button.
                let label = |response: &str| dialog.has_response(response).then(|| dialog.response_label(response));
                let close = label(&dialog.close_response());
                let wanted = match parts.next() {
                    Some(response) if label(response).is_none() => return eprintln!("e2e: no response {response}"),
                    response => response.and_then(label),
                };
                let mut found = Vec::new();
                buttons(dialog.upcast_ref(), &mut found);
                let chosen = found.into_iter().filter(|b| b.is_mapped()).rfind(|b| match (&wanted, b.label()) {
                    (Some(wanted), Some(text)) => &text == wanted,
                    (None, Some(text)) => close.as_ref() != Some(&text),
                    (_, None) => false,
                });
                match chosen {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no such response"),
                }
            }
            Some("press") => {
                let message = id(parts.next());
                let custom_id = parts.next().unwrap_or_default().to_owned();
                match self.account.state.borrow().channel {
                    Some(channel) => self.interact(channel, message, custom_id, discord::interaction::BUTTON, None),
                    None => eprintln!("e2e: no channel"),
                }
            }
            Some(command) if command.starts_with("modal-") => self.e2e_modal(command, rest),
            // The message box's menu, as a right-click at x y opens it (else the Menu key): shoot it with
            // `shot-popover composer`.
            Some("composer-menu") => {
                let at = parts.next().zip(parts.next()).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
                if let Some(refill) = REFILL.with(|r| r.borrow().first().cloned()) {
                    refill(at);
                }
                if let Err(e) = self.composer.activate_action("menu.popup", None) {
                    eprintln!("e2e: {e}");
                }
            }
            // An item of the message box's menu, by its name, as chosen.
            Some("menu-item") => {
                let name = parts.next().unwrap_or_default();
                match self.composer.activate_action(&format!("menu.{name}"), None) {
                    Ok(()) => eprintln!("e2e: chose {name}"),
                    Err(e) => eprintln!("e2e: {e}"),
                }
            }
            // The form for a password the account has to change, over the app: shoot the window.
            Some("ask-password") => self.ask_new_password(),
            // What the form sends, as a login's answer would have it send: `password-send <current> <new>`.
            Some("password-send") => {
                let (old, new) = (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned());
                if let Some(rest) = self.account.state.borrow().rest.clone() {
                    glib::spawn_future_local(async move {
                        match rest.update_password(&old, &new).await {
                            Ok(token) => eprintln!("e2e: password changed, new token {token}"),
                            Err(e) => eprintln!("e2e: password refused: {e}"),
                        }
                    });
                }
            }
            // A row of the attach menu's popover by its label, as clicked: `attach-click Apps`.
            Some("attach-click") => {
                let attach: gtk::Widget = self.drafts.attach.clone().upcast();
                let mut found = Vec::new();
                if let Some(popover) = popover_above(&attach) {
                    buttons(popover.upcast_ref(), &mut found);
                }
                let wanted = rest.trim();
                let row = found.into_iter().find(|b| {
                    let mut label = String::new();
                    fn text(w: &gtk::Widget, out: &mut String) {
                        if let Ok(l) = w.clone().downcast::<gtk::Label>() {
                            out.push_str(&l.text());
                        }
                        let mut child = w.first_child();
                        while let Some(c) = child {
                            text(&c, out);
                            child = c.next_sibling();
                        }
                    }
                    text(b.upcast_ref(), &mut label);
                    label.contains(wanted)
                });
                match row {
                    Some(row) => row.emit_clicked(),
                    None => eprintln!("e2e: no row {wanted}"),
                }
            }
            // The pinned messages popover, as its header button opens it: shoot it with `shot-popover pins_button`.
            Some("pins-open") => self.pins_button.popup(),
            // The next page, as the button under the list or a scroll near its end asks.
            Some("pins-more") => self.more_pins(),
            // The inbox, as its header button opens it: shoot it with `shot-popover inbox_button`.
            Some("inbox-open") => self.inbox_button.popup(),
            Some("inbox-more") => self.more_mentions(),
            // The first mention marked done, as its button does.
            Some("inbox-done") => {
                let first = self.account.state.borrow().mentions.list().first().map(|m| m.id);
                if let Some(id) = first {
                    self.mention_done(id);
                }
            }
            // The message menu on the custom emoji in the newest row that has one, found as a click would:
            // `emoji-menu` turns Developer Mode on, scans the row for the emoji and opens the menu there.
            Some("emoji-menu") => {
                self.account.state.borrow_mut().settings.developer_mode = true;
                let rows: Vec<MessageRow> = self.rows.borrow().iter().filter_map(|r| r.upgrade()).collect();
                for row in rows {
                    let parts = row.parts();
                    let (Some(item), width, height) = (parts.item.upgrade(), parts.row.width(), parts.row.height())
                    else {
                        continue;
                    };
                    let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { continue };
                    let message = message.borrow::<discord::Message>().clone();
                    if !message.content.contains("emoji-probe") {
                        continue;
                    }
                    let mut hit = None;
                    'scan: for y in (0..height).step_by(4) {
                        for x in (0..width).step_by(2) {
                            if let Some(emoji) = emoji_at(parts.row.upcast_ref(), f64::from(x), f64::from(y)) {
                                hit = Some((f64::from(x), f64::from(y), emoji));
                                break 'scan;
                            }
                        }
                    }
                    match hit {
                        Some((x, y, emoji)) => {
                            eprintln!("e2e: emoji {} at {x},{y}: {}", emoji.id, emoji.link);
                            let clicked = Clicked { emoji: Some(emoji), ..Clicked::default() };
                            self.message_menu(&parts.row, message, At::Point(x, y), clicked);
                            // The popover is its own surface: shoot it once it is up, as `emoji-menu <png>`.
                            let (anchor, path) = (parts.row.clone().upcast::<gtk::Widget>(), rest.trim().to_owned());
                            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                                match popover_above(&anchor) {
                                    Some(popover) if !path.is_empty() => shot(&popover, &path),
                                    _ => eprintln!("e2e: no popover on the row"),
                                }
                            });
                        }
                        None => eprintln!("e2e: no emoji in the row"),
                    }
                    return;
                }
                eprintln!("e2e: no row with emoji-probe");
            }
            // The message menu on the picture in the row with `embed-probe`, found as a click would be: `embed-menu <png>`.
            Some("embed-menu") => {
                let rows: Vec<MessageRow> = self.rows.borrow().iter().filter_map(|r| r.upgrade()).collect();
                for row in rows {
                    let parts = row.parts();
                    let (Some(item), width, height) = (parts.item.upgrade(), parts.row.width(), parts.row.height())
                    else {
                        continue;
                    };
                    let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { continue };
                    let message = message.borrow::<discord::Message>().clone();
                    if !message.content.contains("embed-probe") {
                        continue;
                    }
                    let mut hit = None;
                    'scan: for y in (0..height).step_by(4) {
                        for x in (0..width).step_by(4) {
                            if let Some(media) = media_at(parts.row.upcast_ref(), f64::from(x), f64::from(y)) {
                                hit = Some((f64::from(x), f64::from(y), media));
                                break 'scan;
                            }
                        }
                    }
                    let Some((x, y, media)) = hit else { return eprintln!("e2e: no media in the row") };
                    eprintln!("e2e: media {media} at {x},{y}");
                    let clicked = Clicked { media: Some(media), ..Clicked::default() };
                    self.message_menu(&parts.row, message, At::Point(x, y), clicked);
                    let (anchor, path) = (parts.row.clone().upcast::<gtk::Widget>(), rest.trim().to_owned());
                    glib::timeout_add_local_once(Duration::from_millis(600), move || match popover_above(&anchor) {
                        Some(popover) if !path.is_empty() => shot(&popover, &path),
                        _ => eprintln!("e2e: no popover on the row"),
                    });
                    return;
                }
                eprintln!("e2e: no row with embed-probe");
            }
            // Files dropped on the chat, as a drop does: `drop <path>` joins the draft, `drop-shift <path>` is sent
            // at once (Shift held); `drop-overlay` shows what a drag says.
            Some(command @ ("drop" | "drop-shift")) => {
                let (Some(channel), Some(path)) = (self.attach_channel(), parts.next()) else { return };
                let (this, file, instant) = (self.clone(), gio::File::for_path(path), command == "drop-shift");
                glib::spawn_future_local(async move { this.dropped(channel, vec![file], instant).await });
            }
            Some("drop-overlay") => self.show_drop_overlay(true),
            // The member pane closed, as its toggle does, so a channel opened after it subscribes to no list.
            Some("members-closed") => {
                self.account.state.borrow_mut().members_closed = true;
                self.sync_members();
            }
            // The search box: `search-open` opens it, `search-type <text>` puts text in it.
            Some("search-open") => self.toggle_search(),
            Some("search-type") => {
                let view = self.search_view.borrow().clone();
                if let Some(view) = view {
                    // As typed at the end of the box, so the cursor is past it when the change is read.
                    view.entry.set_text("");
                    let mut at = 0;
                    view.entry.insert_text(rest, &mut at);
                    // Programmatic inserts leave the cursor behind: read the box again with it at the end.
                    view.entry.set_position(-1);
                    view.suggest(self);
                }
            }
            Some("send") => {
                let button = &self.send_button;
                if !button.is_visible() || !button.is_sensitive() {
                    return eprintln!("e2e: send button visible={} sensitive={}", button.is_visible(), button.is_sensitive());
                }
                button.emit_clicked();
                eprintln!("e2e: sent");
            }
            // A copied image (2×2 red pixels), or a copied file with its path as text, as a file manager copies.
            Some("clip-image") => {
                let bytes = glib::Bytes::from_owned([255u8, 0, 0, 255].repeat(4));
                let texture = gdk::MemoryTexture::new(2, 2, gdk::MemoryFormat::R8g8b8a8, &bytes, 8);
                self.composer.clipboard().set_texture(&texture);
            }
            Some("clip-files") => {
                let path = parts.next().unwrap_or("/tmp/e2e.txt");
                let files = gdk::FileList::from_array(&[gio::File::for_path(path)]);
                let provider = gdk::ContentProvider::new_union(&[
                    gdk::ContentProvider::for_value(&files.to_value()),
                    gdk::ContentProvider::for_value(&path.to_value()),
                ]);
                if let Err(e) = self.composer.clipboard().set_content(Some(&provider)) {
                    eprintln!("e2e: {e}");
                }
            }
            Some("line") => self.e2e_line(),
            Some("quit") => {
                if let Some(app) = self.window.application() {
                    app.quit();
                }
            }
            other => eprintln!("e2e: unknown {other:?}"),
        }
    }
}
""")

# `line` prints the command in the box: its text, each pill (option, value, red), the caret, the bar, "+n", the
# app icon and the buttons beside it.
open(f"{root}/crates/app/src/ui/e2e.rs", "a").write(r"""
impl App {
    fn e2e_line(self: &Rc<Self>) {
        let buffer = self.composer.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        let caret = buffer.property::<i32>("cursor-position");
        let selection = buffer.selection_bounds().map(|(a, b)| (a.offset(), b.offset()));
        eprintln!("e2e: box {text:?} caret {caret} selection {selection:?}");
        let Some((line, text)) = self.read_line() else { return eprintln!("e2e: no command") };
        let chars: Vec<char> = text.chars().collect();
        let slot = self.slash.active.borrow();
        let active = slot.as_ref().expect("active");
        for pill in &line.pills {
            let value: String = chars[pill.value.clone()].iter().collect();
            let red = if active.states.errored(&pill.option) { " errored" } else { "" };
            eprintln!("e2e: pill {} {value:?}{red}", pill.option);
        }
        eprintln!("e2e: bar {:?} {:?} error={}", self.slash.title.label(), self.slash.description.label(),
                  self.slash.description.has_css_class("error"));
        eprintln!("e2e: more {:?} icon={} attach={} gif={} emoji={} length={}",
                  self.slash.more.is_visible().then(|| self.slash.more.label()), self.slash.icon.is_visible(),
                  self.drafts.attach.is_visible(), self.gif_button.is_visible(), self.emoji_button.is_visible(),
                  self.length_visible());
    }
}
""")
edit("crates/app/src/ui/composer.rs", "    pub(super) fn update_length_indicator(&self) {",
     "    pub(super) fn length_visible(&self) -> bool {\n        self.length.root.is_visible()\n    }\n\n    pub(super) fn update_length_indicator(&self) {")

# The GIF picker: `gif` opens it, `gif-search <q>` types into its search, `gif-pick <n>`, `gif-shift-pick <n>`
# and `gif-star <n>` press the nth GIF shown or its star, `gif-tile <n>` the nth front page tile, `gif-back`
# its back button; `gif-hint [png]` prints whether "Added to Favorites" and the pulse show, and shoots the hint.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(gif) if gif.starts_with("gif") => self.e2e_gif(gif, parts.next()),
            Some("quit") => {''')
edit("crates/app/src/ui/menus.rs", "    pub(super) fn shift_held(&self) -> bool {\n",
     "    pub(super) fn shift_held(&self) -> bool {\n        if super::e2e::SHIFT.get() {\n            return true;\n        }\n")
open(f"{root}/crates/app/src/ui/e2e.rs", "a").write(r"""
thread_local! {
    pub(super) static SHIFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn widgets<T: IsA<gtk::Widget>>(widget: &gtk::Widget, found: &mut Vec<T>) {
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Ok(t) = c.clone().downcast::<T>() {
            found.push(t);
        }
        widgets(&c, found);
        child = c.next_sibling();
    }
}

impl App {
    fn e2e_gif(self: &Rc<Self>, command: &str, arg: Option<&str>) {
        if command == "gif" {
            return self.gif_button.emit_clicked();
        }
        if command == "gif-hint" {
            eprintln!("e2e: hint {} pulse {}", self.gif_hint.is_mapped(), self.gif_button.has_css_class("pulse"));
            if let Some(path) = arg.filter(|_| self.gif_hint.is_mapped()) {
                shot(&self.gif_hint, path);
            }
            return;
        }
        let mut popovers: Vec<gtk::Popover> = Vec::new();
        widgets(self.gif_button.upcast_ref(), &mut popovers);
        let picker = popovers.into_iter().find(|p| p.is_mapped() && p != &self.gif_hint);
        let Some(popover) = picker else { return eprintln!("e2e: no GIF picker") };
        let n: usize = arg.and_then(|n| n.parse().ok()).unwrap_or(0);
        let mut all: Vec<gtk::Button> = Vec::new();
        widgets(popover.upcast_ref(), &mut all);
        let face = |b: &gtk::Button| b.child().is_some_and(|c| c.is::<gtk::Picture>() || c.is::<gtk::Image>());
        let gifs: Vec<_> = all.iter().filter(|b| b.has_css_class("flat") && face(b)).collect();
        let tiles: Vec<_> = all.iter().filter(|b| b.has_css_class("flat") && b.child().is_some_and(|c| c.is::<gtk::Overlay>())).collect();
        let stars: Vec<_> = all.iter().filter(|b| b.has_css_class("osd") && b.has_css_class("circular")).collect();
        let press = |list: &[&gtk::Button]| match list.get(n) {
            Some(b) => b.emit_clicked(),
            None => eprintln!("e2e: no {command} {n}"),
        };
        match command {
            "gif-search" => {
                let mut entries: Vec<gtk::SearchEntry> = Vec::new();
                widgets(popover.upcast_ref(), &mut entries);
                match entries.first() {
                    Some(entry) => entry.set_text(arg.unwrap_or_default()),
                    None => eprintln!("e2e: no search entry"),
                }
            }
            "gif-pick" => press(&gifs),
            "gif-shift-pick" => {
                SHIFT.set(true);
                press(&gifs);
                SHIFT.set(false);
            }
            "gif-star" => press(&stars),
            "gif-tile" => press(&tiles),
            "gif-back" => match all.iter().find(|b| b.icon_name().as_deref() == Some("go-previous-symbolic")) {
                Some(b) => b.emit_clicked(),
                None => eprintln!("e2e: no back button"),
            },
            other => eprintln!("e2e: unknown {other}"),
        }
    }
}
""")

# The form's own commands, inside the module so they reach its fields: `modal-fields` prints each field's path,
# kind and title; `modal-set <path> <text>` fills an input; `modal-check <path> <n>` clicks a field's n-th
# check or radio button; `modal-pick <path> <n>` picks a menu's n-th option and closes it; `modal-clear <path>`
# clicks Clear selection; `modal-submit`; `modal-cancel` (Escape); `modal-shot <png>` shoots the top form.
with open(f"{root}/crates/app/src/ui/modal.rs", "a") as f:
    f.write(r"""
fn e2e_all<T: IsA<gtk::Widget>>(widget: &gtk::Widget, found: &mut Vec<T>) {
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Ok(t) = c.clone().downcast::<T>() {
            found.push(t);
        }
        e2e_all(&c, found);
        child = c.next_sibling();
    }
}

impl App {
    pub(super) fn e2e_modal(self: &Rc<Self>, command: &str, rest: &str) {
        let Some(form) = self.modal_forms.borrow().last().cloned() else { return eprintln!("e2e: no form") };
        let mut words = rest.split_whitespace();
        let path = words.next().unwrap_or_default();
        let field = form.fields.iter().find(|f| f.node.path == path);
        let column = || field.and_then(|f| f.focus.parent()).filter(|p| p.is::<gtk::Box>());
        match command {
            "modal-fields" => {
                eprintln!("e2e: form {:?} error {}", form.modal.title, form.error.is_visible());
                for field in &form.fields {
                    let kind = format!("{:?}", field.node.kind);
                    let kind = kind.split(['(', ' ', '{']).next().unwrap_or_default().to_owned();
                    eprintln!("e2e: field {} {kind} {:?}", field.node.path, field.title.label.label().as_str());
                }
                let state = self.account.state.borrow();
                eprintln!("e2e: submission {:?}", state.modal_submission);
            }
            "modal-set" => {
                let text = rest.split_once(' ').map_or("", |(_, text)| text);
                match field.map(|f| f.focus.clone()) {
                    Some(w) if w.is::<gtk::Entry>() => w.downcast::<gtk::Entry>().unwrap().set_text(text),
                    Some(w) if w.is::<gtk::TextView>() => w.downcast::<gtk::TextView>().unwrap().buffer().set_text(text),
                    _ => eprintln!("e2e: no input {path}"),
                }
            }
            "modal-check" => {
                let n: usize = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let mut checks: Vec<gtk::CheckButton> = Vec::new();
                match field.map(|f| f.focus.clone()) {
                    Some(w) if w.is::<gtk::CheckButton>() && column().is_some() => e2e_all(&column().unwrap(), &mut checks),
                    _ => {}
                }
                let radio = matches!(field.map(|f| &f.node.kind), Some(NodeKind::RadioGroup { .. }));
                match checks.get(n) {
                    Some(check) => check.set_active(radio || !check.is_active()),
                    None => eprintln!("e2e: no check {path} {n}"),
                }
            }
            "modal-pick" => {
                let n: usize = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let Some(button) = field.and_then(|f| f.focus.clone().downcast::<gtk::MenuButton>().ok()) else {
                    return eprintln!("e2e: no menu {path}");
                };
                button.popup();
                let Some(popover) = button.popover() else { return eprintln!("e2e: no popover") };
                let mut choices: Vec<gtk::Button> = Vec::new();
                e2e_all(popover.upcast_ref(), &mut choices);
                let mut checks: Vec<gtk::CheckButton> = Vec::new();
                e2e_all(popover.upcast_ref(), &mut checks);
                if let Some(choice) = choices.get(n) {
                    choice.emit_clicked();
                } else if let Some(check) = checks.get(n) {
                    check.set_active(!check.is_active());
                    button.popdown();
                } else {
                    eprintln!("e2e: no option {n}");
                }
            }
            "modal-clear" => {
                let mut buttons: Vec<gtk::Button> = Vec::new();
                if let Some(column) = column() {
                    e2e_all(&column, &mut buttons);
                }
                match buttons.into_iter().find(|b| b.label().as_deref() == Some("Clear selection")) {
                    Some(clear) => clear.emit_clicked(),
                    None => eprintln!("e2e: no clear {path}"),
                }
            }
            "modal-file" | "modal-unfile" | "modal-files" => self.e2e_modal_files(&form, command, rest),
            "modal-submit" => form.submit.emit_clicked(),
            "modal-cancel" => _ = form.dialog.close(),
            "modal-shot" => {
                let png = if path.is_empty() { "modal.png" } else { path };
                super::e2e::shot(&form.dialog, png);
            }
            other => eprintln!("e2e: unknown {other}"),
        }
    }
}
""")

# The app launcher: `apps` presses the Apps button and `apps-visible` says whether it shows; with the launcher
# open, `launcher-shot <png>`, `launcher-search <text>`, `launcher-rows` (the titles of the shown page's rows),
# `launcher-row <title>` (activate it), `launcher-button <tooltip or label>` and `launcher-close`.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(l) if l == "apps" || l == "apps-visible" || l.starts_with("launcher") => self.e2e_launcher(l, rest),
            Some("quit") => {''')
open(f"{root}/crates/app/src/ui/e2e.rs", "a").write(r"""
impl App {
    fn e2e_launcher(self: &Rc<Self>, command: &str, rest: &str) {
        match command {
            "apps" => return self.apps_button.emit_clicked(),
            "apps-visible" => return eprintln!("e2e: apps button visible={}", self.apps_button.is_visible()),
            _ => {}
        }
        let mut popovers: Vec<gtk::Popover> = Vec::new();
        widgets(self.composer_box.upcast_ref(), &mut popovers);
        let nav = |p: &gtk::Popover| p.child().and_downcast::<adw::NavigationView>();
        let Some(popover) = popovers.into_iter().find(|p| p.is_mapped() && nav(p).is_some()) else {
            return eprintln!("e2e: no launcher");
        };
        let Some(page) = nav(&popover).and_then(|n| n.visible_page()) else { return };
        let page: gtk::Widget = page.upcast();
        let mut rows: Vec<adw::ActionRow> = Vec::new();
        widgets(&page, &mut rows);
        let title = |r: &adw::ActionRow| adw::prelude::PreferencesRowExt::title(r).to_string();
        match command {
            "launcher-shot" => shot(&popover, if rest.is_empty() { "launcher.png" } else { rest }),
            "launcher-close" => popover.popdown(),
            "launcher-search" => {
                let mut entries: Vec<gtk::SearchEntry> = Vec::new();
                widgets(&page, &mut entries);
                match entries.first() {
                    Some(entry) => entry.set_text(rest),
                    None => eprintln!("e2e: no search"),
                }
            }
            "launcher-rows" => eprintln!("e2e: rows {:?}", rows.iter().map(title).collect::<Vec<_>>()),
            "launcher-row" => match rows.iter().find(|r| title(r) == rest) {
                Some(row) => adw::prelude::ActionRowExt::activate(row),
                None => eprintln!("e2e: no row {rest}"),
            },
            "launcher-button" => {
                let mut buttons: Vec<gtk::Button> = Vec::new();
                widgets(&page, &mut buttons);
                let named = |b: &gtk::Button| {
                    b.tooltip_text().as_deref() == Some(rest) || b.label().as_deref() == Some(rest)
                };
                match buttons.iter().find(|b| named(b)) {
                    Some(button) => button.emit_clicked(),
                    None => eprintln!("e2e: no button {rest}"),
                }
            }
            other => eprintln!("e2e: unknown {other}"),
        }
    }
}
""")

# A form's file field: `modal-file <path> <file>…` adds files as a pick or a drop does, `modal-unfile <path> <n>`
# takes back its n-th, as its row's remove button does, `modal-files <path> [png]` prints what the field shows.
with open(f"{root}/crates/app/src/ui/modal.rs", "a") as f:
    f.write(r"""
impl App {
    fn e2e_modal_files(self: &Rc<Self>, form: &Rc<Form>, command: &str, rest: &str) {
        let mut words = rest.split_whitespace();
        let path = words.next().unwrap_or_default();
        let field = form.fields.iter().find(|f| f.node.path == path);
        let Some(files) = field.and_then(|f| f._files.clone()) else { return eprintln!("e2e: no file field {path}") };
        match command {
            "modal-file" => {
                let paths: Vec<gio::File> = words.map(gio::File::for_path).collect();
                let this = self.clone();
                glib::spawn_future_local(async move { this.add_form_files(&files, paths).await });
            }
            "modal-unfile" => {
                let n: usize = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                match self.file_ids(&files).get(n) {
                    Some(id) => self.remove_form_file(&files, &id.clone()),
                    None => eprintln!("e2e: no file {path} {n}"),
                }
            }
            _ => {
                let mut labels: Vec<gtk::Label> = Vec::new();
                e2e_all(files.holder.upcast_ref(), &mut labels);
                let shown: Vec<String> = labels.iter().filter(|l| l.is_visible()).map(|l| l.label().to_string()).collect();
                eprintln!("e2e: files {path} {:?} ids {:?}", shown, self.file_ids(&files));
                if let Some(png) = words.next().map(str::to_owned) {
                    // Scrolled to the bottom, where the fake's file field is, then shot once drawn.
                    let scroller = files.holder.ancestor(gtk::ScrolledWindow::static_type());
                    if let Some(scroller) = scroller.and_downcast::<gtk::ScrolledWindow>() {
                        let adjustment = scroller.vadjustment();
                        adjustment.set_value(adjustment.upper());
                    }
                    let dialog = form.dialog.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                        super::e2e::shot(&dialog, &png);
                    });
                }
            }
        }
    }
}
""")

# `end` scrolls the chat to its newest message.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("end") => self.scroll_to_end(),
            Some("quit") => {''')

# Stickers' images come from the fake (`Sticker::image`, before model.rs's tests).
edit("crates/discord/src/model.rs", '"https://media.discordapp.net/stickers/{}', '"http://127.0.0.1:8099/attachments/stickers/{}')

# `/gif`'s and `/sticker`'s results: `results [png]` prints the popup's title and each result, `*` on the selected
# one, and shoots it; `result-click <n>` clicks result n.
edit("crates/app/src/ui/e2e.rs", '            Some("line") => self.e2e_line(),',
     '            Some("line") => self.e2e_line(),\n'
     '            Some("results") => self.e2e_results(parts.next()),\n'
     '            Some("result-click") => self.e2e_result_click(parts.next().and_then(|n| n.parse().ok()).unwrap_or(0)),')
with open(f"{root}/crates/app/src/ui/integrations.rs", "a") as f:
    f.write(r"""
impl App {
    pub(super) fn e2e_results(&self, png: Option<&str>) {
        let view = &self.integration;
        if !view.popover.is_visible() {
            return eprintln!("e2e: results hidden");
        }
        let selected = self.slash.active.borrow().as_ref().and_then(|a| a.selection.selected());
        let title = if view.title.is_visible() { view.title.text().to_string() } else { String::new() };
        eprintln!("e2e: results {title:?} spinner {}", view.spinner.is_visible());
        let rows: Vec<String> = match &*view.shown.borrow() {
            Shown::Gifs(gifs) => gifs.iter().map(|g| format!("{} {:?}", g.url, g.command_preview())).collect(),
            Shown::Stickers(hits) => hits
                .iter()
                .map(|h| format!("{} | {:?} | {:?}", h.entry.listed.sticker.name, h.similar_to(), h.source))
                .collect(),
            Shown::Nothing => Vec::new(),
        };
        for (i, row) in rows.iter().enumerate() {
            let mark = if selected == Some(i) { " *" } else { "" };
            eprintln!("e2e: result {i} {row}{mark}");
        }
        if let Some(png) = png {
            super::e2e::shot(&view.popover, png);
        }
    }

    pub(super) fn e2e_result_click(&self, n: i32) {
        let view = &self.integration;
        // What is shown is read first: the click changes it.
        let gifs = matches!(&*view.shown.borrow(), Shown::Gifs(_));
        let stickers = matches!(&*view.shown.borrow(), Shown::Stickers(_));
        if gifs {
            view.gifs.child_at_index(n).map(|c| c.activate());
        } else if stickers {
            view.stickers.row_at_index(n).map(|r| r.activate());
        }
    }
}
""")

# `link <url>` clicks the link to `url` in a message shown, as its label is told to follow it, and says whether
# Neucord followed it (else the browser would open it).
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("link") => {
                fn labels(widget: &gtk::Widget, found: &mut Vec<gtk::Label>) {
                    let mut child = widget.first_child();
                    while let Some(c) = child {
                        if let Ok(label) = c.clone().downcast::<gtk::Label>() {
                            found.push(label);
                        }
                        labels(&c, found);
                        child = c.next_sibling();
                    }
                }
                let url = parts.next().unwrap_or_default();
                let mut found = Vec::new();
                labels(self.window.upcast_ref(), &mut found);
                let href = format!("href=\\"{url}\\"");
                match found.into_iter().find(|l| l.uses_markup() && l.label().contains(&href)) {
                    Some(label) => {
                        let followed = label.emit_by_name::<bool>("activate-link", &[&url]);
                        eprintln!("e2e: link {}", if followed { "followed" } else { "left to the browser" });
                    }
                    None => eprintln!("e2e: no link to {url}"),
                }
            }
            Some("quit") => {''')

# `paste` pastes the clipboard into the message box, as Ctrl+V does.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("paste") => self.composer.emit_paste_clipboard(),
            Some("quit") => {''')

# Voice messages need a microphone, which the scratch session has none of: `voice-draft <seconds>` puts a made-up
# recording in the open channel as the recorder would have left it, `voice-play` clicks the player's button,
# `voice-recording <s>` begins a recording s seconds ago (`-elsewhere`: in #general, whichever channel is open), `voice-record` starts one as Record Voice Message does,
# `voice-blink <s>` logs the clock's colour, `voice-scrub <0..1>` presses the waveform there and lets go,
# `voice-state` prints the player, `voice-finishing`
# makes the draft one whose last packets are still coming.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(v) if v.starts_with("voice") => self.e2e_voice(v, parts.next()),
            Some("quit") => {''')
open(f"{root}/crates/app/src/ui/voice.rs", "a").write(r"""
impl App {
    pub(super) fn e2e_voice(self: &Rc<Self>, command: &str, arg: Option<&str>) {
        let number = arg.and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0);
        match command {
            "voice-draft" => {
                let Some(channel) = self.account.state.borrow().channel else { return };
                let id = self.voice.ids.get() + 1;
                self.voice.ids.set(id);
                self.voice.levels.borrow_mut().clear();
                self.voice.draft.replace(Some(Draft {
                    id,
                    channel,
                    phase: Phase::Requesting,
                    recorder: None,
                    capture: None,
                    started: 0,
                    millis: 0,
                    recording: None,
                    send_when_done: false,
                    attempts: Rc::default(),
                }));
                self.voice_event(id, Event::Started);
                for n in 0..(number * 50.0) as usize {
                    // The silence packet of a recording, and a loudness that swells and falls.
                    self.voice_event(id, Event::Packet(vec![0xf8, 0xff, 0xfe]));
                    let swell = (((n as f64) / 12.0).sin().abs() * 20000.0) as i16;
                    self.voice_event(id, Event::Samples(vec![swell; 960]));
                }
                self.voice_event(id, Event::Ended);
                self.voice_ended(id);
            }
            // A recording that began `number` seconds ago in the open channel, with nothing behind it.
            "voice-recording" | "voice-recording-elsewhere" => {
                let Some(open) = self.account.state.borrow().channel else { return };
                let channel = if command == "voice-recording" { open } else { Snowflake(600) };
                let id = self.voice.ids.get() + 1;
                self.voice.ids.set(id);
                self.voice.levels.borrow_mut().clear();
                self.voice.draft.replace(Some(Draft {
                    id,
                    channel,
                    phase: Phase::Recording,
                    recorder: None,
                    capture: Some(Capture::new(0)),
                    started: glib::monotonic_time() - (number * 1e6) as i64,
                    millis: 0,
                    recording: None,
                    send_when_done: false,
                    attempts: Rc::default(),
                }));
                self.voice_started(id);
            }
            // Start a recording as Record Voice Message does.
            "voice-record" => self.record_voice_message(),
            // The clock's warning colour, every 100 ms for `number` seconds.
            "voice-blink" => {
                let (this, n) = (Rc::downgrade(self), std::cell::Cell::new(0));
                glib::timeout_add_local(Duration::from_millis(100), move || {
                    let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
                    n.set(n.get() + 1);
                    let (elapsed, error) = (
                        this.voice.draft.borrow().as_ref().map(|d| (glib::monotonic_time() - d.started) / 1000),
                        this.voice.time.has_css_class("error"),
                    );
                    eprintln!("e2e: blink {:>3} elapsed={elapsed:?} error={error}", n.get());
                    if f64::from(n.get()) / 10.0 >= number { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
                });
            }
            // As if it had just been stopped and its last packets were on their way.
            "voice-finishing" => {
                if let Some(draft) = self.voice.draft.borrow_mut().as_mut() {
                    draft.phase = Phase::Finishing;
                }

            }
            "voice-play" => self.voice.play.emit_clicked(),
            "voice-scrub" => {
                let width = f64::from(self.voice.wave.width());
                self.scrub(number * width, Scrub::Begin);
                self.scrub(number * width, Scrub::End);
            }
            "voice-state" => {
                let playback = self.voice.playback.borrow();
                match playback.as_ref() {
                    Some(p) => eprintln!(
                        "e2e: voice player {:?} stream playing={} ended={} error={:?} time={:?} play-visible={}",
                        p.player, p.stream.is_playing(), p.stream.is_ended(), p.stream.error().map(|e| e.to_string()),
                        self.voice.time.label(), self.voice.play.is_visible()
                    ),
                    None => eprintln!("e2e: no voice player"),
                }
            }
            _ => {}
        }
    }
}
""")
# `clip-text <n>` puts n letters on the clipboard as text, `clip-page-image <src>` an image with the HTML a browser
# copies with it (an `<img>` of that address), and `box-length` prints how long the message box's text is (#260).
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("clip-text") => {
                let n: usize = parts.next().and_then(|n| n.parse().ok()).unwrap_or(10);
                self.composer.clipboard().set_text(&"a".repeat(n));
            }
            Some("clip-page-image") => {
                let src = parts.next().unwrap_or("https://example.com/pic.jpg");
                let bytes = glib::Bytes::from_owned([255u8, 0, 0, 255].repeat(4));
                let texture = gdk::MemoryTexture::new(2, 2, gdk::MemoryFormat::R8g8b8a8, &bytes, 8);
                let texture: gdk::Texture = texture.upcast();
                let html = format!("<meta charset='utf-8'><img src=\\"{src}\\" alt=\\"\\">");
                let provider = gdk::ContentProvider::new_union(&[
                    gdk::ContentProvider::for_value(&texture.to_value()),
                    gdk::ContentProvider::for_bytes("text/html", &glib::Bytes::from_owned(html.into_bytes())),
                ]);
                if let Err(e) = self.composer.clipboard().set_content(Some(&provider)) {
                    eprintln!("e2e: {e}");
                }
            }
            Some("box-length") => {
                let buffer = self.composer.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                eprintln!("e2e: box {}", text.chars().count());
            }
            Some("quit") => {''')

# A voice message in a channel (run with FAKE_VOICE=1): `vmsg state` prints its player,
# `vmsg play|pause` press its button, `vmsg scrub <0..1>` presses the waveform there and lets go.
edit("crates/app/src/ui/attachments.rs", "    wave.set_draw_func({\n        let voice = voice.clone();",
     "    E2E_VOICES.with(|v| v.borrow_mut().push(voice.clone()));\n    wave.set_draw_func({\n        let voice = voice.clone();")
open(f"{root}/crates/app/src/ui/attachments.rs", "a").write(r"""
thread_local! {
    static E2E_VOICES: RefCell<Vec<Rc<VoiceMessage>>> = RefCell::default();
}

pub(super) fn e2e_voice_message(what: &str, arg: Option<&str>) {
    let Some(voice) = E2E_VOICES.with(|v| v.borrow().last().cloned()) else { return eprintln!("e2e: no voice message") };
    let number = arg.and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0);
    match what {
        "state" => eprintln!(
            "e2e: vmsg {:?} stream playing={} time={:?}",
            voice.player.borrow(),
            voice.stream.borrow().as_ref().is_some_and(|s| s.is_playing()),
            voice.time.upgrade().map(|t| t.label())
        ),
        "play" | "pause" => {
            if voice.player.borrow().is_playing() == (what == "pause") {
                voice.toggle();
            }
        }
        "scrub" => {
            let width = voice.wave.upgrade().map_or(0.0, |w| f64::from(w.width()));
            voice.scrub(number * width, Scrub::Begin);
            voice.scrub(number * width, Scrub::End);
        }
        _ => {}
    }
}
""")
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("vmsg") => super::attachments::e2e_voice_message(parts.next().unwrap_or_default(), parts.next()),
            Some("quit") => {''')
# The keyboard's menu on a sidebar row or a rail button: `menu-key row <n> [png]` and `menu-key rail <n> [png]` give
# the nth one the focus and run its Menu / Shift+F10 shortcut (the keys themselves can't be sent here), then say
# whether a menu opened on it and shoot it.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("menu-key") => {
                let (kind, n) = (parts.next().unwrap_or_default(), parts.next().and_then(|n| n.parse::<usize>().ok()));
                let has_keys = |w: &gtk::Widget| {
                    let controllers = w.observe_controllers();
                    (0..controllers.n_items()).any(|i| controllers.item(i).is_some_and(|c| c.is::<gtk::ShortcutController>()))
                };
                let target: Option<gtk::Widget> = match kind {
                    "row" => {
                        let mut rows = Vec::new();
                        widgets::<gtk::ListBoxRow>(self.window.upcast_ref(), &mut rows);
                        rows.into_iter().map(|r| r.upcast::<gtk::Widget>()).filter(|r| has_keys(r)).nth(n.unwrap_or(0))
                    }
                    _ => {
                        let mut found = Vec::new();
                        buttons(self.window.upcast_ref(), &mut found);
                        found.into_iter().map(|b| b.upcast::<gtk::Widget>()).filter(|b| b.has_css_class("guild") && has_keys(b)).nth(n.unwrap_or(0))
                    }
                };
                let Some(target) = target else { return eprintln!("e2e: no {kind} {n:?}") };
                target.grab_focus();
                let controllers = target.observe_controllers();
                let mut ran = false;
                for i in 0..controllers.n_items() {
                    let Some(shortcuts) = controllers.item(i).and_downcast::<gtk::ShortcutController>() else { continue };
                    for j in 0..shortcuts.n_items() {
                        let Some(shortcut) = shortcuts.item(j).and_downcast::<gtk::Shortcut>() else { continue };
                        let Some(action) = shortcut.action() else { continue };
                        ran |= action.activate(gtk::ShortcutActionFlags::empty(), &target, None);
                    }
                }
                let (png, what) = (parts.next().map(str::to_owned), kind.to_owned());
                // The menu maps on the next turn of the loop.
                glib::timeout_add_local_once(Duration::from_millis(300), move || {
                    let popover = popover_on(&target);
                    eprintln!("e2e: menu-key {what} ran={ran} opened={} on {:?} {:?}", popover.is_some(), target.css_classes(), target.widget_name());
                    if let Some(popover) = popover {
                        // With a picture the menu is shot and closed; without, it is left open.
                        if let Some(png) = png {
                            shot(&popover, &png);
                            popover.popdown();
                        }
                    }
                });
            }
            Some("quit") => {''')

# With E2E_NO_PLAYER=1 a draft gets no player: a media stream keeps a window shot from finishing in this session.
edit("crates/app/src/ui/voice.rs", "    fn start_playback(self: &Rc<Self>, id: u64, recording: &Recording) {\n",
     "    fn start_playback(self: &Rc<Self>, id: u64, recording: &Recording) {\n        if std::env::var_os(\"E2E_NO_PLAYER\").is_some() {\n            return;\n        }\n")
# `FAKE_EMOJI_IMAGES=1` serves custom emoji from the fake (`/attachments/emojis/<id>.png`, a stand-in picture), which
# the CDN would, so the picker and the packs page draw them.
if os.environ.get("FAKE_EMOJI_IMAGES"):
    edit("crates/discord/src/cdn.rs", 'format!("{CDN}/emojis/{emoji_id}.{ext}?size={size}")',
         'format!("http://127.0.0.1:8099/attachments/emojis/{emoji_id}.{ext}?width={}&height={}", u32::from(size) * 4, u32::from(size) * 4)')
# `FAKE_HOVER=1` shows every row's hover bar, which a pointer would only show on the row it is over.
if os.environ.get("FAKE_HOVER"):
    edit("crates/app/src/style.css", ".message .message-action { opacity: 0;", ".message .message-action { opacity: 1;")

# A friend's profile popout, as the pointer opens it: `profile-press <user id>` is the pointer going down on their
# name, `profile-click <user id>` the click that opens the popout.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(p) if p == "profile-press" || p == "profile-click" => self.e2e_profile(p, parts.next()),
            Some("quit") => {''')
open(f"{root}/crates/app/src/ui/profile.rs", "a").write(r"""
impl App {
    pub(super) fn e2e_profile(self: &Rc<Self>, command: &str, arg: Option<&str>) {
        let id = Snowflake(arg.and_then(|a| a.parse().ok()).unwrap_or(0));
        let user = self.account.state.borrow().relationships.get(id).map(|r| r.user.clone());
        let Some(user) = user else { return eprintln!("e2e: no friend {id:?}") };
        let target = UserTarget { user, guild: None, channel: None, presence: None, anchor: self.window.clone().upcast() };
        match command {
            "profile-press" => self.prefetch_profile(&target),
            _ => self.show_profile(&target, false),
        }
    }
}
""")

# A thread's settings form: `thread-settings <id>` opens it (whoever owns the thread), `thread-settings-save <name>
# <slowmode index> <archive index>` fills it and presses Save Changes.
edit("crates/app/src/ui/thread_settings.rs", "        form.dialog.present(Some(&self.window));",
     "        E2E_FORM.with(|f| *f.borrow_mut() = Some(form.clone()));\n        form.dialog.present(Some(&self.window));")
open(f"{root}/crates/app/src/ui/thread_settings.rs", "a").write(r"""
thread_local! {
    static E2E_FORM: std::cell::RefCell<Option<Rc<Form>>> = std::cell::RefCell::default();
}

impl App {
    pub(super) fn e2e_thread_settings(self: &Rc<Self>, command: &str, rest: &str) {
        let mut words = rest.split_whitespace();
        match command {
            "thread-settings" => {
                let id = Snowflake(words.next().and_then(|w| w.parse().ok()).unwrap_or(0));
                {
                    let state = self.account.state.borrow();
                    eprintln!("e2e: thread known={} guild={} account={}", state.find_channel(id).is_some(),
                              state.find_channel(id).is_some_and(|(g, _)| g.is_some()), state.account.is_some());
                }
                self.open_thread_settings(id);
            }
            _ => {
                let Some(form) = E2E_FORM.with(|f| f.borrow().clone()) else { return eprintln!("e2e: no form") };
                let name = words.next().unwrap_or_default().replace('_', " ");
                form.name.set_text(&name);
                form.slowmode.set_selected(words.next().and_then(|w| w.parse().ok()).unwrap_or(0));
                form.archive.set_selected(words.next().and_then(|w| w.parse().ok()).unwrap_or(0));
                form.save.emit_clicked();
            }
        }
    }
}
""")
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(c) if c == "thread-settings" || c == "thread-settings-save" => self.e2e_thread_settings(c, rest),
            Some("quit") => {''')

# Forward: the picker, driven as hands would. `forward-menu <text> <png>` opens the menu of the message with <text>
# and shoots it; `forward-open <text>` opens the picker on it (`forward-open <text> @<attachment id>` for one file);
# `forward-viewer <text>` opens the media viewer on its first picture and `forward-viewer-click` presses its Forward;
# `forward-rows`, `-state`, `-search <text>`, `-pick <n>`, `-comment <text>` and `-send` work the picker.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("forward-menu") => {
                let (needle, path) = (parts.next().unwrap_or_default().replace('_', " "), parts.next().unwrap_or_default().to_owned());
                let Some((row, message)) = self.e2e_message(&needle) else { return eprintln!("e2e: no message {needle}") };
                self.message_menu(&row.parts().row, message, At::Point(10.0, 10.0), Clicked::default());
                let anchor = row.parts().row.clone().upcast::<gtk::Widget>();
                glib::timeout_add_local_once(Duration::from_millis(600), move || match popover_above(&anchor) {
                    Some(popover) => shot(&popover, &path),
                    None => eprintln!("e2e: no popover on the row"),
                });
            }
            Some(c) if c.starts_with("forward-") => {
                let rest: Vec<&str> = parts.collect();
                self.e2e_forward(c, &rest.join(" "));
            }
            Some("quit") => {''')
open(f"{root}/crates/app/src/ui/forward.rs", "a").write(r"""
thread_local! {
    static E2E_PICKER: RefCell<Option<Rc<Picker>>> = const { RefCell::new(None) };
}

impl App {
    pub(super) fn e2e_message(&self, needle: &str) -> Option<(super::message_row::MessageRow, Message)> {
        let rows: Vec<_> = self.rows.borrow().iter().filter_map(|r| r.upgrade()).collect();
        for row in rows {
            let Some(item) = row.parts().item.upgrade() else { continue };
            let Some(message) = item.item().and_downcast::<glib::BoxedAnyObject>() else { continue };
            let message = message.borrow::<Message>().clone();
            if message.content.contains(needle) {
                return Some((row, message));
            }
        }
        None
    }

    pub(super) fn e2e_forward(self: &Rc<Self>, command: &str, rest: &str) {
        let picker = E2E_PICKER.with(|p| p.borrow().clone());
        match (command, picker) {
            ("forward-open", _) => {
                let (needle, only) = match rest.split_once(" @") {
                    Some((needle, id)) => (needle, id.parse().ok().map(|id| Only { attachments: Some(vec![Snowflake(id)]), embeds: None })),
                    None => (rest, None),
                };
                let Some((_, message)) = self.e2e_message(needle) else { return eprintln!("e2e: no message {needle}") };
                self.open_forward(message, only.unwrap_or_default(), Vec::new(), None);
            }
            ("forward-viewer", _) => {
                let Some((_, message)) = self.e2e_message(rest) else { return eprintln!("e2e: no message {rest}") };
                let settings = self.account.state.borrow().settings.media;
                let items = discord::media::items(&message, &settings, false);
                let held = discord::embed::visible(&message, &settings, false).len() == message.embeds.len();
                super::viewer::MediaCx::new(self, &message, items, held).open(0, 0.0);
            }
            ("forward-viewer-click", _) => self.viewer.e2e_forward(),
            ("forward-rows", Some(p)) => {
                let rows: Vec<String> = p
                    .shown
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(i, h)| format!("{} {:?} {} sensitive={}", h.comparator, h.kind, h.id.0, p.list.row_at_index(i as i32).is_some_and(|r| r.is_sensitive())))
                    .collect();
                eprintln!("e2e: forward rows {rows:?}");
            }
            ("forward-state", Some(p)) => eprintln!(
                "e2e: forward selected={:?} send={:?} sensitive={} subtitle={:?} slowmode={:?}/{}",
                p.selected.borrow().iter().map(|d| match d { Destination::User(i) | Destination::Channel(i) => i.0 }).collect::<Vec<_>>(),
                p.send.label(), p.send.is_sensitive(), p.subtitle.label(), p.slowmode.label(), p.slowmode.is_visible(),
            ),
            ("forward-search", Some(p)) => p.entry.set_text(rest),
            ("forward-pick", Some(p)) => p.toggle(self, rest.parse().unwrap_or(0)),
            ("forward-comment", Some(p)) => p.comment.set_text(rest),
            ("forward-send", Some(p)) => p.send.emit_clicked(),
            (other, _) => eprintln!("e2e: no forward command {other} (or no picker)"),
        }
    }
}
""")
edit("crates/app/src/ui/forward.rs", "        picker.dialog.present(Some(&self.window));\n        picker.refresh(self);\n", "        picker.dialog.present(Some(&self.window));\n        picker.refresh(self);\n        E2E_PICKER.with(|p| *p.borrow_mut() = Some(picker));\n")
open(f"{root}/crates/app/src/ui/viewer.rs", "a").write(r"""
impl Viewer {
    pub(super) fn e2e_forward(&self) {
        eprintln!("e2e: viewer forward visible={}", self.0.forward.is_visible());
        self.0.forward.emit_clicked();
    }
}
""")
edit("crates/app/src/ui/viewer.rs", "    pub(super) fn open(&self, index: usize, start: f64) {", "    pub(super) fn open(&self, index: usize, start: f64) {")
# The settings window: `settings <page>` opens it on a page by its name (`activity` is Activity Privacy),
# `activity-share on|off` saves "Share my activity", and `activity-default <1..3>` saves the default (as a pick of its row).
edit("crates/app/src/ui/settings.rs", "        dialog.add(&activity);\n", "        activity.set_name(Some(\"activity\"));\n        dialog.add(&activity);\n")
open(f"{root}/crates/app/src/ui/settings.rs", "a").write(r"""
impl App {
    pub(super) fn e2e_settings(self: &Rc<Self>, command: &str, arg: &str) {
        match command {
            "settings" => {
                self.show_settings();
                if let Some(window) = self.settings_window.borrow().as_ref() {
                    window.dialog.set_visible_page_name(arg);
                }
            }
            "activity-share" => self.change_setting(Setting::ShowCurrentGame(arg == "on")),
            _ => {
                let default = match arg {
                    "1" => discord::settings::ActivityDefault::Off,
                    "2" => discord::settings::ActivityDefault::LargeServers,
                    _ => discord::settings::ActivityDefault::On,
                };
                self.set_activity_default(default);
            }
        }
    }
}
""")
edit("crates/app/src/ui/activity_privacy.rs", "    fn set_activity_default(self: &Rc<Self>", "    pub(super) fn set_activity_default(self: &Rc<Self>")
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(c) if c == "settings" || c == "activity-share" || c == "activity-default" => self.e2e_settings(c, rest),
            Some("quit") => {''')

# Focus Mode: `focus-mode <ms>|forever|off` as a pick of its submenu, `status-menu <png>` opens the status menu and shoots
# it, `focus-state` says whether Focus Mode is on and what it ends.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some("focus-mode") => {
                let what = parts.next().unwrap_or("off");
                match what {
                    "off" => self.set_focus_mode_for_e2e(false, None),
                    "forever" => self.set_focus_mode_for_e2e(true, None),
                    ms => self.set_focus_mode_for_e2e(true, ms.parse().ok()),
                }
            }
            Some("focus-state") => {
                let state = self.account.state.borrow();
                let now = glib::real_time() as u64 / 1000;
                eprintln!("e2e: focus on={} silenced={} ends_in_ms={:?} quiet={}",
                          state.settings.focus_mode(now), state.settings.silenced(now),
                          state.settings.notifications.focus_expires_at_ms.checked_sub(now),
                          state.settings.notifications.quiet_mode);
            }
            Some("status-menu") => {
                let png = parts.next().unwrap_or("status.png").to_owned();
                let button = self.status_button.borrow().clone();
                if let Some(button) = button {
                    button.popup();
                    glib::timeout_add_local_once(Duration::from_millis(400), move || {
                        if let Some(popover) = button.popover() {
                            shot(&popover, &png);
                        }
                    });
                }
            }
            Some("quit") => {''')
edit("crates/app/src/ui/own_status.rs", "    fn set_focus_mode(self: &Rc<Self>", "    pub(super) fn set_focus_mode_for_e2e(self: &Rc<Self>, on: bool, duration_ms: Option<u64>) {\n        self.set_focus_mode(on, duration_ms);\n    }\n\n    fn set_focus_mode(self: &Rc<Self>")

# The age gate: `gate-press <label>` presses the button with that label on the gate page, `gate-state` says which
# page is showing and what its buttons are.
edit("crates/app/src/ui/e2e.rs", '            Some("quit") => {', '''            Some(c) if c.starts_with("gate-") => self.e2e_gate(c, &parts.collect::<Vec<_>>().join(" ")),
            Some("quit") => {''')
edit("crates/app/src/ui/age_gate.rs", "        column.append(&buttons);", "        column.append(&buttons);\n        E2E_BUTTONS.with(|b| *b.borrow_mut() = std::iter::successors(buttons.first_child(), |w| w.next_sibling()).filter_map(|w| w.downcast::<gtk::Button>().ok()).collect());")
open(f"{root}/crates/app/src/ui/age_gate.rs", "a").write(r"""
thread_local! {
    static E2E_BUTTONS: std::cell::RefCell<Vec<gtk::Button>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl App {
    pub(super) fn e2e_gate(self: &Rc<Self>, command: &str, rest: &str) {
        let buttons = E2E_BUTTONS.with(|b| b.borrow().clone());
        match command {
            "gate-state" => eprintln!(
                "e2e: gate page={:?} buttons={:?} channel={:?}",
                self.content.visible_child_name(),
                buttons.iter().filter_map(|b| b.label()).collect::<Vec<_>>(),
                self.account.state.borrow().channel,
            ),
            _ => match buttons.iter().find(|b| b.label().as_deref() == Some(rest)) {
                Some(button) => button.emit_clicked(),
                None => eprintln!("e2e: no gate button {rest}"),
            },
        }
    }
}
""")

