//! The media viewer: the official client's media modal. A layer over the
//! window, dimmed, with the picture or video of a message fitted to it, and
//! the arrows and strip of thumbnails to step through the others that open
//! with it. A click outside the media, or Escape, closes it.
//!
//! What it loads, and how big, is [`discord::media`] and [`discord::cdn`]'s.
//! The actions on media (Copy Image, Save, Copy Link, Open) are here too, for
//! the menus of messages to share.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use discord::media::{self, Item, Kind, LinkKind, Source};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::App;
use super::media_frame::MediaFrame;
use super::menus::{self, At, Entry};
use super::video::{self, Control, Player};
use crate::images::{Images, Lease};

/// How much the zoom enlarges a picture (the client's `setZoomed`).
const ZOOM: f64 = 2.5;
/// A press that moves less than this (squared) is a click, not a drag.
const CLICK_RADIUS_SQ: f64 = 400.0;
/// Thumbnail edge.
const THUMB: i32 = 40;
/// Marks what a click does not close the viewer through.
const KEEP: &str = "viewer-keep";

/// Who sent the message the media came from, and when.
#[derive(Clone)]
pub(super) struct Origin {
    pub(super) author: String,
    pub(super) time: String,
}

/// What to open.
pub(super) struct Show {
    pub(super) items: Vec<Item>,
    pub(super) at: usize,
    pub(super) origin: Option<Origin>,
    /// The media's options are hidden (a forum's `HIDE_MEDIA_DOWNLOAD_OPTIONS`).
    pub(super) hide_options: bool,
    /// Where a video starts, in seconds.
    pub(super) start: f64,
}

/// The layer itself; one lives in the window.
#[derive(Clone)]
pub(super) struct Viewer(Rc<Inner>);

struct Inner {
    root: gtk::Overlay,
    stage: gtk::ScrolledWindow,
    holder: gtk::Box,
    info: gtk::Label,
    zoom: gtk::Button,
    save: gtk::Button,
    open: gtk::Button,
    more: gtk::MenuButton,
    previous: gtk::Button,
    next: gtk::Button,
    strip: gtk::Box,
    strip_scroll: gtk::ScrolledWindow,
    actions: gio::SimpleActionGroup,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    app: Weak<App>,
    open: Option<Open>,
}

struct Open {
    items: Vec<Item>,
    index: usize,
    origin: Option<Origin>,
    hide_options: bool,
    start: f64,
    /// Bumped with every item, so a load that finishes late is dropped.
    generation: u32,
    zoomed: bool,
    /// The box the media is laid out in, and the size the proxy was asked for.
    fitted: (u32, u32),
    loaded: (u32, u32),
    /// What is shown now, as loaded: what a menu on it copies.
    src: String,
    picture: Option<gtk::Picture>,
    /// Keeps an animation's frames advancing while it is shown.
    lease: Option<Lease>,
    /// What holds the picture or player at the size it is shown at.
    frame: Option<MediaFrame>,
    player: Option<Player>,
    tick: Option<gtk::TickCallbackId>,
    previous_focus: Option<glib::WeakRef<gtk::Widget>>,
    fullscreened: bool,
    last_window: (i32, i32),
}

impl Viewer {
    pub(super) fn new() -> Viewer {
        let root = gtk::Overlay::builder().visible(false).focusable(true).css_classes(["media-viewer"]).build();
        let holder = gtk::Box::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        let stage = gtk::ScrolledWindow::builder()
            .child(&holder)
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .build();
        root.set_child(Some(&stage));

        let round = |icon: &str, tip: &str| {
            gtk::Button::builder().icon_name(icon).tooltip_text(tip).css_classes(["osd", "circular"]).build()
        };
        let info = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["heading"]).build();
        let zoom = round("zoom-in-symbolic", "Zoom In");
        let save = round("document-save-symbolic", "Save Image");
        let open = round("external-link-symbolic", "Open in Browser");
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .css_classes(["osd", "circular"])
            .build();
        let close = round("window-close-symbolic", "Close");
        let top = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_start(16)
            .margin_end(16)
            .build();
        for widget in [
            info.upcast_ref::<gtk::Widget>(),
            zoom.upcast_ref(),
            save.upcast_ref(),
            open.upcast_ref(),
            more.upcast_ref(),
            close.upcast_ref(),
        ] {
            top.append(widget);
        }
        keep(&top);
        root.add_overlay(&top);

        let previous = round("go-previous-symbolic", "Previous");
        previous.set_halign(gtk::Align::Start);
        previous.set_valign(gtk::Align::Center);
        previous.set_margin_start(16);
        let next = round("go-next-symbolic", "Next");
        next.set_halign(gtk::Align::End);
        next.set_valign(gtk::Align::Center);
        next.set_margin_end(16);
        root.add_overlay(&previous);
        root.add_overlay(&next);

