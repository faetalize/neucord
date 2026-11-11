//! The tray icon, as a StatusNotifierItem on the session bus (what KDE,
//! Waybar and most panels host; GNOME needs the AppIndicator extension).
//!
//! GLib's own D-Bus bindings carry it: libappindicator and the tray crates
//! built on it need GTK 3, which can't live in a GTK 4 process, and the
//! pure-Rust ones bring an async runtime of their own for a handful of
//! properties and one menu.
//!
//! The item owns `org.kde.StatusNotifierItem-<pid>-1` and registers it with
//! `org.kde.StatusNotifierWatcher`. Whether a panel actually shows it is
//! [`Tray::shown`]: without one (GNOME without the extension, a bar not yet
//! started) the window is the only way back in, so the app mustn't hide it.

mod icon;
mod menu;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::gio::{self, BusNameOwnerFlags, DBusCallFlags, DBusConnection, DBusNodeInfo, DBusSignalFlags};
use gtk::glib::prelude::*;
use gtk::glib::{self, Variant};

pub use icon::Alert;
pub use menu::Command;

const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// Neither entry changes while we run, so the menu stays at its first
/// revision.
const MENU_REVISION: u32 = 1;

const INTROSPECTION: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="u" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="OverlayIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionMovieName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <method name="ContextMenu"><arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/></method>
    <method name="Activate"><arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/></method>
    <method name="SecondaryActivate"><arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/></method>
    <method name="Scroll"><arg type="i" name="delta" direction="in"/><arg type="s" name="orientation" direction="in"/></method>
    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewAttentionIcon"/>
    <signal name="NewOverlayIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus"><arg type="s" name="status"/></signal>
  </interface>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg type="i" name="parentId" direction="in"/>
      <arg type="i" name="recursionDepth" direction="in"/>
      <arg type="as" name="propertyNames" direction="in"/>
      <arg type="u" name="revision" direction="out"/>
      <arg type="(ia{sv}av)" name="layout" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg type="ai" name="ids" direction="in"/>
      <arg type="as" name="propertyNames" direction="in"/>
      <arg type="a(ia{sv})" name="properties" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg type="i" name="id" direction="in"/>
      <arg type="s" name="name" direction="in"/>
      <arg type="v" name="value" direction="out"/>
    </method>
    <method name="Event">
      <arg type="i" name="id" direction="in"/>
      <arg type="s" name="eventId" direction="in"/>
      <arg type="v" name="data" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg type="a(isvu)" name="events" direction="in"/>
      <arg type="ai" name="idErrors" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg type="i" name="id" direction="in"/>
      <arg type="b" name="needUpdate" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg type="ai" name="ids" direction="in"/>
      <arg type="ai" name="updatesNeeded" direction="out"/>
      <arg type="ai" name="idErrors" direction="out"/>
    </method>
    <signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})"/><arg type="a(ias)"/></signal>
    <signal name="LayoutUpdated"><arg type="u" name="revision"/><arg type="i" name="parent"/></signal>
    <signal name="ItemActivationRequested"><arg type="i" name="id"/><arg type="u" name="timestamp"/></signal>
  </interface>
</node>"#;

/// What the tray tells the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The icon was clicked, or an entry of its menu chosen.
    Command(Command),
    /// A panel started or stopped showing the icon.
    Shown(bool),
}

type Handler = Rc<dyn Fn(Event)>;

#[derive(Clone)]
pub struct Tray(Rc<Inner>);

struct Inner {
    alert: Cell<Alert>,
    shown: Cell<bool>,
    handler: RefCell<Option<Handler>>,
    connection: RefCell<Option<DBusConnection>>,
    /// Kept for as long as the tray lives; dropping one would end it.
    subscriptions: RefCell<Vec<gio::SignalSubscription>>,
}

impl Tray {
    /// A tray that isn't on the bus yet.
    pub fn new() -> Tray {
        Tray(Rc::new(Inner {
            alert: Cell::default(),
            shown: Cell::new(false),
            handler: RefCell::default(),
            connection: RefCell::default(),
            subscriptions: RefCell::default(),
        }))
    }

