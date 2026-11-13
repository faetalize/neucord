//! `/gif` and `/sticker`, the two built-ins that search
//! (`BUILT_IN_INTEGRATION`, 496533). While one is in the box, its results
//! show in a popup over it as the caret sits in its `query` pill, in place of
//! the pill's own lists: the GIFs in one row that scrolls sideways, the
//! stickers one to a row. The first is selected as they show (`Selection`),
//! so Enter right away picks it; Enter with none selected shakes the box.
//!
//! A GIF goes as the box's own message (Enter, a click) with whatever else
//! the box holds, or replaces the box as typed text (Tab); no
//! `POST /gifs/select`, which only the picker sends. A sticker goes as the
//! picker's does from an empty box, or waits beside the command for the
//! message when files or another sticker wait. Each pick counts as a use of
//! the command (`APPLICATION_COMMAND_USED`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::Snowflake;
use discord::animation::Subject;
use discord::builtins::Integration;
use discord::command_line;
use discord::commands::{Focus, Pick};
use discord::gifs::{COMMAND_DEBOUNCE_MS, CommandShown, Gif};
use discord::stickers::Hit;
use gtk::glib;

use super::App;
use super::slash::Keep;

/// What a GIF is shown at: this high, as wide as its ratio makes it.
const GIF_HEIGHT: i32 = 100;
/// A sticker's row (`eL` in 927659).
const STICKER: i32 = 40;

/// What the popup holds now.
#[derive(Default)]
enum Shown {
    #[default]
    Nothing,
    Gifs(Vec<Gif>),
    Stickers(Vec<Hit>),
}

impl Shown {
    fn count(&self) -> usize {
        match self {
            Shown::Nothing => 0,
            Shown::Gifs(gifs) => gifs.len(),
            Shown::Stickers(hits) => hits.len(),
        }
    }
}

/// The popup of `/gif`'s and `/sticker`'s results.
pub(super) struct View {
    popover: gtk::Popover,
    title: gtk::Label,
    spinner: adw::Spinner,
    gif_scroller: gtk::ScrolledWindow,
    gifs: gtk::FlowBox,
    sticker_scroller: gtk::ScrolledWindow,
    stickers: gtk::ListBox,
    shown: RefCell<Shown>,
    /// The trimmed `/gif` query last seen: the search goes only when it
    /// changes (`T` in 31498's `queryResults`).
    asked: RefCell<Option<String>>,
}

impl View {
    pub(super) fn new() -> Self {
        let gifs = gtk::FlowBox::builder()
            // One child to a column: a single row.
            .orientation(gtk::Orientation::Vertical)
            .max_children_per_line(1)
            .min_children_per_line(1)
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .column_spacing(4)
            .build();
        let gif_scroller = gtk::ScrolledWindow::builder()
            .child(&gifs)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .build();
        let stickers = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let sticker_scroller = gtk::ScrolledWindow::builder()
            .child(&stickers)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(400)
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(6)
            .margin_top(6)
            .margin_bottom(2)
            .css_classes(["caption-heading", "dim-label"])
            .build();
        let spinner =
            adw::Spinner::builder().width_request(32).height_request(32).margin_top(8).margin_bottom(8).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&title);
        content.append(&spinner);
        content.append(&gif_scroller);
        content.append(&sticker_scroller);
        let popover = gtk::Popover::builder()
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Top)
            .child(&content)
            .build();
        View {
            popover,
            title,
            spinner,
            gif_scroller,
            gifs,
            sticker_scroller,
            stickers,
            shown: RefCell::default(),
            asked: RefCell::default(),
        }
    }
}

impl App {
    pub(super) fn build_integrations(self: &Rc<Self>) {
        let view = &self.integration;
        view.popover.set_parent(&self.composer);
        view.gifs.connect_child_activated({
            let this = Rc::downgrade(self);
            move |_, child| {
                if let Some(this) = this.upgrade() {
                    this.pick_integration(child.index().max(0) as usize, true);
                }
            }
        });
        view.stickers.connect_row_activated({
            let this = Rc::downgrade(self);
            move |_, row| {
                if let Some(this) = this.upgrade() {
                    this.pick_integration(row.index().max(0) as usize, true);
                }
            }
        });
        // A manually-parented popover must be unparented before teardown.
        let popover = view.popover.clone();
        self.window.connect_close_request(move |_| {
            popover.unparent();
            glib::Propagation::Proceed
        });
    }