        let strip = gtk::Box::builder().spacing(4).halign(gtk::Align::Center).build();
        let strip_scroll = gtk::ScrolledWindow::builder()
            .child(&strip)
            .valign(gtk::Align::End)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .visible(false)
            .build();
        keep(&strip_scroll);
        root.add_overlay(&strip_scroll);

        let actions = gio::SimpleActionGroup::new();
        root.insert_action_group("viewer", Some(&actions));
        let inner = Rc::new(Inner {
            root,
            stage,
            holder,
            info,
            zoom,
            save,
            open,
            more,
            previous,
            next,
            strip,
            strip_scroll,
            actions,
            state: RefCell::default(),
        });
        let viewer = Viewer(inner);
        viewer.connect(&close);
        viewer
    }

    pub(super) fn widget(&self) -> &gtk::Overlay {
        &self.0.root
    }

    fn connect(&self, close: &gtk::Button) {
        let inner = &self.0;
        let weak = Rc::downgrade(inner);
        let with = move |f: fn(&Viewer)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&Viewer(inner));
                }
            }
        };
        close.connect_clicked({
            let run = with(Viewer::close);
            move |_| run()
        });
        inner.previous.connect_clicked({
            let run = with(|v| v.step(-1));
            move |_| run()
        });
        inner.next.connect_clicked({
            let run = with(|v| v.step(1));
            move |_| run()
        });
        inner.zoom.connect_clicked({
            let run = with(|v| v.toggle_zoom(None));
            move |_| run()
        });
        inner.save.connect_clicked({
            let run = with(Viewer::save);
            move |_| run()
        });
        inner.open.connect_clicked({
            let run = with(Viewer::open_in_browser);
            move |_| run()
        });
        for (name, act) in [
            ("copy-image", Viewer::copy_image as fn(&Viewer)),
            ("copy-link", Viewer::copy_link),
            ("copy-attachment-id", Viewer::copy_attachment_id),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let run = with(act);
            action.connect_activate(move |_, _| run());
            inner.actions.add_action(&action);
        }

        // A click outside the media closes it; a click on it or on a control doesn't.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        click.connect_released({
            let run = with(Viewer::close);
            let root = inner.root.downgrade();
            move |_, _, x, y| {
                let Some(root) = root.upgrade() else { return };
                let mut widget = root.pick(x, y, gtk::PickFlags::DEFAULT);
                while let Some(w) = widget {
                    if w.has_css_class(KEEP) {
                        return;
                    }
                    widget = w.parent();
                }
                run();
            }
        });
        inner.root.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = Rc::downgrade(inner);
            move |_, key, _, _| {
                let Some(inner) = weak.upgrade() else { return glib::Propagation::Proceed };
                let viewer = Viewer(inner);
                if viewer.key(key) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
            }
        });
        inner.root.add_controller(keys);
    }

    /// Show `show` over the window, `app`'s.
    pub(super) fn open(&self, app: &Rc<App>, show: Show) {
        if show.items.is_empty() {
            return;
        }
        let inner = &self.0;
        let at = show.at.min(show.items.len() - 1);
        {
            let mut state = inner.state.borrow_mut();
            let previous_focus = RootExt::focus(&app.window).map(|w| w.downgrade());
            state.app = Rc::downgrade(app);
            state.open = Some(Open {
                items: show.items,
                index: at,
                origin: show.origin,
                hide_options: show.hide_options,
                start: show.start,
                generation: 0,
                zoomed: false,
                fitted: (0, 0),
                loaded: (0, 0),
                src: String::new(),
                picture: None,
                lease: None,
                frame: None,
                player: None,
                tick: None,
                previous_focus,
                fullscreened: false,
                last_window: (0, 0),
            });
        }
        inner.root.set_visible(true);
        self.build_strip();
        self.show_item(at);
        // Once it is shown: a widget that isn't can't take the focus.
        let root = inner.root.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(root) = root.upgrade() {
                root.grab_focus();
            }
        });
        // The layer follows the window's size, as the client's follows its viewport.
        let weak = Rc::downgrade(inner);
        let tick = inner.root.add_tick_callback(move |root, _| {
            if let Some(inner) = weak.upgrade() {
                let size = (root.width(), root.height());
                let changed = inner.state.borrow().open.as_ref().is_some_and(|open| open.last_window != size);
                if changed {
                    Viewer(inner).relayout();
                }
            }
            glib::ControlFlow::Continue
        });
        if let Some(open) = inner.state.borrow_mut().open.as_mut() {
            open.tick = Some(tick);
        }
    }

    pub(super) fn close(&self) {
        let inner = &self.0;
        let Some(open) = inner.state.borrow_mut().open.take() else { return };
        let app = inner.state.borrow().app.upgrade();
        if let Some(player) = &open.player {
            player.control().pause();
        }
        if let Some(tick) = open.tick {
            tick.remove();
        }
        if open.fullscreened
            && let Some(app) = &app
        {
            app.window.set_fullscreened(false);
        }
        inner.root.set_visible(false);
        while let Some(child) = inner.holder.first_child() {
            inner.holder.remove(&child);
        }
        inner.info.set_label("");
        if let Some(focus) = open.previous_focus.and_then(|w| w.upgrade()) {
            focus.grab_focus();
        }
    }

    /// Whether `key` was the viewer's.
    fn key(&self, key: gdk::Key) -> bool {
        if self.0.state.borrow().open.is_none() {
            return false;
        }
        let player: Option<Control> =
            self.0.state.borrow().open.as_ref().and_then(|open| open.player.as_ref().map(Player::control));
        match key {
            gdk::Key::Escape => self.close(),
            gdk::Key::Left => self.step(-1),
            gdk::Key::Right => self.step(1),
            gdk::Key::space if player.is_some() => player.iter().for_each(Control::toggle),
            _ => {
                // 0 to 9 jump to that tenth of a video.
                let digit = key.to_unicode().and_then(|c| c.to_digit(10));
                match (digit, player) {
                    (Some(tenth), Some(player)) => player.seek_tenth(tenth),
                    _ => return false,
                }
            }
        }
        true
    }

    /// To the next or previous, around the ends.
    fn step(&self, by: isize) {
        let (index, len) = {
            let state = self.0.state.borrow();
            let Some(open) = state.open.as_ref() else { return };
            (open.index, open.items.len())
        };
        if len > 1 {
            self.show_item(((index as isize + by).rem_euclid(len as isize)) as usize);
        }
    }

    fn app(&self) -> Option<Rc<App>> {
        self.0.state.borrow().app.upgrade()
    }

    fn item(&self) -> Option<Item> {
        let state = self.0.state.borrow();
        let open = state.open.as_ref()?;
        open.items.get(open.index).cloned()
    }

    fn window_size(&self) -> (i32, i32) {
        (self.0.root.width(), self.0.root.height())
    }

    /// Lay the shown item out again in the room the window leaves.
    fn relayout(&self) {
        let Some(item) = self.item() else { return };
        let (window, several, zoomed) = {
            let mut state = self.0.state.borrow_mut();
            let Some(open) = state.open.as_mut() else { return };
            open.last_window = self.window_size();
            (open.last_window, open.items.len() > 1, open.zoomed)
        };
        let fitted = media::viewer_box(window, several, item.size);
        let (picture, frame, loaded) = {
            let mut state = self.0.state.borrow_mut();
            let Some(open) = state.open.as_mut() else { return };
            open.fitted = fitted;
            (open.picture.clone(), open.frame.clone(), open.loaded)
        };
        let scale = if zoomed { ZOOM } else { 1.0 };
        let request = ((f64::from(fitted.0) * scale) as i32, (f64::from(fitted.1) * scale) as i32);
        if let Some(frame) = &frame {
            frame.set_size(request);
        }
        // A larger box wants more pixels than the proxy was asked for.
        if picture.is_some() && !zoomed && (fitted.0 > loaded.0 || fitted.1 > loaded.1) {
            self.load_picture(&item);
        }
    }

    /// The item at `index`: its media, and everything about it.
    fn show_item(&self, index: usize) {
        let inner = &self.0;
        let Some(app) = self.app() else { return };
        let (item, several, hide, origin, start) = {
            let mut state = inner.state.borrow_mut();
            let Some(open) = state.open.as_mut() else { return };
            open.index = index.min(open.items.len() - 1);
            open.generation = open.generation.wrapping_add(1);
            open.zoomed = false;
            open.loaded = (0, 0);
            open.picture = None;
            open.frame = None;
            if let Some(player) = open.player.take() {
                player.control().pause();
            }
            (open.items[open.index].clone(), open.items.len() > 1, open.hide_options, open.origin.clone(), open.start)
        };
        while let Some(child) = inner.holder.first_child() {
            inner.holder.remove(&child);
        }
        inner.stage.set_hscrollbar_policy(gtk::PolicyType::Never);
        inner.stage.set_vscrollbar_policy(gtk::PolicyType::Never);

        inner.info.set_label(&origin.map(|o| format!("{} · {}", o.author, o.time)).unwrap_or_default());
        inner.previous.set_visible(several);
        inner.next.set_visible(several);
        inner.strip_scroll.set_visible(several);
        let image = item.kind == Kind::Image;
        let links = !hide && item.can_link();
        inner.zoom.set_visible(image);
        inner.zoom.set_icon_name("zoom-in-symbolic");
        inner.zoom.set_tooltip_text(Some("Zoom In"));
        inner.save.set_visible(!hide && if image { item.can_save() } else { true });
        inner.save.set_tooltip_text(Some(if image { "Save Image" } else { "Save Video" }));
        inner.open.set_visible(links);
        self.refresh_menu(&app, &item, hide);
        self.mark_strip(index);

        let window = self.window_size();
        let fitted = media::viewer_box(window, several, item.size);
        {
            let mut state = inner.state.borrow_mut();
            if let Some(open) = state.open.as_mut() {
                open.fitted = fitted;
                open.last_window = window;
            }
        }
        let spinner = adw::Spinner::builder().width_request(32).height_request(32).build();
        inner.holder.append(&spinner);
        match item.kind {
            Kind::Image => self.show_picture(&item, spinner.upcast_ref()),
            Kind::Video => self.show_video(&app, &item, start, spinner.upcast_ref()),
        }
        self.preload(&app, index);
    }

    fn show_picture(&self, item: &Item, spinner: &gtk::Widget) {
        let (fitted, generation) = self.layout_now();
        let picture = gtk::Picture::builder().can_shrink(true).content_fit(gtk::ContentFit::Contain).build();
        let frame = MediaFrame::new((fitted.0 as i32, fitted.1 as i32), &picture);
        frame.set_exact(true);
        frame.set_halign(gtk::Align::Center);
        frame.set_valign(gtk::Align::Center);
        keep(&frame);
        // A click zooms, a drag pans.
        let drag = gtk::GestureDrag::builder().button(gdk::BUTTON_PRIMARY).build();
        let origin = Rc::new(Cell::new((0.0, 0.0)));
        drag.connect_drag_begin({
            let (viewer, origin) = (self.clone(), origin.clone());
            move |_, _, _| {
                origin.set((viewer.0.stage.hadjustment().value(), viewer.0.stage.vadjustment().value()));
            }
        });
        drag.connect_drag_update({
            let (viewer, origin) = (self.clone(), origin.clone());
            move |_, dx, dy| {
                let zoomed = viewer.0.state.borrow().open.as_ref().is_some_and(|open| open.zoomed);
                if zoomed {
                    viewer.0.stage.hadjustment().set_value(origin.get().0 - dx);
                    viewer.0.stage.vadjustment().set_value(origin.get().1 - dy);
                }
            }
        });
        drag.connect_drag_end({
            let viewer = self.clone();
            let picture = picture.downgrade();
            move |gesture, dx, dy| {
                if dx * dx + dy * dy >= CLICK_RADIUS_SQ {
                    return;
                }
                let Some(picture) = picture.upgrade() else { return };
                let (x, y) = gesture.start_point().unwrap_or_default();
                let (w, h) = (f64::from(picture.width().max(1)), f64::from(picture.height().max(1)));
                viewer.toggle_zoom(Some((x / w, y / h)));
            }
        });
        picture.add_controller(drag);
        // The menu on it, for what it shows.
        let this = self.clone();
        menus::on_secondary(&picture, move |x, y| this.context_menu(x, y));
        self.0.holder.append(&frame);
        {
            let mut state = self.0.state.borrow_mut();
            if let Some(open) = state.open.as_mut().filter(|open| open.generation == generation) {
                open.picture = Some(picture);
                open.frame = Some(frame);
            }
        }
        self.load_picture_with(item, Some(spinner.clone()));
    }

    /// The box and generation the shown item is laid out with.
    fn layout_now(&self) -> ((u32, u32), u32) {
        let state = self.0.state.borrow();
        state.open.as_ref().map_or(((1, 1), 0), |open| (open.fitted, open.generation))
    }

    fn load_picture(&self, item: &Item) {
        self.load_picture_with(item, None);
    }

    /// Ask for the picture at the size it is shown at.
    fn load_picture_with(&self, item: &Item, spinner: Option<gtk::Widget>) {
        let Some(app) = self.app() else { return };
        let (fitted, generation, picture) = {
            let state = self.0.state.borrow();
            let Some(open) = state.open.as_ref() else { return };
            (open.fitted, open.generation, open.picture.clone())
        };
        let Some(picture) = picture else { return };
        let url = self.src_for(&app.images, item, fitted);
        {
            let mut state = self.0.state.borrow_mut();
            if let Some(open) = state.open.as_mut().filter(|open| open.generation == generation) {
                open.loaded = fitted;
                open.src = url.clone();
            }
        }
        let viewer = self.clone();
        // The viewer is dark, as the client's modal is.
        app.images.load_playing(&url, item.animated, move |paintable, lease| {
            let current = viewer.0.state.borrow().open.as_ref().is_some_and(|open| open.generation == generation);
            if current {
                picture.set_paintable(Some(&paintable));
                if let Some(open) = viewer.0.state.borrow_mut().open.as_mut() {
                    open.lease = lease;
                }
                if let Some(spinner) = spinner {
                    spinner.set_visible(false);
                }
            }
        });
    }

    /// The URL to load `item` from, in `fitted`: the proxy's, sized.
    fn src_for(&self, images: &Images, item: &Item, fitted: (u32, u32)) -> String {
        let src = item.file_url();
        match item.size {
            Some(size) => discord::cdn::modal_url(src, size, fitted, images.scale() as u32, item.animated),
            None => src.to_owned(),
        }
    }

    fn show_video(&self, app: &Rc<App>, item: &Item, start: f64, spinner: &gtk::Widget) {
        let (viewer, item, spinner) = (self.clone(), item.clone(), spinner.clone());
        let generation = self.layout_now().1;
        let images = app.images.clone();
        glib::spawn_future_local(async move {
            let path = images.file(item.file_url()).await;
            let current = viewer.0.state.borrow().open.as_ref().is_some_and(|open| open.generation == generation);
            if !current {
                return;
            }
            let Some(path) = path else { return };
            spinner.set_visible(false);
            let player = Player::new(&path, &video::Options { autoplay: true, start, fullscreen: true, classes: &[] });
            let fitted = viewer.layout_now().0;
            let frame = MediaFrame::new((fitted.0 as i32, fitted.1 as i32), player.widget());
            frame.set_exact(true);
            frame.set_halign(gtk::Align::Center);
            frame.set_valign(gtk::Align::Center);
            keep(&frame);
            player.fullscreen_button().connect_clicked({
                let viewer = viewer.clone();
                move |_| viewer.toggle_fullscreen()
            });
            let menu = viewer.clone();
            menus::on_secondary(player.widget(), move |x, y| menu.context_menu(x, y));
            viewer.0.holder.append(&frame);
            if let Some(open) = viewer.0.state.borrow_mut().open.as_mut() {
                open.src = item.file_url().to_owned();
                open.player = Some(player);
                open.frame = Some(frame);
            }
        });
    }

    fn toggle_fullscreen(&self) {
        let Some(app) = self.app() else { return };
        let on = !app.window.is_fullscreen();
        app.window.set_fullscreened(on);
        // Left as found once the viewer closes.
        if let Some(open) = self.0.state.borrow_mut().open.as_mut() {
            open.fullscreened = on;
        }
    }

    /// Zoom a picture in, around `at` (as a fraction of it), or out.
    fn toggle_zoom(&self, at: Option<(f64, f64)>) {
        let Some(item) = self.item() else { return };
        if item.kind != Kind::Image {
            return;
        }
        let (zoomed, picture, frame, fitted, generation) = {
            let mut state = self.0.state.borrow_mut();
            let Some(open) = state.open.as_mut() else { return };
            open.zoomed = !open.zoomed;
            (open.zoomed, open.picture.clone(), open.frame.clone(), open.fitted, open.generation)
        };
        let (Some(picture), Some(frame)) = (picture, frame) else { return };
        let inner = &self.0;
        let scale = if zoomed { ZOOM } else { 1.0 };
        let (w, h) = ((f64::from(fitted.0) * scale) as i32, (f64::from(fitted.1) * scale) as i32);
        frame.set_size((w, h));
        let policy = if zoomed { gtk::PolicyType::Automatic } else { gtk::PolicyType::Never };
        inner.stage.set_hscrollbar_policy(policy);
        inner.stage.set_vscrollbar_policy(policy);
        inner.zoom.set_icon_name(if zoomed { "zoom-out-symbolic" } else { "zoom-in-symbolic" });
        inner.zoom.set_tooltip_text(Some(if zoomed { "Zoom Out" } else { "Zoom In" }));
        if zoomed {
            // The place clicked stays under the pointer's middle of the stage.
            let (fx, fy) = at.unwrap_or((0.5, 0.5));
            let stage = inner.stage.clone();
            // Once the scroller has the new size to scroll in.
            stage.add_tick_callback(move |stage, _| {
                let (hadj, vadj) = (stage.hadjustment(), stage.vadjustment());
                if hadj.upper() < f64::from(w) - 1.0 {
                    return glib::ControlFlow::Continue;
                }
                hadj.set_value(fx * f64::from(w) - hadj.page_size() / 2.0);
                vadj.set_value(fy * f64::from(h) - vadj.page_size() / 2.0);
                glib::ControlFlow::Break
            });
            // Zoomed in, the original: what the client's `XW` gives.
            let Some(app) = self.app() else { return };
            let url = item.zoom_url().to_owned();
            let (viewer, picture) = (self.clone(), picture.clone());
            app.images.load_playing(&url, item.animated, move |paintable, lease| {
                let current = viewer
                    .0
                    .state
                    .borrow()
                    .open
                    .as_ref()
                    .is_some_and(|open| open.generation == generation && open.zoomed);
                if current {
                    picture.set_paintable(Some(&paintable));
                    if let Some(open) = viewer.0.state.borrow_mut().open.as_mut() {
                        open.lease = lease;
                    }
                }
            });
            if let Some(open) = inner.state.borrow_mut().open.as_mut() {
                open.src = url;
            }
        } else {
            self.load_picture(&item);
        }
    }

    /// Pictures next to the shown one are fetched ahead (`Z` in 415097): the
    /// one before always, the one after only if there are others besides.
    fn preload(&self, app: &Rc<App>, index: usize) {
        let state = self.0.state.borrow();
        let Some(open) = state.open.as_ref() else { return };
        let len = open.items.len();
        if len < 2 {
            return;
        }
        let mut neighbours = vec![(index + len - 1) % len];
        if len > 2 {
            neighbours.push((index + 1) % len);
        }
        let window = (self.0.root.width(), self.0.root.height());
        for n in neighbours {
            let item = &open.items[n];
            if item.kind != Kind::Image {
                continue;
            }
            let fitted = media::viewer_box(window, true, item.size);
            let url = self.src_for(&app.images, item, fitted);
            app.images.load(&url, |_| {});
        }
    }

    /// The thumbnails of the opened items, the first time and again if they change.
    fn build_strip(&self) {
        let inner = &self.0;
        while let Some(child) = inner.strip.first_child() {
            inner.strip.remove(&child);
        }
        let Some(app) = self.app() else { return };
        let items = match inner.state.borrow().open.as_ref() {
            Some(open) => open.items.clone(),
            None => return,
        };
        if items.len() < 2 {
            return;
        }
        for (n, item) in items.iter().enumerate() {
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .overflow(gtk::Overflow::Hidden)
                .build();
            // A picture asks for its own size; the thumbnail is this one.
            let thumb = MediaFrame::new((THUMB, THUMB), &picture);
            thumb.set_exact(true);
            // A video's still is what the proxy makes of it.
            let url = match item.kind {
                Kind::Image => item.file_url().to_owned(),
                Kind::Video => discord::cdn::with_params(item.file_url(), &[("format", "png")]),
            };
            app.images.set_sized(&picture, &url, (THUMB, THUMB), item.size);
            let button = gtk::Button::builder()
                .child(&thumb)
                .tooltip_text(format!("{} of {}", n + 1, items.len()))
                .css_classes(["flat"])
                .build();
            let viewer = self.clone();
            button.connect_clicked(move |_| viewer.show_item(n));
            inner.strip.append(&button);
        }
    }

    /// The shown item's thumbnail stands out among the others.
    fn mark_strip(&self, index: usize) {
        let mut n = 0;
        let mut child = self.0.strip.first_child();
        while let Some(button) = child {
            button.set_opacity(if n == index { 1.0 } else { 0.5 });
            child = button.next_sibling();
            n += 1;
        }
    }

    /// The More menu: Copy Image, Copy Link, and the attachment's id in
    /// Developer Mode (`eZ` in 2323).
    fn refresh_menu(&self, app: &Rc<App>, item: &Item, hide: bool) {
        let section = gio::Menu::new();
        if !hide && item.can_copy() {
            section.append(Some("Copy Image"), Some("viewer.copy-image"));
        }
        if !hide && item.can_link() {
            section.append(Some("Copy Media Link"), Some("viewer.copy-link"));
        }
        let developer = app.account.state.borrow().settings.developer_mode;
        if developer && matches!(item.source, Source::Attachment(_)) {
            section.append(Some("Copy Attachment ID"), Some("viewer.copy-attachment-id"));
        }
        let menu = gio::Menu::new();
        menu.append_section(None, &section);
        self.0.more.set_menu_model(Some(&menu));
        self.0.more.set_visible(section.n_items() > 0);
    }

    fn save(&self) {
        if let (Some(app), Some(item)) = (self.app(), self.item()) {
            save(&app, &item);
        }
    }

    fn open_in_browser(&self) {
        if let (Some(app), Some(item)) = (self.app(), self.item()) {
            open_link(&app, &item.link());
        }
    }

    fn copy_image(&self) {
        if let (Some(app), Some(item)) = (self.app(), self.item()) {
            copy_image(&app, &item);
        }
    }

    fn copy_link(&self) {
        if let (Some(app), Some(item)) = (self.app(), self.item()) {
            copy_link(&app, &item.link());
        }
    }

    fn copy_attachment_id(&self) {
        if let (Some(app), Some(item)) = (self.app(), self.item())
            && let Source::Attachment(id) = item.source
        {
            app.copy(&id.to_string());
        }
    }

    /// The menu of the picture or video: the client's `image-context` (177079),
    /// for what is shown. The link is the one loaded.
    fn context_menu(&self, x: f64, y: f64) {
        let (Some(app), Some(item)) = (self.app(), self.item()) else { return };
        let (src, hide) = {
            let state = self.0.state.borrow();
            let Some(open) = state.open.as_ref() else { return };
            (open.src.clone(), open.hide_options)
        };
        if hide {
            return;
        }
        let target = Target { item, src };
        menus::popup(&app, menus::VIEWER_MEDIA, target, &self.0.root, At::Point(x, y));
    }
}

