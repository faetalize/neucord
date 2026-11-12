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
open(f"{root}/crates/app/src/ui/e2e.rs", "w").write('''
use std::rc::Rc;
use std::time::Duration;

use discord::Snowflake;
use gtk::glib;
use gtk::prelude::*;

use super::App;

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

impl App {
    pub(super) fn e2e_start(self: &Rc<Self>) {
        let token = std::env::var("E2E_TOKEN").unwrap_or_else(|_| "fake-token-1".into());
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
                    this.e2e_run(line.trim());
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn e2e_run(self: &Rc<Self>, line: &str) {
        eprintln!("e2e: {line}");
        let mut parts = line.split_whitespace();
        let id = |s: Option<&str>| Snowflake(s.and_then(|s| s.parse().ok()).unwrap_or(0));
        match parts.next() {
            Some("channel") => self.select_channel(id(parts.next())),
            Some("guild") => self.select_guild(id(parts.next())),
            Some("home") => self.select_home(),
            Some("attach") => self.drafts.attach.emit_clicked(),
            Some("shot") => shot(&self.window, parts.next().unwrap_or("shot.png")),
            Some("popover") => {
                let path = parts.next().unwrap_or("popover.png");
                let mut child = self.composer_box.first_child();
                while let Some(c) = child {
                    if let Ok(p) = c.clone().downcast::<gtk::Popover>() {
                        return shot(&p, path);
                    }
                    child = c.next_sibling();
                }
                eprintln!("e2e: no popover");
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
''')