    /// The active command's search and the text of its `query` option
    /// (`optionText`): empty while the caret is outside it (133746).
    fn integration_text(&self) -> Option<(Integration, Snowflake, String)> {
        let (integration, channel) = {
            let slot = self.slash.active.borrow();
            let active = slot.as_ref()?;
            (active.command.integration()?, active.channel)
        };
        let (line, text) = self.read_line()?;
        let caret = self.composer.buffer().property::<i32>("cursor-position").max(0) as usize;
        let option = command_line::active(&line, caret)
            .filter(|pill| pill.option == "query")
            .map(|pill| text.chars().skip(pill.value.start).take(pill.value.len()).collect())
            .unwrap_or_default();
        Some((integration, channel, option))
    }

    /// The popup over a command in the box, if it is `/gif` or `/sticker`:
    /// theirs is the only list they have (`tc` in 496533 stops at the first
    /// type that matches, and theirs match from the moment they are chosen).
    /// False for any other command, whose lists are the line's.
    pub(super) fn integration_popup(self: &Rc<Self>) -> bool {
        let Some((integration, channel, text)) = self.integration_text() else {
            self.hide_integration();
            self.integration.asked.replace(None);
            return false;
        };
        self.mention_popover.popdown();
        match integration {
            Integration::Gif => self.show_gifs(&text),
            Integration::Sticker => self.show_stickers(channel, &text),
        }
        true
    }

    pub(super) fn hide_integration(&self) {
        self.integration.popover.popdown();
        self.integration.shown.replace(Shown::Nothing);
    }

    // ---- /gif ----

    /// `/gif`'s query holds `text`: search it once typing stops, then show
    /// what is known of it.
    fn show_gifs(self: &Rc<Self>, text: &str) {
        let query = text.trim().to_owned();
        if self.integration.asked.borrow().as_deref() != Some(query.as_str()) {
            self.integration.asked.replace(Some(query.clone()));
            self.gif_typed(text);
        }
        let shown = {
            let state = self.account.state.borrow();
            match state.gif_commands.shown(text) {
                CommandShown::Nothing => None,
                CommandShown::Loading => Some(None),
                CommandShown::Gifs(gifs) => Some(Some(gifs.to_vec())),
            }
        };
        let view = &self.integration;
        let Some(gifs) = shown else {
            self.list_integration("GIFS", &query, false, 0);
            return self.hide_integration();
        };
        view.title.set_visible(gifs.is_some());
        view.spinner.set_visible(gifs.is_none());
        view.gif_scroller.set_visible(gifs.is_some());
        view.sticker_scroller.set_visible(false);
        while let Some(child) = view.gifs.first_child() {
            view.gifs.remove(&child);
        }
        let (loading, count) = (gifs.is_none(), gifs.as_ref().map_or(0, Vec::len));
        if let Some(gifs) = &gifs {
            view.title.set_markup(&format!("Gif content matching <b>{}</b>", glib::markup_escape_text(&query)));
            for (index, gif) in gifs.iter().enumerate() {
                let child = gtk::FlowBoxChild::builder().child(&self.gif_face(gif)).build();
                self.on_hover(&child, index);
                view.gifs.append(&child);
            }
            // As wide as the box, so the row has room to scroll.
            view.gif_scroller.set_min_content_width(self.composer.width().max(320));
        }
        self.integration.shown.replace(gifs.map_or(Shown::Nothing, Shown::Gifs));
        self.list_integration("GIFS", &query, loading, count);
        self.show_integration();
    }