/// Mark `widget` as one a click doesn't close the viewer through.
fn keep(widget: &impl IsA<gtk::Widget>) {
    widget.add_css_class(KEEP);
}

/// What the viewer's menu acts on.
pub(super) struct Target {
    item: Item,
    /// What the viewer loaded, which Copy Link copies.
    src: String,
}

/// The CDN's Copy Image and Save Image items, offered where the file can be
/// copied or saved (115184).
pub(super) const COPY_IMAGE: Entry<Target> = Entry::Item(menus::Item {
    name: "copy-image",
    label: "Copy _Image",
    shows: |_, target| target.item.can_copy(),
    kind: menus::Kind::Action(|app, target| copy_image(app, &target.item)),
});

pub(super) const SAVE_IMAGE: Entry<Target> = Entry::Item(menus::Item {
    name: "save-image",
    label: "_Save Image",
    shows: |_, target| target.item.kind == Kind::Image && target.item.can_save(),
    kind: menus::Kind::Action(|app, target| save(app, &target.item)),
});

/// Copy and open the link of what is shown, by the name its type gets
/// (777933). It is the one the viewer loaded.
pub(super) const COPY_LINK: Entry<Target> =
    menus::link_item!(Target, "copy-link", "Copy Lin_k", LinkKind::Other, copy_link, src_of, never);
