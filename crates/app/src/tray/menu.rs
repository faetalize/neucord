//! The tray's menu, in `com.canonical.dbusmenu`: the panel draws it from a
//! layout we describe and tells us which entry was clicked.
//!
//! Open and Quit, as in the client's. Its Mute and Deafen are for a voice
//! call, which Neucord can't join; it has no status to mute either (#2).

use gtk::glib::prelude::*;
use gtk::glib::{self, Variant, VariantDict};

/// What choosing an entry asks of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Quit,
}

const OPEN: i32 = 1;
const SEPARATOR: i32 = 2;
const QUIT: i32 = 3;

/// The entries in order: id, and a label (a separator has none).
const ENTRIES: [(i32, Option<&str>); 3] =
    [(OPEN, Some("Open Neucord")), (SEPARATOR, None), (QUIT, Some("Quit Neucord"))];

/// What a click on `id` asks for.
pub fn command(id: i32) -> Option<Command> {
    match id {
        OPEN => Some(Command::Open),
        QUIT => Some(Command::Quit),
        _ => None,
    }
}

/// An entry's properties, `a{sv}`. The panel asks for some by name; one that
/// isn't asked for is left out.
fn properties(id: i32, wanted: &[String]) -> Variant {
    let dict = VariantDict::new(None);
    let put = |name: &str, value: Variant| {
        if wanted.is_empty() || wanted.iter().any(|w| w == name) {
            dict.insert_value(name, &value);
        }
    };
    match ENTRIES.iter().find(|(entry, _)| *entry == id) {
        Some((_, Some(label))) => put("label", label.to_variant()),
        Some((_, None)) => put("type", "separator".to_variant()),
        // The root.
        None => put("children-display", "submenu".to_variant()),
    }
    dict.end()
}

/// `(ia{sv}av)` for one entry, with children for the root.
fn node(id: i32, wanted: &[String], children: Variant) -> Variant {
    Variant::tuple_from_iter([id.to_variant(), properties(id, wanted), children])
}

fn no_children() -> Variant {
    Variant::array_from_iter_with_type(glib::VariantTy::VARIANT, std::iter::empty::<Variant>())
}

/// The reply to `GetLayout`: a revision, and the tree under `parent`. Only
/// the root has entries below it.
pub fn layout(parent: i32, revision: u32, wanted: &[String]) -> Variant {
    let children = if parent == 0 {
        Variant::array_from_iter_with_type(
            glib::VariantTy::VARIANT,
            ENTRIES.iter().map(|(id, _)| Variant::from_variant(&node(*id, wanted, no_children()))),
        )
    } else {
        no_children()
    };
    Variant::tuple_from_iter([revision.to_variant(), node(parent, wanted, children)])
}

/// The reply to `GetGroupProperties`: `(a(ia{sv}))`. No ids means all.
pub fn group_properties(ids: &[i32], wanted: &[String]) -> Variant {
    let all = std::iter::once(0).chain(ENTRIES.iter().map(|(id, _)| *id));
    let known: Vec<i32> = if ids.is_empty() { all.collect() } else { all.filter(|id| ids.contains(id)).collect() };
    let entries = Variant::array_from_iter_with_type(
        glib::VariantTy::new("(ia{sv})").unwrap(),
        known.into_iter().map(|id| Variant::tuple_from_iter([id.to_variant(), properties(id, wanted)])),
    );
    Variant::tuple_from_iter([entries])
}

/// The reply to `GetProperty`: `(v)`.
pub fn property(id: i32, name: &str) -> Option<Variant> {
    let all = properties(id, &[]);
    let value = VariantDict::new(Some(&all)).lookup_value(name, None)?;
    Some(Variant::tuple_from_iter([Variant::from_variant(&value)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(layout: &Variant) -> Vec<Option<String>> {
        let root = layout.child_value(1);
        root.child_value(2)
            .iter()
            .map(|child| {
                let node = child.as_variant().unwrap();
                let props = VariantDict::new(Some(&node.child_value(1)));
                props.lookup::<String>("label").unwrap()
            })
            .collect()
    }

    #[test]
    fn the_layout_is_open_a_separator_and_quit_under_a_root_that_is_a_menu() {
        let tree = layout(0, 1, &[]);
        assert_eq!(tree.type_().as_str(), "(u(ia{sv}av))");
        assert_eq!(labels(&tree), [Some("Open Neucord".to_owned()), None, Some("Quit Neucord".to_owned())]);
        let root = tree.child_value(1);
        let props = VariantDict::new(Some(&root.child_value(1)));
        assert_eq!(props.lookup::<String>("children-display").unwrap().as_deref(), Some("submenu"));
    }

    #[test]
    fn only_the_properties_asked_for_are_sent() {
        let tree = layout(0, 1, &["type".to_owned()]);
        assert_eq!(labels(&tree), [None, None, None], "no labels were asked for");
    }

    #[test]
    fn an_entry_has_no_children_of_its_own() {
        let tree = layout(OPEN, 1, &[]);
        assert_eq!(tree.child_value(1).child_value(0).get::<i32>(), Some(OPEN));
        assert_eq!(tree.child_value(1).child_value(2).n_children(), 0);
    }

    #[test]
    fn clicks_map_to_commands() {
        assert_eq!(command(OPEN), Some(Command::Open));
        assert_eq!(command(QUIT), Some(Command::Quit));
        assert_eq!(command(SEPARATOR), None);
        assert_eq!(command(0), None);
        assert_eq!(command(99), None);
    }

    #[test]
    fn group_properties_and_single_properties_agree_with_the_layout() {
        let group = group_properties(&[OPEN, QUIT, 99], &[]);
        assert_eq!(group.type_().as_str(), "(a(ia{sv}))");
        assert_eq!(group.child_value(0).n_children(), 2, "an unknown id is skipped");
        assert_eq!(group_properties(&[], &[]).child_value(0).n_children(), 4, "no ids means the root and all entries");
        let label = property(OPEN, "label").unwrap().child_value(0).as_variant().unwrap();
        assert_eq!(label.str(), Some("Open Neucord"));
        assert!(property(OPEN, "icon-name").is_none());
    }
}
