//! Loading an image only once it is near the screen, as the client's
//! `LazyImage` does for the accounts in `2026-02-lazy-load-all-images`
//! (619517): an `IntersectionObserver` with a 300 px margin and no
//! threshold starts the load when any part of the image comes within that of
//! the part of its scroller that shows. Without the experiment an image
//! loads when it is made.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;

/// How far outside the visible part an image still counts as near.
const MARGIN: f32 = 300.0;

type Start = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;
/// The scroll position, the scroller's extent and the widget's size.
type Seen = (i32, i32, i32, i32, i32);

/// Whether a box at `at` (x, y, width, height) in a scroller showing
/// `viewport` (width, height) reaches within [`MARGIN`] of it.
fn near(at: (f32, f32, f32, f32), viewport: (f32, f32)) -> bool {
    let (x, y, width, height) = at;
    x < viewport.0 + MARGIN && x + width > -MARGIN && y < viewport.1 + MARGIN && y + height > -MARGIN
}

/// How long the picture's place has to stay put before it counts. A list that
/// is built at its top and then scrolled to the end shows the top for a
/// moment, which the client's observer never sees; its images are not wanted.
const SETTLE: Duration = Duration::from_millis(120);

/// Call `start` once `widget` is near the screen, at once if it is not in a
/// scroller. An image that is never near is never loaded.
pub fn when_near(widget: &impl IsA<gtk::Widget>, start: impl FnOnce() + 'static) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    let start: Start = Rc::new(RefCell::new(Some(Box::new(start))));
    // Where it was last looked at, since when, and whether that was judged.
    let seen: Cell<Option<(Seen, Instant, bool)>> = Cell::new(None);
    widget.add_tick_callback(move |widget, _| {
        if start.borrow().is_none() {
            return glib::ControlFlow::Break;
        }
        if !widget.is_mapped() {
            return glib::ControlFlow::Continue;
        }
        let scroller = std::iter::successors(widget.parent(), |w| w.parent())
            .find_map(|w| w.downcast::<gtk::ScrolledWindow>().ok());
        let ready = match &scroller {
            None => true,
            Some(scroller) => {
                let key = (
                    scroller.vadjustment().value() as i32,
                    scroller.vadjustment().upper() as i32,
                    scroller.hadjustment().value() as i32,
                    widget.width(),
                    widget.height(),
                );
                // Not laid out yet: nothing is known about where it is.
                if widget.width() == 0 || widget.height() == 0 {
                    false
                } else {
                    let (since, judged) = match seen.get() {
                        Some((last, since, judged)) if last == key => (since, judged),
                        _ => (Instant::now(), false),
                    };
                    let settled = since.elapsed() >= SETTLE;
                    seen.set(Some((key, since, judged || settled)));
                    settled
                        && !judged
                        && widget.compute_bounds(scroller).is_some_and(|b| {
                            near(
                                (b.x(), b.y(), b.width(), b.height()),
                                (scroller.width() as f32, scroller.height() as f32),
                            )
                        })
                }
            }
        };
        if ready && let Some(start) = start.borrow_mut().take() {
            start();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_reaches_three_hundred_pixels_beyond_the_screen() {
        let screen = (800.0, 600.0);
        assert!(near((0.0, 0.0, 100.0, 100.0), screen), "on screen");
        assert!(near((0.0, 899.0, 100.0, 100.0), screen), "just below");
        assert!(!near((0.0, 900.0, 100.0, 100.0), screen), "300 below is not within 300");
        assert!(near((0.0, -399.0, 100.0, 100.0), screen), "its bottom edge just above");
        assert!(!near((0.0, -400.0, 100.0, 100.0), screen));
        assert!(near((1099.0, 0.0, 10.0, 10.0), screen), "beside it too");
        assert!(!near((1100.0, 0.0, 10.0, 10.0), screen));
    }
}