pub(super) const COPY_IMAGE_LINK: Entry<Target> =
    menus::link_item!(Target, "copy-image-link", "Copy _Image Link", LinkKind::Image, copy_link, src_of, never);
pub(super) const COPY_MEDIA_LINK: Entry<Target> =
    menus::link_item!(Target, "copy-media-link", "Copy _Media Link", LinkKind::Media, copy_link, src_of, never);
pub(super) const OPEN_LINK: Entry<Target> =
    menus::link_item!(Target, "open-link", "_Open Link", LinkKind::Other, open_link, src_of, never);
pub(super) const OPEN_IMAGE_LINK: Entry<Target> =
    menus::link_item!(Target, "open-image-link", "_Open Image Link", LinkKind::Image, open_link, src_of, never);
pub(super) const OPEN_MEDIA_LINK: Entry<Target> =
    menus::link_item!(Target, "open-media-link", "_Open Media Link", LinkKind::Media, open_link, src_of, never);

fn src_of(target: &Target) -> Option<&str> {
    Some(&target.src)
}

fn never(_: &App, _: &Target) -> bool {
    false
}

/// Put the picture at `item`'s original size on the clipboard.
pub(super) fn copy_image(app: &Rc<App>, item: &Item) {
    let Some(url) = item.download_url() else { return };
    let app = app.clone();
    glib::spawn_future_local(async move {
        let texture = app.images.fetch(&url).await.and_then(|bytes| gdk::Texture::from_bytes(&bytes).ok());
        match texture {
            Some(texture) => {
                app.window.clipboard().set_texture(&texture);
                app.toast("Image Copied!");
            }
            None => app.toast("Unable to copy image"),
        }
    });
}

