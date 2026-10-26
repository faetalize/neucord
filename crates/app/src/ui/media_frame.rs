//! A frame for media in messages: as wide as the media is meant to be shown,
//! narrower when there is less room, and always as tall as that width and
//! the media's aspect ratio make it. A fixed size would force the whole
//! message list at least that wide; a shrinkable picture alone reports no
//! minimum height, which list views size their rows by.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    pub struct MediaFrame {
        /// The largest size shown, in logical pixels.
        pub size: Cell<(i32, i32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MediaFrame {
        const NAME: &'static str = "NeucordMediaFrame";
        type Type = super::MediaFrame;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MediaFrame {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for MediaFrame {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // GTK wants children measured before they are allocated; the
            // frame decides their size regardless.
            let mut child = self.obj().first_child();
            while let Some(widget) = child {
                widget.measure(orientation, for_size);
                child = widget.next_sibling();
            }
            let (width, height) = self.size.get();
            match orientation {
                gtk::Orientation::Horizontal => (0, width, -1, -1),
                _ => {
                    let shown = if for_size < 0 { width } else { for_size.min(width) };
                    let height = (f64::from(shown) * f64::from(height) / f64::from(width.max(1))).round() as i32;
                    (height, height, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let mut child = self.obj().first_child();
            while let Some(widget) = child {
                widget.allocate(width, height, baseline, None);
                child = widget.next_sibling();
            }
        }
    }
}

glib::wrapper! {
    pub struct MediaFrame(ObjectSubclass<imp::MediaFrame>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MediaFrame {
    /// A frame at most `size` holding `child`, which fills it.
    pub fn new(size: (i32, i32), child: &impl IsA<gtk::Widget>) -> Self {
        let frame: Self = glib::Object::builder().property("halign", gtk::Align::Start).build();
        frame.imp().size.set((size.0.max(1), size.1.max(1)));
        child.set_parent(&frame);
        frame
    }
}
