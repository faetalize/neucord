//! Text with images among the words, and spoilers that hide until clicked.
//!
//! A wrapping, selectable `Label` lays out the text and handles links and
//! selection. Each image is an object replacement character whose shape
//! attribute leaves room for it in the layout, and is drawn into that room
//! over the label; hidden spoilers are covered the same way. A `TextView`
//! holds images by itself, but measures its height only for the width it
//! last had, which a list view sizing its rows can't wait for.

use std::cell::{Cell, OnceCell, RefCell};
use std::ops::Range;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango};

/// What stands in the text for each image.
pub const OBJECT: char = '\u{FFFC}';

/// One image in the text.
pub struct Image {
    /// Byte index of its [`OBJECT`] in the label's text.
    pub index: u32,
    /// Its side, in logical pixels.
    pub size: i32,
    /// What copying the text gives for it, and its tooltip.
    pub alt: String,
    pub paintable: Option<gdk::Paintable>,
}

type RevealHandler = Box<dyn Fn(usize)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RichText {
        pub label: OnceCell<gtk::Label>,
        pub images: RefCell<Vec<Image>>,
        /// Byte ranges of the spoilers, and whether each is revealed.
        pub spoilers: RefCell<Vec<(Range<u32>, bool)>>,
        pub on_reveal: RefCell<Option<RevealHandler>>,
        /// The pointer is over a hidden spoiler.
        pub over_spoiler: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RichText {
        const NAME: &'static str = "NeucordRichText";
        type Type = super::RichText;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for RichText {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let label = gtk::Label::builder()
                .use_markup(true)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .xalign(0.0)
                .selectable(true)
                .build();
            label.set_parent(&*obj);
            // Copying gives emoji as `:name:`, not the character standing in.
            label.connect_copy_clipboard({
                let obj = obj.downgrade();
                move |label| {
                    let Some(obj) = obj.upgrade() else { return };
                    if obj.imp().images.borrow().is_empty() {
                        return;
                    }
                    label.stop_signal_emission_by_name("copy-clipboard");
                    if let Some(text) = obj.selected_text() {
                        label.clipboard().set_text(&text);
                    }
                }
            });
            self.label.set(label).expect("constructed once");

            // A click on a hidden spoiler reveals it, and goes no further: a
            // link under it doesn't open.
            let click = gtk::GestureClick::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
            click.connect_pressed({
                let obj = obj.downgrade();
                move |gesture, _, x, y| {
                    let Some(obj) = obj.upgrade() else { return };
                    if let Some(n) = obj.hidden_spoiler_at(x, y) {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        obj.reveal(n);
                    }
                }
            });
            obj.add_controller(click);
            // Ahead of the label, which points at links as the pointer
            // moves onto them.
            let motion =
                gtk::EventControllerMotion::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
            motion.connect_motion({
                let obj = obj.downgrade();
                move |_, x, y| {
                    let Some(obj) = obj.upgrade() else { return };
                    let over = obj.hidden_spoiler_at(x, y).is_some();
                    if obj.imp().over_spoiler.replace(over) != over {
                        obj.label().set_cursor_from_name(Some(if over { "pointer" } else { "text" }));
                    }
                }
            });
            obj.add_controller(motion);
            obj.connect_query_tooltip(|obj, x, y, _, tooltip| {
                let Some(n) = obj.image_at(x as f64, y as f64) else { return false };
                tooltip.set_text(Some(&obj.imp().images.borrow()[n].alt));
                true
            });
        }

        fn dispose(&self) {
            if let Some(label) = self.label.get() {
                label.unparent();
            }
        }
    }

    impl WidgetImpl for RichText {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.obj().label().request_mode()
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.obj().label().measure(orientation, for_size)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.obj().label().allocate(width, height, baseline, None);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let label = obj.label();
            obj.snapshot_child(label, snapshot);
            let layout = label.layout();
            let (ox, oy) = label.layout_offsets();
            for (image, (x, y)) in self.images.borrow().iter().zip(image_positions(&layout, &self.images.borrow())) {
                let Some(paintable) = &image.paintable else { continue };
                snapshot.save();
                snapshot.translate(&graphene::Point::new((ox + x) as f32, (oy + y) as f32));
                paintable.snapshot(snapshot, image.size as f64, image.size as f64);
                snapshot.restore();
            }
            // Spoilers that are still hidden, covered, images and all.
            let cover = gdk::RGBA::new(0.5, 0.5, 0.5, 1.0);
            for (range, _) in self.spoilers.borrow().iter().filter(|(_, revealed)| !revealed) {
                for rect in range_rects(&layout, range) {
                    let rect =
                        graphene::Rect::new((ox + rect.0) as f32, (oy + rect.1) as f32, rect.2 as f32, rect.3 as f32);
                    let rounded = gsk::RoundedRect::from_rect(rect, 3.0);
                    snapshot.push_rounded_clip(&rounded);
                    snapshot.append_color(&cover, &rect);
                    snapshot.pop();
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct RichText(ObjectSubclass<imp::RichText>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl RichText {
    /// Text from Pango `markup`, with `images` and `spoilers` (byte ranges
    /// of its text, and whether each starts revealed).
    pub fn new(markup: &str, images: Vec<Image>, spoilers: Vec<(Range<u32>, bool)>) -> Self {
        let this: Self = glib::Object::new();
        let label = this.label();
        label.set_markup(markup);
        if !images.is_empty() {
            let attributes = pango::AttrList::new();
            for image in &images {
                let size = image.size * pango::SCALE;
                // Most of it above the baseline, as text sits.
                let logical = pango::Rectangle::new(0, -size * 4 / 5, size, size);
                let mut shape = pango::AttrShape::new(&logical, &logical);
                shape.set_start_index(image.index);
                shape.set_end_index(image.index + OBJECT.len_utf8() as u32);
                attributes.insert(shape);
            }
            label.set_attributes(Some(&attributes));
        }
        // Images name themselves in a tooltip.
        this.set_has_tooltip(!images.is_empty());
        *this.imp().images.borrow_mut() = images;
        *this.imp().spoilers.borrow_mut() = spoilers;
        this
    }

    pub fn label(&self) -> &gtk::Label {
        self.imp().label.get().expect("constructed")
    }

    /// Show the `n`th image, once it has loaded. Animated images redraw the
    /// text as they play.
    pub fn set_image(&self, n: usize, paintable: gdk::Paintable) {
        if !paintable.flags().contains(gdk::PaintableFlags::STATIC_CONTENTS) {
            let weak = self.downgrade();
            paintable.connect_invalidate_contents(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.queue_draw();
                }
            });
        }
        if let Some(image) = self.imp().images.borrow_mut().get_mut(n) {
            image.paintable = Some(paintable);
        }
        self.queue_draw();
    }

    /// Called with a spoiler's number when the reader reveals it.
    pub fn connect_reveal(&self, f: impl Fn(usize) + 'static) {
        *self.imp().on_reveal.borrow_mut() = Some(Box::new(f));
    }

    fn reveal(&self, n: usize) {
        if let Some((_, revealed)) = self.imp().spoilers.borrow_mut().get_mut(n) {
            *revealed = true;
        }
        self.queue_draw();
        if let Some(on_reveal) = self.imp().on_reveal.borrow().as_ref() {
            on_reveal(n);
        }
    }

    /// The byte index of the text under a point, if there is text there.
    fn index_at(&self, x: f64, y: f64) -> Option<u32> {
        let label = self.label();
        let (ox, oy) = label.layout_offsets();
        let to_pango = |v: f64, offset: i32| ((v - offset as f64) * pango::SCALE as f64) as i32;
        let (inside, index, _) = label.layout().xy_to_index(to_pango(x, ox), to_pango(y, oy));
        inside.then_some(index as u32)
    }

    fn hidden_spoiler_at(&self, x: f64, y: f64) -> Option<usize> {
        let spoilers = self.imp().spoilers.borrow();
        if spoilers.iter().all(|(_, revealed)| *revealed) {
            return None;
        }
        let index = self.index_at(x, y)?;
        spoilers.iter().position(|(range, revealed)| !revealed && range.contains(&index))
    }

    fn image_at(&self, x: f64, y: f64) -> Option<usize> {
        let images = self.imp().images.borrow();
        if images.is_empty() {
            return None;
        }
        let index = self.index_at(x, y)?;
        images.iter().position(|image| image.index == index)
    }

    /// The selection, with images as their alt text.
    fn selected_text(&self) -> Option<String> {
        let label = self.label();
        let (start, end) = label.selection_bounds()?;
        let text = label.layout().text();
        let images = self.imp().images.borrow();
        let mut alts = images.iter();
        let mut out = String::new();
        for (i, c) in text.chars().enumerate() {
            let alt = if c == OBJECT { alts.next() } else { None };
            if (start as usize..end as usize).contains(&i) {
                match alt {
                    Some(image) => out.push_str(&image.alt),
                    None => out.push(c),
                }
            }
        }
        Some(out)
    }
}

/// Where each image's top left corner goes, in layout pixels.
fn image_positions(layout: &pango::Layout, images: &[Image]) -> Vec<(i32, i32)> {
    images
        .iter()
        .map(|image| {
            let (line, _) = layout.index_to_line_x(image.index as i32, false);
            let baseline = line_baseline(layout, line);
            let x = layout.index_to_pos(image.index as i32).x();
            let size = image.size * pango::SCALE;
            (x / pango::SCALE, (baseline - size * 4 / 5) / pango::SCALE)
        })
        .collect()
}

fn line_baseline(layout: &pango::Layout, line: i32) -> i32 {
    let mut iter = layout.iter();
    for _ in 0..line {
        if !iter.next_line() {
            break;
        }
    }
    iter.baseline()
}

/// The rectangles a byte range covers, a line at a time, in layout pixels.
fn range_rects(layout: &pango::Layout, range: &Range<u32>) -> Vec<(i32, i32, i32, i32)> {
    let mut rects = Vec::new();
    let mut iter = layout.iter();
    loop {
        if let Some(line) = iter.line_readonly() {
            let start = line.start_index() as u32;
            let end = start + line.length() as u32;
            if range.start < end && range.end > start {
                let (_, logical) = iter.line_extents();
                let (y0, y1) = (logical.y() / pango::SCALE, (logical.y() + logical.height()) / pango::SCALE);
                // A range comes back in pieces, one per run of text: join
                // the ones that touch, so the cover has no seams.
                let mut pieces: Vec<(i32, i32)> = Vec::new();
                let ranges = line.x_ranges(range.start.max(start) as i32, range.end.min(end) as i32);
                for [x0, x1] in ranges.as_chunks::<2>().0 {
                    let (x0, x1) = (x0 / pango::SCALE, x1 / pango::SCALE);
                    match pieces.last_mut() {
                        Some(last) if x0 <= last.1 + 1 => last.1 = last.1.max(x1),
                        _ => pieces.push((x0, x1)),
                    }
                }
                rects.extend(pieces.into_iter().map(|(x0, x1)| (x0, y0, (x1 - x0).max(1), y1 - y0)));
            }
        }
        if !iter.next_line() {
            return rects;
        }
    }
}
