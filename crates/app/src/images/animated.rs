//! `gdk::Paintable`s that step through the frames of a `PixbufAnimation`, or
//! of frames already made such as an APNG's, so GIF/WebP images animate.
//! GTK's `Picture` otherwise shows only the first frame of an animation.
//!
//! Frames advance only while somebody holds a [`Lease`] from [`play`], so a
//! paintable nobody can see costs no timer, and widgets showing the same
//! paintable keep it going between them.

use std::cell::{Cell, RefCell};
use std::time::{Duration, SystemTime};

use gtk::gdk;
use gtk::gdk_pixbuf::{PixbufAnimation, PixbufAnimationIter};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// A claim on a paintable's frames advancing. Dropping it gives the claim
/// up; the frames stop when no claim is left.
pub struct Lease(Option<Box<dyn FnOnce()>>);

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            release();
        }
    }
}

/// Make `paintable` advance for as long as the lease lives. `None` for one
/// that doesn't move.
pub fn play(paintable: &gdk::Paintable) -> Option<Lease> {
    if let Some(this) = paintable.downcast_ref::<AnimatedPaintable>() {
        this.imp().timer.hold();
        this.schedule();
        let weak = this.downgrade();
        return Some(Lease(Some(Box::new(move || {
            if let Some(this) = weak.upgrade() {
                this.imp().timer.release();
            }
        }))));
    }
    if let Some(this) = paintable.downcast_ref::<FramePaintable>() {
        this.imp().timer.hold();
        this.schedule();
        let weak = this.downgrade();
        return Some(Lease(Some(Box::new(move || {
            if let Some(this) = weak.upgrade() {
                this.imp().timer.release();
            }
        }))));
    }
    None
}

/// The one timer of a paintable and how many hold it.
#[derive(Default)]
pub struct Timer {
    holders: Cell<u32>,
    source: RefCell<Option<glib::SourceId>>,
}

impl Timer {
    fn hold(&self) {
        self.holders.set(self.holders.get() + 1);
    }

    fn release(&self) {
        self.holders.set(self.holders.get().saturating_sub(1));
        if self.holders.get() == 0
            && let Some(source) = self.source.take()
        {
            source.remove();
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AnimatedPaintable {
        pub iter: RefCell<Option<PixbufAnimationIter>>,
        pub texture: RefCell<Option<gdk::Texture>>,
        pub size: RefCell<(i32, i32)>,
        pub timer: Timer,
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
        let timer = &self.imp().timer;
        if timer.holders.get() == 0 || timer.source.borrow().is_some() {
            return;
        }
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
        let source = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if let Some(this) = weak.upgrade() {
                this.imp().timer.source.take();
                this.advance();
            }
        });
        *timer.source.borrow_mut() = Some(source);
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

mod frames {
    use super::*;

    #[derive(Default)]
    pub struct FramePaintable {
        pub frames: RefCell<Vec<(gdk::Texture, Duration)>>,
        pub at: Cell<usize>,
        /// Plays still to go once this one ends; `None` is for ever.
        pub plays_left: Cell<Option<u32>>,
        pub timer: Timer,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FramePaintable {
        const NAME: &'static str = "NeucordFramePaintable";
        type Type = super::FramePaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for FramePaintable {}

    impl PaintableImpl for FramePaintable {
        fn intrinsic_width(&self) -> i32 {
            self.frames.borrow().first().map_or(0, |(t, _)| t.width())
        }

        fn intrinsic_height(&self) -> i32 {
            self.frames.borrow().first().map_or(0, |(t, _)| t.height())
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if let Some((texture, _)) = self.frames.borrow().get(self.at.get()) {
                texture.snapshot(snapshot, width, height);
            }
        }
    }
}

glib::wrapper! {
    /// Frames that are shown in turn, each for its own time.
    pub struct FramePaintable(ObjectSubclass<frames::FramePaintable>) @implements gdk::Paintable;
}

impl FramePaintable {
    /// `frames` in order. `plays` is how many times they go through; 0 is for
    /// ever, and when they stop the last frame stays.
    pub fn new(frames: Vec<(gdk::Texture, Duration)>, plays: u32) -> Self {
        let this: Self = glib::Object::new();
        let imp = this.imp();
        *imp.frames.borrow_mut() = frames;
        imp.plays_left.set((plays != 0).then(|| plays - 1));
        this
    }

    fn schedule(&self) {
        let imp = self.imp();
        if imp.timer.holders.get() == 0 || imp.timer.source.borrow().is_some() {
            return;
        }
        let delay = imp.frames.borrow().get(imp.at.get()).map_or(Duration::ZERO, |(_, d)| *d);
        // No fixed rate is no faster than the GIF path goes.
        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(delay.max(Duration::from_millis(20)), move || {
            if let Some(this) = weak.upgrade() {
                this.imp().timer.source.take();
                this.advance();
            }
        });
        *imp.timer.source.borrow_mut() = Some(source);
    }

    fn advance(&self) {
        let imp = self.imp();
        let next = imp.at.get() + 1;
        if next >= imp.frames.borrow().len() {
            match imp.plays_left.get() {
                // The last frame stays.
                Some(0) => return,
                Some(left) => imp.plays_left.set(Some(left - 1)),
                None => {}
            }
            imp.at.set(0);
        } else {
            imp.at.set(next);
        }
        self.invalidate_contents();
        self.schedule();
    }
}
