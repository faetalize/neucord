//! The line joining a thread's row in the sidebar to its parent channel's:
//! down from the row above, then curving into the thread's name. Above the
//! last thread it carries on down to the next. It reaches across the gaps
//! between rows, so the line is unbroken from the parent to its last
//! thread.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, gsk};

/// Where the line runs, from the widget's start.
const X: f32 = 4.5;
/// How far the line reaches across, curve included.
const WIDTH: i32 = 14;
const RADIUS: f32 = 6.0;

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    pub struct ThreadLine {
        /// The parent's last thread: the line ends here.
        pub last: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThreadLine {
        const NAME: &'static str = "NeucordThreadLine";
        type Type = super::ThreadLine;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ThreadLine {}

    impl WidgetImpl for ThreadLine {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (WIDTH, WIDTH, -1, -1),
                _ => (0, 0, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let height = widget.height() as f32;
            let middle = (height / 2.0).round() + 0.5;
            // The rows around ours, in our coordinates: the line starts at
            // the bottom of the one above and, unless we're last, reaches
            // the top of the one below.
            let row = widget.ancestor(gtk::ListBoxRow::static_type());
            let bounds = |row: Option<gtk::Widget>| row.and_then(|r| r.compute_bounds(&*widget));
            let top = bounds(row.as_ref().and_then(|r| r.prev_sibling())).map_or(0.0, |b| b.y() + b.height());
            let bottom = bounds(row.as_ref().and_then(|r| r.next_sibling())).map_or(height, |b| b.y());

            let path = gsk::PathBuilder::new();
            path.move_to(X, top);
            path.line_to(X, middle - RADIUS);
            path.quad_to(X, middle, X + RADIUS, middle);
            path.line_to(WIDTH as f32, middle);
            if !self.last.get() {
                path.move_to(X, middle - RADIUS);
                path.line_to(X, bottom);
            }
            let stroke = gsk::Stroke::new(1.0);
            snapshot.append_stroke(&path.to_path(), &stroke, &widget.color());
        }
    }
}

glib::wrapper! {
    pub struct ThreadLine(ObjectSubclass<imp::ThreadLine>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ThreadLine {
    /// The line for a thread's row; `last` if no thread of the same parent
    /// follows it.
    pub fn new(last: bool) -> Self {
        let line: Self = glib::Object::new();
        line.add_css_class("dimmed");
        line.imp().last.set(last);
        line
    }
}