    /// A result: its image at the answer's ratio, from the answer's link as
    /// given; a video Neucord can't play shows as the picker shows one.
    fn gif_face(&self, gif: &Gif) -> gtk::Widget {
        let width = if gif.height > 0 {
            (i64::from(GIF_HEIGHT) * i64::from(gif.width) / i64::from(gif.height)).clamp(40, 400) as i32
        } else {
            GIF_HEIGHT
        };
        match gif.command_preview() {
            Some(preview) => {
                let picture = gtk::Picture::builder()
                    .width_request(width)
                    .height_request(GIF_HEIGHT)
                    .content_fit(gtk::ContentFit::Cover)
                    .build();
                self.images.set_moving(&picture, &preview, &preview, Subject::Gif, crate::images::Row::None);
                picture.upcast()
            }
            None => gtk::Image::builder()
                .icon_name("media-playback-start-symbolic")
                .pixel_size(32)
                .width_request(width)
                .height_request(GIF_HEIGHT)
                .build()
                .upcast(),
        }
    }

    /// `/gif`'s query changed to `text` (`ej.search`): (re)start the one
    /// debounce for a query not yet answered; else leave a running one to
    /// send its own.
    fn gif_typed(self: &Rc<Self>, text: &str) {
        if !self.account.state.borrow_mut().gif_commands.typed(text) {
            return;
        }
        let this = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(COMMAND_DEBOUNCE_MS.into()), move || {
            if let Some(this) = this.upgrade() {
                this.account.state.borrow_mut().gif_command_wait = None;
                this.gif_search_due();
            }
        });
        if let Some(old) = self.account.state.borrow_mut().gif_command_wait.replace(source) {
            old.remove();
        }
    }

    /// The debounce ran out: `GET /gifs/search` for its query, unless that
    /// one is being asked already. Nothing cancels it, so it goes even if
    /// `/gif` has left the box, as `eb` does.
    fn gif_search_due(self: &Rc<Self>) {
        let (query, rest) = {
            let mut state = self.account.state.borrow_mut();
            (state.gif_commands.fire(), state.rest.clone())
        };
        let (Some(query), Some(rest)) = (query, rest) else { return };
        let this = self.clone();
        let epoch = self.account.epoch.get();
        glib::spawn_future_local(async move {
            let gifs = rest.gifs_command_search(&query).await.map_err(|e| eprintln!("gifs: {e}")).ok();
            if this.account.epoch.get() != epoch {
                return;
            }
            this.account.state.borrow_mut().gif_commands.answered(&query, gifs);
            this.integration_popup();
        });
    }

    // ---- /sticker ----

    /// `/sticker`'s query holds `text`, as typed: the stickers we may send
    /// here that it finds, at most eight (`te` in 496533). Each time, the
    /// sticker frecency loads and the standard packs are fetched if they
    /// aren't (the guards make that once), as the client's `queryResults`
    /// does from the moment `/sticker` is chosen.
    fn show_stickers(self: &Rc<Self>, channel: Snowflake, text: &str) {
        let redraw = {
            let this = Rc::downgrade(self);
            move || {
                if let Some(this) = this.upgrade() {
                    this.integration_popup();
                }
            }
        };
        self.load_frecency(redraw.clone());
        self.fetch_sticker_packs(redraw);
        let hits = self.sticker_search_library(channel).command_results(text, &self.sticker_scores());
        let view = &self.integration;
        if hits.is_empty() {
            self.list_integration("STICKERS", text, false, 0);
            return self.hide_integration();
        }
        view.title.set_text(&format!("Stickers matching {text}"));
        view.title.set_visible(true);
        view.spinner.set_visible(false);
        view.gif_scroller.set_visible(false);
        view.sticker_scroller.set_visible(true);
        while let Some(child) = view.stickers.first_child() {
            view.stickers.remove(&child);
        }
        for (index, hit) in hits.iter().enumerate() {
            let row = gtk::ListBoxRow::builder().child(&self.sticker_row(hit)).build();
            self.on_hover(&row, index);
            view.stickers.append(&row);
        }
        let count = hits.len();
        self.integration.shown.replace(Shown::Stickers(hits));
        self.list_integration("STICKERS", text, false, count);
        self.show_integration();
    }

    /// A sticker's row: its image, name, what it matched when not its name,
    /// and its server or pack at the end.
    fn sticker_row(&self, hit: &Hit) -> gtk::Widget {
        let sticker = &hit.entry.listed.sticker;
        let line = gtk::Box::builder().spacing(8).build();
        let picture = gtk::Picture::builder()
            .width_request(STICKER)
            .height_request(STICKER)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        let size = crate::images::request_size((STICKER, STICKER), self.images.scale(), None).0;
        // A Lottie sticker, which Neucord can't draw yet (#46), keeps the space.
        if let Some(url) = sticker.still(size) {
            self.images.set(&picture, &url);
        }
        line.append(&picture);
        line.append(&gtk::Label::builder().label(&sticker.name).xalign(0.0).build());
        if let Some(similar) = hit.similar_to() {
            let label = format!("Similar to: {similar}");
            line.append(&gtk::Label::builder().label(label).xalign(0.0).css_classes(["dim-label"]).build());
        }
        line.append(
            &gtk::Label::builder()
                .label(hit.source.as_deref().unwrap_or_default())
                .hexpand(true)
                .xalign(1.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label"])
                .build(),
        );
        line.upcast()
    }

    /// The server's stickers or the standard packs changed: `/sticker`'s
    /// list follows (`stores: [StickersStore]`).
    pub(super) fn stickers_changed(self: &Rc<Self>) {
        let sticker = self.slash.active.borrow().as_ref().and_then(|a| a.command.integration());
        if sticker == Some(Integration::Sticker) {
            self.integration_popup();
        }
    }

    // ---- Both ----

    /// The results were drawn: the selection follows (the first as they
    /// show, none while loading).
    fn list_integration(&self, list: &str, query: &str, loading: bool, count: usize) {
        if let Some(active) = self.slash.active.borrow_mut().as_mut() {
            active.selection.listed(list, Focus::Auto, query, loading, count);
        }
        self.show_integration_selection();
    }

    fn show_integration(&self) {
        if let Some(rect) = self.caret_rect() {
            self.integration.popover.set_pointing_to(Some(&rect));
        }
        self.integration.popover.popup();
    }

    fn show_integration_selection(&self) {
        let selected = self.slash.active.borrow().as_ref().and_then(|a| a.selection.selected());
        let view = &self.integration;
        match &*view.shown.borrow() {
            Shown::Gifs(_) => match selected.and_then(|i| view.gifs.child_at_index(i as i32)) {
                Some(child) => {
                    view.gifs.select_child(&child);
                    // The row scrolls, so the selection must stay in view.
                    if let Some(bounds) = child.compute_bounds(&view.gifs) {
                        let adjustment = view.gif_scroller.hadjustment();
                        let (start, end) = (f64::from(bounds.x()), f64::from(bounds.x() + bounds.width()));
                        if start < adjustment.value() {
                            adjustment.set_value(start);
                        } else if end > adjustment.value() + adjustment.page_size() {
                            adjustment.set_value(end - adjustment.page_size());
                        }
                    }
                }
                None => view.gifs.unselect_all(),
            },
            Shown::Stickers(_) => match selected.and_then(|i| view.stickers.row_at_index(i as i32)) {
                Some(row) => view.stickers.select_row(Some(&row)),
                None => view.stickers.unselect_all(),
            },
            Shown::Nothing => {}
        }
    }

    /// The pointer onto a result that isn't selected leaves none selected
    /// (`onResultHover`, 31498): Enter then shakes until a key or new
    /// results select one again.
    fn on_hover(self: &Rc<Self>, widget: &impl IsA<gtk::Widget>, index: usize) {
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let this = Rc::downgrade(self);
            move |_, _, _| {
                let Some(this) = this.upgrade() else { return };
                let cleared = match this.slash.active.borrow_mut().as_mut() {
                    Some(active) if active.selection.selected() != Some(index) => {
                        active.selection.hovered();
                        true
                    }
                    _ => false,
                };
                if cleared {
                    this.show_integration_selection();
                }
            }
        });
        widget.add_controller(motion);
    }

    /// What a key does while the results show (31498's keys): Up and Down
    /// move the selection round, Enter sends the selected result, Tab
    /// inserts it, Escape closes the popup. `None` leaves the key to the
    /// line: Enter with nothing selected runs the command, which shakes.
    pub(super) fn integration_key(
        self: &Rc<Self>,
        key: gtk::gdk::Key,
        modifiers: gtk::gdk::ModifierType,
    ) -> Option<glib::Propagation> {
        use gtk::gdk::{Key, ModifierType};
        if !self.integration.popover.is_visible() {
            return None;
        }
        let stop = Some(glib::Propagation::Stop);
        let mods = modifiers & gtk::accelerator_get_default_mod_mask();
        let none = mods.is_empty();
        let ctrl = mods == ModifierType::CONTROL_MASK;
        let count = self.integration.shown.borrow().count();
        let moved = match key {
            Key::Escape => {
                self.integration.popover.popdown();
                return stop;
            }
            Key::Down if none => Some(1),
            Key::Up if none => Some(-1),
            Key::n if ctrl => Some(1),
            Key::p if ctrl => Some(-1),
            _ => None,
        };
        if let Some(by) = moved {
            if let Some(active) = self.slash.active.borrow_mut().as_mut() {
                active.selection.moved(by, count);
            }
            self.show_integration_selection();
            return stop;
        }
        let enter = matches!(key, Key::Return | Key::KP_Enter | Key::ISO_Enter) && none;
        if !(enter || (key == Key::Tab && none)) {
            return None;
        }
        let pick = self.slash.active.borrow_mut().as_mut().and_then(|a| a.selection.key(enter, count));
        match pick {
            Some(Pick::Send(index)) => self.pick_integration(index, true),
            Some(Pick::Insert(index)) => self.pick_integration(index, false),
            Some(Pick::Selected) | None => return None,
        }
        stop
    }

    /// Result `index` picked: sent (`SEND`: Enter, a click) or, for a GIF,
    /// put in the box in the command's place (`INSERT`: Tab). A sticker
    /// does the same either way.
    fn pick_integration(self: &Rc<Self>, index: usize, send: bool) {
        enum Picked {
            Gif(String),
            Sticker(Box<discord::stickers::Entry>),
        }
        let picked = match &*self.integration.shown.borrow() {
            Shown::Gifs(gifs) => gifs.get(index).map(|g| Picked::Gif(g.url.clone())),
            Shown::Stickers(hits) => hits.get(index).map(|h| Picked::Sticker(Box::new(h.entry.clone()))),
            Shown::Nothing => None,
        };
        let Some(picked) = picked else { return };
        let Some((channel, command)) = self.slash.active.borrow().as_ref().map(|a| (a.channel, a.command.clone()))
        else {
            return;
        };
        let guild = self.account.state.borrow().find_channel(channel).and_then(|(guild, _)| guild.map(|g| g.id));
        self.command_used(&command, guild);
        match picked {
            // The box's own send (`sendMessage(meta.url)` → `handleSendMessage`, 699344),
            // which clears it and so ends the command.
            Picked::Gif(url) if send => {
                if self.send_text(channel, url, false) {
                    self.end_command(Keep::Nothing);
                }
            }
            // `replaceText`: the box becomes the link as typed, so typing starts as for typing.
            Picked::Gif(url) => {
                self.end_command(Keep::Nothing);
                let buffer = self.composer.buffer();
                buffer.set_text(&url);
                buffer.place_cursor(&buffer.end_iter());
            }
            // `nm` in 989161's `BUILT_IN_INTEGRATION` arm, where the box's text never
            // counts: alone it goes at once and the box clears whatever the send does;
            // with files or a sticker waiting it waits instead, and the command stays.
            Picked::Sticker(entry) => {
                let alone = self.files_for_message(channel).is_empty() && self.waiting.of(channel).is_none();
                if alone {
                    self.end_command(Keep::Nothing);
                    self.pick_sticker(channel, &entry);
                } else {
                    self.set_waiting_sticker(channel, Some(entry.listed.clone()));
                }
            }
        }
        self.composer.grab_focus();
    }
}