    /// Put the icon on the session bus. Without one the tray stays out of
    /// sight, which [`Tray::shown`] reports.
    pub fn start(&self, handler: impl Fn(Event) + 'static) {
        *self.0.handler.borrow_mut() = Some(Rc::new(handler));
        let inner = Rc::downgrade(&self.0);
        glib::spawn_future_local(async move {
            let connection = match gio::bus_get_future(gio::BusType::Session).await {
                Ok(connection) => connection,
                Err(e) => return eprintln!("tray: no session bus: {e}"),
            };
            let Some(inner) = inner.upgrade() else { return };
            if let Err(e) = inner.publish(&connection) {
                eprintln!("tray: {e}");
            }
        });
    }

    /// Whether a panel is showing the icon.
    pub fn shown(&self) -> bool {
        self.0.shown.get()
    }

    /// Change the badge.
    pub fn set_alert(&self, alert: Alert) {
        if self.0.alert.replace(alert) != alert {
            self.0.emit(ITEM_PATH, ITEM, "NewIcon");
            self.0.emit(ITEM_PATH, ITEM, "NewToolTip");
        }
    }
}

impl Inner {
    fn say(&self, event: Event) {
        let handler = self.handler.borrow().clone();
        if let Some(handler) = handler {
            handler(event);
        }
    }

    fn emit(&self, path: &str, interface: &str, signal: &str) {
        if let Some(connection) = self.connection.borrow().as_ref()
            && let Err(e) = connection.emit_signal(None, path, interface, signal, None)
        {
            eprintln!("tray: {signal}: {e}");
        }
    }

    fn set_shown(&self, shown: bool) {
        if self.shown.replace(shown) != shown {
            self.say(Event::Shown(shown));
        }
    }

    /// Export the item and its menu, take the name, and find the watcher.
    fn publish(self: &Rc<Self>, connection: &DBusConnection) -> Result<(), glib::Error> {
        let nodes = DBusNodeInfo::for_xml(INTROSPECTION)?;
        let interface = |name| nodes.lookup_interface(name).expect("declared above");
        let (item, menu) = (interface(ITEM), interface(MENU));

        let weak = Rc::downgrade(self);
        connection
            .register_object(ITEM_PATH, &item)
            .method_call({
                let weak = weak.clone();
                move |_, _, _, _, method, _, invocation| {
                    if let (Some(inner), "Activate" | "SecondaryActivate") = (weak.upgrade(), method) {
                        inner.say(Event::Command(Command::Open));
                    }
                    invocation.return_value(None);
                }
            })
            .property({
                let weak = weak.clone();
                move |_, _, _, _, name| {
                    weak.upgrade()
                        .map_or_else(|| item_property("", Alert::Quiet), |i| item_property(name, i.alert.get()))
                }
            })
            .build()?;
        connection
            .register_object(MENU_PATH, &menu)
            .method_call({
                let weak = weak.clone();
                move |_, _, _, _, method, parameters, invocation| answer(&weak, method, &parameters, invocation)
            })
            .property(|_, _, _, _, name| menu_property(name))
            .build()?;

        *self.connection.borrow_mut() = Some(connection.clone());
        let service = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let _ = gio::bus_own_name_on_connection(
            connection,
            &service,
            BusNameOwnerFlags::DO_NOT_QUEUE,
            {
                let weak = weak.clone();
                let service = service.clone();
                move |connection, _| {
                    if let Some(inner) = weak.upgrade() {
                        inner.watch(&connection, &service);
                    }
                }
            },
            |_, name| eprintln!("tray: could not take {name}"),
        );
        Ok(())
    }