/// Ask where to put the file, then save it there.
pub(super) fn save(app: &Rc<App>, item: &Item) {
    let video = item.kind == Kind::Video;
    let dialog = gtk::FileDialog::builder()
        .title(if video { "Save Video" } else { "Save Image" })
        .initial_name(item.save_name())
        .build();
    let (app, item) = (app.clone(), item.clone());
    dialog.clone().save(Some(&app.window.clone()), gio::Cancellable::NONE, move |chosen| {
        // Cancelling is no error.
        let Ok(file) = chosen else { return };
        glib::spawn_future_local(async move {
            let bytes = if video {
                match app.images.file(item.file_url()).await {
                    Some(path) => std::fs::read(path).ok(),
                    None => None,
                }
            } else {
                match item.download_url() {
                    Some(url) => app.images.fetch(&url).await.map(|bytes| bytes.to_vec()),
                    None => None,
                }
            };
            let saved = match bytes {
                Some(bytes) => {
                    file.replace_contents_future(bytes, None, false, gio::FileCreateFlags::NONE).await.is_ok()
                }
                None => false,
            };
            app.toast(match (saved, video) {
                (true, false) => "Image Saved",
                (true, true) => "Video Saved",
                (false, false) => "Unable to save image",
                (false, true) => "Unable to save video",
            });
        });
    });
}

