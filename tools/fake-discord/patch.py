#!/usr/bin/env python3
"""Point a scratch copy of Neucord at the loopback fake and give it a command file."""
import re, sys
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
edit("crates/app/src/ui/mod.rs", "mod emoji;", "mod e2e;\nmod emoji;")
edit("crates/app/src/ui/mod.rs", "glib::spawn_future_local(async move { this.resume_session().await });",
     "let _ = &this; self.e2e_start();")
open(f"{root}/crates/app/src/ui/e2e.rs", "w").write(r"""
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::Snowflake;
use gtk::glib;

use super::App;
use super::composer::Completion;

fn shot(widget: &impl IsA<gtk::Widget>, path: &str) {
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
            Some("shot") => shot(&self.window, parts.next().unwrap_or("shot.png")),
            Some("shot-popover") => {
                let anchor: Option<gtk::Widget> = match parts.next() {
                    Some("composer") => Some(self.composer.clone().upcast()),
                    Some("composer_box") => Some(self.composer_box.clone().upcast()),
                    Some("gif_button") => Some(self.gif_button.clone().upcast()),
                    other => return eprintln!("e2e: no widget {other:?}"),
                };
                match anchor.and_then(|w| popover_on(&w)) {
                    Some(popover) => shot(&popover, parts.next().unwrap_or("popover.png")),
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
                        Completion::Mention(_, name) => format!("@{name}"),
                        Completion::Emoji(item) => format!(":{}:", item.name()),
                        Completion::Command(s) => format!("{} ({})", s.name, s.app),
                    })
                    .collect();
                eprintln!("e2e: suggestions {}", rows.len());
                for (i, row) in rows.iter().enumerate() {
                    let mark = if selected == Some(i as i32) { " *" } else { "" };
                    eprintln!("e2e: suggestion {i} {row}{mark}");
                }
                if let Some(path) = parts.next().filter(|_| shown) {
                    shot(&self.mention_popover, path);
                }
            }
            Some("pick") => self.accept_mention(parts.next().and_then(|n| n.parse().ok()).unwrap_or(0)),
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
