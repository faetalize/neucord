//! A `gdk::Paintable` that steps a `PixbufAnimation` frame by frame on a timer,
//! so GIF/WebP images animate. GTK's `Picture` otherwise shows only the first
//! frame of an animation.

use std::cell::RefCell;
use std::time::{Duration, SystemTime};

use gtk::gdk;
use gtk::gdk_pixbuf::{PixbufAnimation, PixbufAnimationIter};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AnimatedPaintable {
        pub iter: RefCell<Option<PixbufAnimationIter>>,
        pub texture: RefCell<Option<gdk::Texture>>,
        pub size: RefCell<(i32, i32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AnimatedPaintable {
        const NAME: &'static str = "NeucordAnimatedPaintable";
        type Type = super::AnimatedPaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for AnimatedPaintable {}

    impl PaintableImpl for AnimatedPaintable {
        fn intrinsic_width(&self) -> i32 {
            self.size.borrow().0
        }

        fn intrinsic_height(&self) -> i32 {
            self.size.borrow().1
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if let Some(texture) = self.texture.borrow().as_ref() {
                texture.snapshot(snapshot, width, height);
            }
        }
    }
}

glib::wrapper! {
    pub struct AnimatedPaintable(ObjectSubclass<imp::AnimatedPaintable>) @implements gdk::Paintable;
}

impl AnimatedPaintable {
    pub fn new(animation: PixbufAnimation) -> Self {
        let this: Self = glib::Object::new();
        let iter = animation.iter(Some(SystemTime::now()));
        let imp = this.imp();
        *imp.size.borrow_mut() = (animation.width(), animation.height());
        this.apply_frame(&iter);
        *imp.iter.borrow_mut() = Some(iter);
        this.schedule();
        this
    }

    fn apply_frame(&self, iter: &PixbufAnimationIter) {
        // for_pixbuf is deprecated in 4.20 but remains the simplest per-frame path.
        #[allow(deprecated)]
        let texture = gdk::Texture::for_pixbuf(&iter.pixbuf());
        *self.imp().texture.borrow_mut() = Some(texture);
        self.invalidate_contents();
    }

    fn schedule(&self) {
        // A missing/zero delay means "no fixed rate"; use a sane minimum.
        let delay = self
            .imp()
            .iter
            .borrow()
            .as_ref()
            .and_then(PixbufAnimationIter::delay_time)
            .unwrap_or(Duration::from_millis(100));
        let ms = delay.as_millis().max(20) as u64;
        let weak = self.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if let Some(this) = weak.upgrade() {
                this.advance();
            }
        });
    }

    fn advance(&self) {
        let advanced = match self.imp().iter.borrow().as_ref() {
            Some(iter) => iter.advance(SystemTime::now()),
            None => return,
        };
        if advanced && let Some(iter) = self.imp().iter.borrow().as_ref() {
            self.apply_frame(iter);
        }
        self.schedule();
    }
}