pub(super) fn copy_link(app: &Rc<App>, url: &str) {
    app.copy(url);
    app.toast("Link Copied!");
}

pub(super) fn open_link(app: &Rc<App>, url: &str) {
    gtk::UriLauncher::new(url).launch(Some(&app.window), gio::Cancellable::NONE, |_| {});
}

impl App {
    /// Whether the media options (saving, copying, opening links) are hidden
    /// in `channel`: a post of a forum that says so (`s` in 207133).
    pub(super) fn media_options_hidden(&self, channel: discord::Snowflake) -> bool {
        let state = self.account.state.borrow();
        let Some((Some(guild), channel)) = state.find_channel(channel) else { return false };
        channel.kind.is_thread()
            && channel
                .parent_id
                .and_then(|parent| guild.find(parent))
                .is_some_and(|parent| parent.kind.is_forum_like() && parent.hides_media_options())
    }
}

/// What a message's media is opened through: the media of the message, and
/// where the viewer is. A forwarded message has none.
#[derive(Clone)]
pub(super) struct MediaCx {
    app: Weak<App>,
    items: Rc<Vec<Item>>,
    origin: Option<Origin>,
    hide_options: bool,
}

/// What a media widget is named before its place among the message's items.
const MEDIA_NAME: &str = "media-";