    /// Register with the watcher whenever it appears (it may start after us
    /// or restart), and follow whether a panel hosts the icon.
    fn watch(self: &Rc<Self>, connection: &DBusConnection, service: &str) {
        let weak = Rc::downgrade(self);
        let recheck = {
            let weak = weak.clone();
            move |connection: &DBusConnection| {
                if let Some(inner) = weak.upgrade() {
                    inner.check_host(connection);
                }
            }
        };
        let mut subscriptions = self.subscriptions.borrow_mut();
        for signal in ["StatusNotifierHostRegistered", "StatusNotifierHostUnregistered"] {
            let recheck = recheck.clone();
            subscriptions.push(connection.subscribe_to_signal(
                Some(WATCHER),
                Some(WATCHER),
                Some(signal),
                Some(WATCHER_PATH),
                None,
                DBusSignalFlags::NONE,
                move |s| recheck(s.connection),
            ));
        }
        let service = service.to_owned();
        let _ = gio::bus_watch_name_on_connection(
            connection,
            WATCHER,
            gio::BusNameWatcherFlags::NONE,
            {
                let weak = weak.clone();
                move |connection, _, _| {
                    let Some(inner) = weak.upgrade() else { return };
                    let weak = Rc::downgrade(&inner);
                    let checking = connection.clone();
                    connection.call(
                        Some(WATCHER),
                        WATCHER_PATH,
                        WATCHER,
                        "RegisterStatusNotifierItem",
                        Some(&(service.as_str(),).to_variant()),
                        None,
                        DBusCallFlags::NONE,
                        -1,
                        None::<&gio::Cancellable>,
                        move |reply| match (reply, weak.upgrade()) {
                            (Ok(_), Some(inner)) => inner.check_host(&checking),
                            (Err(e), _) => eprintln!("tray: registering: {e}"),
                            _ => {}
                        },
                    );
                }
            },
            {
                let weak = weak.clone();
                move |_, _| {
                    if let Some(inner) = weak.upgrade() {
                        inner.set_shown(false);
                    }
                }
            },
        );
    }

    fn check_host(self: &Rc<Self>, connection: &DBusConnection) {
        let weak = Rc::downgrade(self);
        connection.call(
            Some(WATCHER),
            WATCHER_PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(WATCHER, "IsStatusNotifierHostRegistered").to_variant()),
            None,
            DBusCallFlags::NONE,
            -1,
            None::<&gio::Cancellable>,
            move |reply| {
                let hosted = reply.ok().and_then(|r| r.child_value(0).as_variant()?.get::<bool>()).unwrap_or(false);
                if let Some(inner) = weak.upgrade() {
                    inner.set_shown(hosted);
                }
            },
        );
    }
}

/// A StatusNotifierItem property. The title and the badge's text are the
/// only things that vary.
fn item_property(name: &str, alert: Alert) -> Variant {
    let text = |s: &str| s.to_variant();
    let pixmap_type = glib::VariantTy::new("(iiay)").expect("a type");
    let no_pixmaps = || Variant::array_from_iter_with_type(pixmap_type, std::iter::empty::<Variant>());
    match name {
        "Category" => text("Communications"),
        "Id" => text("dev.faetalize.neucord"),
        "Title" => text("Neucord"),
        "Status" => text("Active"),
        "WindowId" => 0u32.to_variant(),
        // The glyph is drawn: no icon is installed to name.
        "IconThemePath" | "IconName" | "OverlayIconName" | "AttentionIconName" | "AttentionMovieName" => text(""),
        "IconPixmap" => Variant::array_from_iter_with_type(
            pixmap_type,
            icon::SIZES.map(|size| {
                Variant::tuple_from_iter([size.to_variant(), size.to_variant(), icon::render(size, alert).to_variant()])
            }),
        ),
        "OverlayIconPixmap" | "AttentionIconPixmap" => no_pixmaps(),
        "ToolTip" => Variant::tuple_from_iter([text(""), no_pixmaps(), text("Neucord"), text(&alert.describe())]),
        "ItemIsMenu" => false.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH).expect("a path").to_variant(),
        _ => text(""),
    }
}

