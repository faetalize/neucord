//! Side panes that can be dragged wider or narrower. libadwaita's split
//! views have no handle, but they size a sidebar by its minimum and maximum
//! width, so pinning both to the dragged width resizes it and keeps their
//! collapsing.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

/// How far from a sidebar's inner edge a press still grabs it, in px.
const GRAB: f64 = 4.0;

/// Give `split`'s sidebar `width` px, and let it be dragged within `range`
/// from its inner edge while it sits beside the content.
pub(super) fn resizable(split: &adw::OverlaySplitView, width: f64, range: (f64, f64)) {
    split.set_sidebar_width_unit(adw::LengthUnit::Px);
    set_width(split, width);

    // On the split view, not on a handle widget: a handle moves with the
    // edge, which would feed the drag back into its own offsets.
    let drag = gtk::GestureDrag::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
    let start = Rc::new(Cell::new(width));
    drag.connect_drag_begin({
        let start = start.clone();
        move |drag, x, _| {
            let Some(split) = drag.widget().and_downcast::<adw::OverlaySplitView>() else { return };
            if !on_edge(&split, x) {
                drag.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            drag.set_state(gtk::EventSequenceState::Claimed);
            start.set(split.max_sidebar_width());
        }
    });
    drag.connect_drag_update(move |drag, dx, _| {
        let Some(split) = drag.widget().and_downcast::<adw::OverlaySplitView>() else { return };
        let dx = if split.sidebar_position() == gtk::PackType::Start { dx } else { -dx };
        set_width(&split, (start.get() + dx).clamp(range.0, range.1));
    });
    split.add_controller(drag);

    let hover = gtk::EventControllerMotion::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
    hover.connect_motion(|hover, x, _| {
        let Some(split) = hover.widget().and_downcast::<adw::OverlaySplitView>() else { return };
        split.set_cursor_from_name(on_edge(&split, x).then_some("col-resize"));
    });
    hover.connect_leave(|hover| {
        if let Some(split) = hover.widget() {
            split.set_cursor(None);
        }
    });
    split.add_controller(hover);
}

/// Whether `x` is on the sidebar's inner edge, with the sidebar beside the content.
fn on_edge(split: &adw::OverlaySplitView, x: f64) -> bool {
    if split.is_collapsed() || !split.shows_sidebar() || split.sidebar().is_none() {
        return false;
    }
    let width = split.max_sidebar_width();
    let edge = match split.sidebar_position() {
        gtk::PackType::Start => width,
        _ => f64::from(split.width()) - width,
    };
    (x - edge).abs() <= GRAB
}

fn set_width(split: &adw::OverlaySplitView, width: f64) {
    split.set_min_sidebar_width(width);
    split.set_max_sidebar_width(width);
}