impl MediaCx {
    pub(super) fn new(app: &Rc<App>, message: &discord::Message, items: Vec<Item>) -> MediaCx {
        let time = super::chat::parse_time(&message.timestamp).map(|t| super::chat::format_time(&t));
        MediaCx {
            app: Rc::downgrade(app),
            items: Rc::new(items),
            origin: Some(Origin { author: message.author.display_name().to_owned(), time: time.unwrap_or_default() }),
            hide_options: app.media_options_hidden(message.channel_id),
        }
    }

    /// For media that opens nothing.
    pub(super) fn none() -> MediaCx {
        MediaCx { app: Weak::new(), items: Rc::default(), origin: None, hide_options: false }
    }

    /// The place of an attachment among the items.
    pub(super) fn attachment(&self, id: discord::Snowflake) -> Option<usize> {
        self.items.iter().position(|i| i.source == Source::Attachment(id))
    }

    /// The place of the `n`th item of the embed at `embed`.
    pub(super) fn embedded(&self, embed: usize, n: usize) -> Option<usize> {
        let mut found = (0..self.items.len()).filter(|&i| self.items[i].source == Source::Embed(embed));
        found.nth(n)
    }

    /// How many open together with the item at `index`.
    pub(super) fn together(&self, index: usize) -> usize {
        media::gallery(&self.items, index).0.len()
    }

