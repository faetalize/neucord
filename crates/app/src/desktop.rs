//! The desktop's notification server and launcher, spoken to over D-Bus
//! directly. GNotification can't carry what the official client's
//! notifications have here: a reply typed into the notification itself
//! (`inline-reply`, where the server has it), a sound the server plays from
//! the sound theme (or is told not to), and a notification of ours that a
//! newer one replaces in place.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

const SERVER: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const LAUNCHER_INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
/// The sound theme's name for a message arriving.
const SOUND: &str = "message-new-instant";

/// Ask the server to take down notification `id`.
fn close(bus: &gio::DBusConnection, id: u32) {
    let call = bus.call_future(
        Some(SERVER),
        PATH,
        SERVER,
        "CloseNotification",
        Some(&(id,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
    );
    glib::spawn_future_local(async move {
        let _ = call.await;
    });
}

/// A notification to show.
pub struct Notice<'a> {
    /// Names the notification: showing another with it replaces the first.
    pub key: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub icon: Option<&'a gdk::Texture>,
    /// Whether the server plays its sound for it.
    pub sound: bool,
    /// Whether it takes a reply, on a server that has inline replies.
    pub reply: bool,
}

/// What happened to a notification of ours.
pub enum Event {
    /// It was clicked.
    Opened,
    /// The text typed into it.
    Replied(String),
}

pub struct Desktop {
    bus: gio::DBusConnection,
    app_id: &'static str,
    app_name: &'static str,
    /// The server's id for each key's notification, while it shows.
    ids: Rc<RefCell<HashMap<String, u32>>>,
    /// The latest `show` of each key not since withdrawn. The server answers
    /// a `Notify` later than it was sent: an answer to anything else is for a
    /// notification nobody wants any more.
    wanted: Rc<RefCell<HashMap<String, u64>>>,
    shows: Cell<u64>,
    /// What the server says it can do, once it has.
    capabilities: Rc<RefCell<Option<Vec<String>>>>,
    /// The count last given to the launcher.
    launcher: Cell<Option<u32>>,
    _signals: Vec<gio::SignalSubscription>,
}

impl Desktop {
    /// Listens to the server on `bus` and tells `on_event` what happens to
    /// notifications by key.
    pub fn new(
        bus: gio::DBusConnection,
        app_id: &'static str,
        app_name: &'static str,
        on_event: impl Fn(&str, Event) + 'static,
    ) -> Self {
        let ids: Rc<RefCell<HashMap<String, u32>>> = Rc::default();
        let capabilities: Rc<RefCell<Option<Vec<String>>>> = Rc::default();
        let on_event = Rc::new(on_event);
        let key_of = {
            let ids = ids.clone();
            move |id: u32| ids.borrow().iter().find(|(_, shown)| **shown == id).map(|(key, _)| key.clone())
        };
        let subscribe = |member: &str, handle: Box<dyn Fn(&glib::Variant)>| {
            bus.subscribe_to_signal(
                Some(SERVER),
                Some(SERVER),
                Some(member),
                Some(PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |signal| handle(signal.parameters),
            )
        };
        let signals = vec![
            subscribe("ActionInvoked", {
                let (key_of, on_event) = (key_of.clone(), on_event.clone());
                Box::new(move |parameters| {
                    if let Some((id, action)) = parameters.get::<(u32, String)>()
                        && action == "default"
                        && let Some(key) = key_of(id)
                    {
                        on_event(&key, Event::Opened);
                    }
                })
            }),
            // KDE's reply box: the text goes out with the notification's id.
            subscribe("NotificationReplied", {
                let (key_of, on_event) = (key_of.clone(), on_event.clone());
                Box::new(move |parameters| {
                    if let Some((id, text)) = parameters.get::<(u32, String)>()
                        && let Some(key) = key_of(id)
                    {
                        on_event(&key, Event::Replied(text));
                    }
                })
            }),
            subscribe("NotificationClosed", {
                let ids = ids.clone();
                Box::new(move |parameters| {
                    if let Some((id, _reason)) = parameters.get::<(u32, u32)>() {
                        ids.borrow_mut().retain(|_, shown| *shown != id);
                    }
                })
            }),
        ];
        let desktop = Desktop {
            bus,
            app_id,
            app_name,
            ids,
            wanted: Rc::default(),
            shows: Cell::new(0),
            capabilities,
            launcher: Cell::new(None),
            _signals: signals,
        };
        desktop.load_capabilities();
        desktop
    }

    fn load_capabilities(&self) {
        let (bus, capabilities) = (self.bus.clone(), self.capabilities.clone());
        glib::spawn_future_local(async move {
            capabilities_of(&bus, &capabilities).await;
        });
    }

    /// Show `notice`, in place of the one with its key, if one shows.
    pub fn show(&self, notice: &Notice) {
        let replaces = self.ids.borrow().get(notice.key).copied().unwrap_or(0);
        let (bus, ids, capabilities) = (self.bus.clone(), self.ids.clone(), self.capabilities.clone());
        let show = self.shows.get() + 1;
        self.shows.set(show);
        self.wanted.borrow_mut().insert(notice.key.to_owned(), show);
        let wanted = self.wanted.clone();
        let (app_name, app_id) = (self.app_name, self.app_id);
        let owned = (notice.key.to_owned(), notice.title.to_owned(), notice.body.to_owned());
        let (icon, sound, reply) = (notice.icon.cloned(), notice.sound, notice.reply);
        glib::spawn_future_local(async move {
            // The first notification may come before the server has said what it can do.
            let known = capabilities_of(&bus, &capabilities).await;
            let reply = reply && known.iter().any(|c| c == "inline-reply");
            let (key, title, body) = &owned;
            let notice = Notice { key, title, body, icon: icon.as_ref(), sound, reply };
            let parameters = arguments(app_name, app_id, replaces, &notice, reply);
            let call = bus.call_future(
                Some(SERVER),
                PATH,
                SERVER,
                "Notify",
                Some(&parameters),
                Some(glib::VariantTy::new("(u)").expect("a type")),
                gio::DBusCallFlags::NONE,
                -1,
            );
            match call.await {
                Ok(answer) => {
                    if let Some((id,)) = answer.get::<(u32,)>() {
                        if wanted.borrow().get(key) == Some(&show) {
                            ids.borrow_mut().insert(key.clone(), id);
                        } else if ids.borrow().get(key) != Some(&id) {
                            // Withdrawn, or shown again, while the server was making it:
                            // it would stay on screen with nothing left to take it back.
                            close(&bus, id);
                        }
                    }
                }
                Err(e) => eprintln!("notification: {e}"),
            }
        });
    }

    /// Take back the notification with `key`, if it shows.
    pub fn withdraw(&self, key: &str) {
        self.wanted.borrow_mut().remove(key);
        let Some(id) = self.ids.borrow_mut().remove(key) else { return };
        close(&self.bus, id);
    }

    /// The count on the app's icon, for launchers that follow Unity's
    /// protocol. Only a change is sent.
    pub fn set_launcher_count(&self, count: u32) {
        if self.launcher.replace(Some(count)) == Some(count) {
            return;
        }
        let (path, update) = launcher_update(self.app_id, count);
        if let Err(e) = self.bus.emit_signal(None, &path, LAUNCHER_INTERFACE, "Update", Some(&update)) {
            eprintln!("launcher: {e}");
        }
    }
}

/// What the server can do: asked once, and kept.
async fn capabilities_of(bus: &gio::DBusConnection, cache: &RefCell<Option<Vec<String>>>) -> Vec<String> {
    if let Some(known) = cache.borrow().clone() {
        return known;
    }
    let call = bus.call_future(
        Some(SERVER),
        PATH,
        SERVER,
        "GetCapabilities",
        None,
        Some(glib::VariantTy::new("(as)").expect("a type")),
        gio::DBusCallFlags::NONE,
        -1,
    );
    let list = match call.await {
        Ok(reply) => reply.get::<(Vec<String>,)>().map(|(list,)| list).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    *cache.borrow_mut() = Some(list.clone());
    list
}

/// `Notify`'s arguments for `notice`, replacing the notification `replaces`
/// (0 for none). `reply` says the server takes replies.
fn arguments(app_name: &str, app_id: &str, replaces: u32, notice: &Notice, reply: bool) -> glib::Variant {
    let mut actions = vec!["default".to_owned(), "Open".to_owned()];
    if reply {
        actions.extend(["inline-reply".to_owned(), "Reply".to_owned()]);
    }
    let hints = glib::VariantDict::new(None);
    hints.insert("desktop-entry", app_id);
    if notice.sound {
        hints.insert("sound-name", SOUND);
    } else {
        hints.insert("suppress-sound", true);
    }
    if let Some(texture) = notice.icon {
        let mut download = gdk::TextureDownloader::new(texture);
        download.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = download.download_bytes();
        let image = (texture.width(), texture.height(), stride as i32, true, 8i32, 4i32, bytes.to_vec());
        hints.insert_value("image-data", &image.to_variant());
    }
    // `hints.end()` is a dict already: in a Rust tuple it would be boxed as a `v`.
    glib::Variant::tuple_from_iter([
        app_name.to_variant(),
        replaces.to_variant(),
        "".to_variant(),
        notice.title.to_variant(),
        notice.body.to_variant(),
        actions.to_variant(),
        hints.end(),
        (-1i32).to_variant(),
    ])
}

/// What Unity's launcher protocol sends for `count`: the object path, and
/// `Update`'s arguments. The count shows while it isn't zero.
fn launcher_update(app_id: &str, count: u32) -> (String, glib::Variant) {
    let uri = format!("application://{app_id}.desktop");
    let hash = uri.bytes().fold(5381u32, |hash, byte| hash.wrapping_mul(33) ^ u32::from(byte));
    let properties = glib::VariantDict::new(None);
    properties.insert("count", i64::from(count));
    properties.insert("count-visible", count > 0);
    (
        format!("/com/canonical/unity/launcherentry/{hash}"),
        glib::Variant::tuple_from_iter([uri.to_variant(), properties.end()]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(sound: bool) -> Notice<'static> {
        Notice { key: "channel-1", title: "Ann (#general, Club)", body: "hi", icon: None, sound, reply: true }
    }

    /// The hint `name` of `Notify`'s arguments.
    fn hint(arguments: &glib::Variant, name: &str) -> Option<glib::Variant> {
        glib::VariantDict::new(Some(&arguments.child_value(6))).lookup_value(name, None)
    }

    #[test]
    fn the_server_plays_the_sound_or_is_told_not_to() {
        let loud = arguments("Neucord", "dev.example.app", 0, &notice(true), false);
        assert_eq!(hint(&loud, "sound-name").and_then(|v| v.get::<String>()).as_deref(), Some(SOUND));
        assert!(hint(&loud, "suppress-sound").is_none());
        let quiet = arguments("Neucord", "dev.example.app", 0, &notice(false), false);
        assert_eq!(hint(&quiet, "suppress-sound").and_then(|v| v.get::<bool>()), Some(true));
        assert!(hint(&quiet, "sound-name").is_none());
    }

    #[test]
    fn a_reply_is_offered_only_where_the_server_takes_one() {
        let actions = |reply| arguments("Neucord", "dev.example.app", 0, &notice(true), reply).child_value(5);
        assert_eq!(actions(false).get::<Vec<String>>().unwrap(), ["default", "Open"]);
        assert_eq!(actions(true).get::<Vec<String>>().unwrap(), ["default", "Open", "inline-reply", "Reply"]);
    }

    #[test]
    fn the_icon_travels_as_raw_pixels() {
        let pixels = glib::Bytes::from_owned(vec![200u8; 2 * 3 * 4]);
        let texture = gdk::MemoryTexture::new(2, 3, gdk::MemoryFormat::R8g8b8a8, &pixels, 8);
        let with = Notice { icon: Some(texture.upcast_ref()), ..notice(true) };
        let image = hint(&arguments("Neucord", "dev.example.app", 0, &with, false), "image-data").unwrap();
        // Width, height, row stride, alpha, bits per sample, channels, the pixels.
        assert_eq!(image.get::<(i32, i32, i32, bool, i32, i32, Vec<u8>)>(), Some((2, 3, 8, true, 8, 4, vec![200; 24])));
        assert!(hint(&arguments("Neucord", "dev.example.app", 0, &notice(true), false), "image-data").is_none());
    }

    #[test]
    fn a_newer_notification_replaces_the_old() {
        let arguments = arguments("Neucord", "dev.example.app", 7, &notice(true), false);
        assert_eq!(arguments.child_value(1).get::<u32>(), Some(7));
        assert_eq!(arguments.child_value(0).get::<String>().as_deref(), Some("Neucord"));
    }

    /// A notification server on a private bus that records what it is asked.
    struct Server {
        calls: Rc<RefCell<Vec<(String, glib::Variant)>>>,
        /// While set, `Notify` isn't answered until [`Server::release`], so a
        /// test decides when the answer reaches the client.
        hold: Rc<Cell<bool>>,
        held: Rc<RefCell<Vec<(gio::DBusMethodInvocation, u32)>>>,
        bus: gio::DBusConnection,
        registration: gio::RegistrationId,
    }

    impl Server {
        fn start(bus: &gio::DBusConnection) -> Server {
            let xml = "<node><interface name='org.freedesktop.Notifications'>\
                <method name='GetCapabilities'><arg type='as' direction='out'/></method>\
                <method name='Notify'><arg type='s'/><arg type='u'/><arg type='s'/><arg type='s'/><arg type='s'/>\
                    <arg type='as'/><arg type='a{sv}'/><arg type='i'/><arg type='u' direction='out'/></method>\
                <method name='CloseNotification'><arg type='u'/></method>\
                </interface></node>";
            let info = gio::DBusNodeInfo::for_xml(xml).expect("valid xml");
            let calls: Rc<RefCell<Vec<(String, glib::Variant)>>> = Rc::default();
            let next = Cell::new(1u32);
            let log = calls.clone();
            let hold: Rc<Cell<bool>> = Rc::default();
            let held: Rc<RefCell<Vec<(gio::DBusMethodInvocation, u32)>>> = Rc::default();
            let (holding, held_back) = (hold.clone(), held.clone());
            let registration = bus
                .register_object(PATH, &info.lookup_interface(SERVER).expect("an interface"))
                .method_call(move |_, _, _, _, method, parameters, invocation| {
                    log.borrow_mut().push((method.to_owned(), parameters.clone()));
                    match method {
                        "GetCapabilities" => {
                            invocation.return_value(Some(&(vec!["actions", "inline-reply"],).to_variant()))
                        }
                        "Notify" => {
                            let replaces = parameters.child_value(1).get::<u32>().unwrap_or(0);
                            let id = if replaces > 0 {
                                replaces
                            } else {
                                next.set(next.get() + 1);
                                next.get() - 1
                            };
                            if holding.get() {
                                held_back.borrow_mut().push((invocation, id));
                            } else {
                                invocation.return_value(Some(&(id,).to_variant()))
                            }
                        }
                        _ => invocation.return_value(None),
                    }
                })
                .build()
                .expect("registered");
            Server { calls, hold, held, bus: bus.clone(), registration }
        }

        /// Answer every `Notify` held back, with the ids it was given.
        fn release(&self) -> Vec<u32> {
            self.hold.set(false);
            self.held
                .take()
                .into_iter()
                .map(|(invocation, id)| {
                    invocation.return_value(Some(&(id,).to_variant()));
                    id
                })
                .collect()
        }

        /// Let go of the bus: it can't be shut down while this holds it.
        fn stop(self) {
            self.bus.unregister_object(self.registration).expect("unregistered");
        }

        fn calls(&self, method: &str) -> Vec<glib::Variant> {
            self.calls.borrow().iter().filter(|(m, _)| m == method).map(|(_, p)| p.clone()).collect()
        }

        fn signal(&self, name: &str, parameters: glib::Variant) {
            self.bus.emit_signal(None, PATH, SERVER, name, Some(&parameters)).expect("emitted");
        }
    }

    /// Run the main loop until `done`, which must happen within a few seconds.
    fn until(done: impl Fn() -> bool) {
        let context = glib::MainContext::ref_thread_default();
        let limit = glib::monotonic_time() + 5_000_000;
        while !done() {
            assert!(glib::monotonic_time() < limit, "timed out");
            context.iteration(false);
        }
    }

    /// Run the main loop until everything already on its way to the client
    /// has been handled: a call to the server is answered after anything the
    /// server sent before it.
    fn settle(bus: &gio::DBusConnection) {
        let answered = Rc::new(Cell::new(false));
        let call =
            bus.call_future(Some(SERVER), PATH, SERVER, "GetCapabilities", None, None, gio::DBusCallFlags::NONE, -1);
        glib::spawn_future_local({
            let answered = answered.clone();
            async move {
                let _ = call.await;
                answered.set(true);
            }
        });
        until(|| answered.get());
        while glib::MainContext::ref_thread_default().iteration(false) {}
    }

    /// A private bus with a notification server and a [`Desktop`] on it, run
    /// by a main context of its own: what one test leaves queued must not be
    /// dispatched by another's thread.
    struct Rig {
        _turn: std::sync::MutexGuard<'static, ()>,
        private: gio::TestDBus,
        bus: gio::DBusConnection,
        server: Server,
        name: gio::OwnerId,
        heard: Rc<RefCell<Vec<(String, String)>>>,
        desktop: Desktop,
    }

    impl Rig {
        /// Runs `test` on a rig. Without a daemon to start there is nothing
        /// to test against, and it isn't run.
        fn run(test: impl FnOnce(&Rig)) {
            static TURNS: std::sync::Mutex<()> = std::sync::Mutex::new(());
            if glib::find_program_in_path("dbus-daemon").is_none() {
                return;
            }
            let turn = TURNS.lock().unwrap_or_else(|e| e.into_inner());
            glib::MainContext::new()
                .with_thread_default(|| {
                    let rig = Rig::start(turn);
                    test(&rig);
                    rig.finish();
                })
                .expect("a main context of its own");
        }

        fn start(turn: std::sync::MutexGuard<'static, ()>) -> Rig {
            let private = gio::TestDBus::new(gio::TestDBusFlags::NONE);
            private.up();
            // Its own connection: the session bus's is shared by every test.
            let address = private.bus_address().expect("the private bus's address");
            let bus = gio::DBusConnection::for_address_sync(
                &address,
                gio::DBusConnectionFlags::AUTHENTICATION_CLIENT | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
                None,
                gio::Cancellable::NONE,
            )
            .expect("the private bus");
            let server = Server::start(&bus);
            let name =
                gio::bus_own_name_on_connection(&bus, SERVER, gio::BusNameOwnerFlags::NONE, |_, _| {}, |_, _| {});
            let heard: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
            let desktop = Desktop::new(bus.clone(), "dev.example.app", "Neucord", {
                let heard = heard.clone();
                move |key, event| {
                    let what = match event {
                        Event::Opened => "opened".to_owned(),
                        Event::Replied(text) => format!("replied {text}"),
                    };
                    heard.borrow_mut().push((key.to_owned(), what));
                }
            });
            until(|| desktop.capabilities.borrow().is_some());
            Rig { _turn: turn, private, bus, server, name, heard, desktop }
        }

        /// Everything on the bus lets go before it goes: the daemon waits for its connections.
        fn finish(self) {
            let Rig { private, bus, server, name, desktop, .. } = self;
            gio::bus_unown_name(name);
            drop(desktop);
            server.stop();
            bus.close_sync(gio::Cancellable::NONE).expect("closed");
            drop(bus);
            private.down();
        }
    }

    #[test]
    fn notifications_go_through_the_server_and_its_answers_come_back() {
        Rig::run(|rig| {
            let Rig { server, heard, desktop, .. } = rig;

            desktop.show(&notice(true));
            until(|| !server.calls("Notify").is_empty() && desktop.ids.borrow().contains_key("channel-1"));
            let first = server.calls("Notify").remove(0);
            assert_eq!(
                first.child_value(5).get::<Vec<String>>().unwrap(),
                ["default", "Open", "inline-reply", "Reply"]
            );
            assert_eq!(first.child_value(1).get::<u32>(), Some(0), "nothing to replace yet");
            let id = desktop.ids.borrow()["channel-1"];

            desktop.show(&notice(true));
            until(|| server.calls("Notify").len() == 2);
            assert_eq!(server.calls("Notify")[1].child_value(1).get::<u32>(), Some(id), "the newer replaces the first");
            settle(&rig.bus);

            server.signal("ActionInvoked", (id, "default").to_variant());
            server.signal("ActionInvoked", (id, "something else").to_variant());
            server.signal("NotificationReplied", (id, "on my way").to_variant());
            server.signal("ActionInvoked", (id + 100, "default").to_variant());
            until(|| heard.borrow().len() == 2);
            assert_eq!(
                *heard.borrow(),
                [
                    ("channel-1".to_owned(), "opened".to_owned()),
                    ("channel-1".to_owned(), "replied on my way".to_owned())
                ]
            );

            desktop.withdraw("channel-1");
            until(|| !server.calls("CloseNotification").is_empty());
            assert_eq!(server.calls("CloseNotification")[0].child_value(0).get::<u32>(), Some(id));
            assert!(desktop.ids.borrow().is_empty());
            desktop.show(&notice(true));
            until(|| server.calls("Notify").len() == 3);
            assert_eq!(
                server.calls("Notify")[2].child_value(1).get::<u32>(),
                Some(0),
                "a withdrawn one isn't replaced"
            );

            // A server that closed it by itself is forgotten the same way.
            until(|| desktop.ids.borrow().contains_key("channel-1"));
            let id = desktop.ids.borrow()["channel-1"];
            server.signal("NotificationClosed", (id, 2u32).to_variant());
            until(|| desktop.ids.borrow().is_empty());
        });
    }

    #[test]
    fn an_answer_that_comes_after_the_withdrawal_leaves_nothing_shown() {
        Rig::run(|rig| {
            let Rig { server, desktop, .. } = rig;

            server.hold.set(true);
            desktop.show(&notice(true));
            until(|| !server.held.borrow().is_empty());
            desktop.withdraw("channel-1");
            let shown = server.release();
            settle(&rig.bus);

            assert!(desktop.ids.borrow().is_empty(), "the withdrawn notification's id was kept");
            until(|| !server.calls("CloseNotification").is_empty());
            let closed: Vec<_> =
                server.calls("CloseNotification").iter().filter_map(|p| p.child_value(0).get::<u32>()).collect();
            assert_eq!(closed, shown, "the notification the server made after the withdrawal is taken back");
        });
    }

    #[test]
    fn the_launcher_shows_a_count_while_it_isnt_zero() {
        let (path, update) = launcher_update("dev.example.app", 3);
        assert!(path.starts_with("/com/canonical/unity/launcherentry/"));
        assert_eq!(update.child_value(0).get::<String>().as_deref(), Some("application://dev.example.app.desktop"));
        let properties = glib::VariantDict::new(Some(&update.child_value(1)));
        assert_eq!(properties.lookup::<i64>("count").unwrap(), Some(3));
        assert_eq!(properties.lookup::<bool>("count-visible").unwrap(), Some(true));
        let (_, none) = launcher_update("dev.example.app", 0);
        let properties = glib::VariantDict::new(Some(&none.child_value(1)));
        assert_eq!(properties.lookup::<bool>("count-visible").unwrap(), Some(false));
    }
}