fn menu_property(name: &str) -> Variant {
    match name {
        "Version" => 3u32.to_variant(),
        "TextDirection" => "ltr".to_variant(),
        "Status" => "normal".to_variant(),
        _ => Vec::<String>::new().to_variant(),
    }
}

/// Reply to a dbusmenu call.
fn answer(inner: &Weak<Inner>, method: &str, parameters: &Variant, invocation: gio::DBusMethodInvocation) {
    let click = |id: i32, event: &str| {
        if let (true, Some(command), Some(inner)) = (event == "clicked", menu::command(id), inner.upgrade()) {
            inner.say(Event::Command(command));
        }
    };
    let none = Vec::<i32>::new().to_variant();
    let reply = match method {
        "GetLayout" => parameters
            .get::<(i32, i32, Vec<String>)>()
            .map(|(parent, _, names)| menu::layout(parent, MENU_REVISION, &names)),
        "GetGroupProperties" => {
            parameters.get::<(Vec<i32>, Vec<String>)>().map(|(ids, names)| menu::group_properties(&ids, &names))
        }
        "GetProperty" => parameters.get::<(i32, String)>().and_then(|(id, name)| menu::property(id, &name)),
        "Event" => parameters.get::<(i32, String, Variant, u32)>().map(|(id, event, _, _)| {
            click(id, &event);
            Variant::tuple_from_iter(std::iter::empty::<Variant>())
        }),
        "EventGroup" => parameters.get::<(Vec<(i32, String, Variant, u32)>,)>().map(|(events,)| {
            for (id, event, _, _) in events {
                click(id, &event);
            }
            Variant::tuple_from_iter([none.clone()])
        }),
        "AboutToShow" => Some((false,).to_variant()),
        "AboutToShowGroup" => Some(Variant::tuple_from_iter([none.clone(), none])),
        _ => None,
    };
    match reply {
        Some(reply) => invocation.return_value(Some(&reply)),
        None => invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "not understood"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The properties an interface declares in the XML, with their types.
    /// GLib has no accessor for them, so the XML is read directly.
    fn declared(interface: &str) -> Vec<(String, String)> {
        DBusNodeInfo::for_xml(INTROSPECTION).expect("the XML parses");
        let section = INTROSPECTION.split("<interface name=\"").find(|s| s.starts_with(interface)).unwrap();
        let section = section.split("</interface>").next().unwrap();
        let attribute = |tag: &str, name: &str| {
            let at = tag.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
            tag[at..].split('"').next().unwrap().to_owned()
        };
        section.split("<property ").skip(1).map(|tag| (attribute(tag, "name"), attribute(tag, "type"))).collect()
    }

    #[test]
    fn every_property_is_served_with_the_type_it_declares() {
        for alert in [Alert::Quiet, Alert::Unread, Alert::Mentions(4)] {
            for (name, signature) in declared(ITEM) {
                assert_eq!(item_property(&name, alert).type_().as_str(), signature, "{name}");
            }
        }
        for (name, signature) in declared(MENU) {
            assert_eq!(menu_property(&name).type_().as_str(), signature, "{name}");
        }
    }

    #[test]
    fn the_tooltip_and_the_icon_follow_the_alert() {
        let tooltip = item_property("ToolTip", Alert::Mentions(2));
        assert_eq!(tooltip.child_value(3).str(), Some("2 mentions"));
        assert_eq!(tooltip.child_value(2).str(), Some("Neucord"));
        let pixmaps = |alert| item_property("IconPixmap", alert);
        assert_eq!(pixmaps(Alert::Quiet).n_children(), icon::SIZES.len());
        assert_ne!(pixmaps(Alert::Quiet), pixmaps(Alert::Unread));
    }

    #[test]
    fn the_menu_is_where_the_item_says_it_is() {
        let menu = item_property("Menu", Alert::Quiet);
        assert_eq!(menu.str(), Some(MENU_PATH));
        assert_eq!(item_property("ItemIsMenu", Alert::Quiet).get::<bool>(), Some(false), "a click activates");
    }
}