    /// Open the viewer on the item at `index`; a video starts at `start`.
    pub(super) fn open(&self, index: usize, start: f64) {
        let Some(app) = self.app.upgrade() else { return };
        if index >= self.items.len() {
            return;
        }
        let (items, at) = media::gallery(&self.items, index);
        app.viewer.open(&app, Show { items, at, origin: self.origin.clone(), hide_options: self.hide_options, start });
    }

    /// Make `widget` open the item at `index` when clicked, and name it for
    /// the message's menu.
    pub(super) fn link(&self, widget: &impl IsA<gtk::Widget>, index: Option<usize>) {
        let Some(index) = index else { return };
        widget.set_widget_name(&format!("{MEDIA_NAME}{index}"));
        widget.set_cursor_from_name(Some("pointer"));
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        let this = self.clone();
        click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            this.open(index, 0.0);
        });
        widget.add_controller(click);
    }

    /// Name `widget` for the message's menu only, for media that plays where it is.
    pub(super) fn name(&self, widget: &impl IsA<gtk::Widget>, index: Option<usize>) {
        if let Some(index) = index {
            widget.set_widget_name(&format!("{MEDIA_NAME}{index}"));
        }
    }
}

/// The place among a message's media of the widget [`MediaCx::link`] named.
pub(super) fn media_index(widget: &gtk::Widget) -> Option<usize> {
    widget.widget_name().strip_prefix(MEDIA_NAME)?.parse().ok()
}
