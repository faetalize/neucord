//! How much message history to ask for, and when: the official client's
//! rules. Fetching harder than it does looks like scraping, which is how an
//! account gets flagged (#88).

use std::time::{Duration, Instant};

use crate::Snowflake;

/// Messages fetched around a jump target (`d7Q`).
pub const AROUND_PAGE: u8 = 30;
/// The most messages a page of scrollback asks for (`EMb`). Discord would
/// allow 100, but the client only asks for those in one-off loads.
const MAX_SCROLL_PAGE: u8 = 30;
/// The fewest messages a channel opens with, and the most (`EMb` again).
const MIN_FIRST_PAGE: f64 = 10.0;
/// Window height outside the message list, and the least left for it.
const WINDOW_CHROME: f64 = 145.0;
const MIN_LIST_HEIGHT: f64 = 400.0;
/// How tall the client takes a message to be at its default font size.
const COZY_ROW: f64 = 91.0;
const COMPACT_ROW: f64 = 65.0;
/// `FONT_SIZES`: a font size outside these would be a client that can't exist.
const FONT_SIZES: [f64; 7] = [12.0, 14.0, 15.0, 16.0, 18.0, 20.0, 24.0];
const DEFAULT_FONT_SIZE: f64 = 16.0;
/// How close to an end of the loaded messages the next page is fetched
/// (`getOffsetToTriggerLoading`).
const LOAD_DISTANCE: f64 = 500.0;
/// A page that grows the list by less than this pauses loading: more of the
/// same could go on for a long stretch without moving the view.
const MIN_PAGE_HEIGHT: f64 = 100.0;
/// How long after a wheel or key a move still counts as the user's.
pub const SCROLL_INPUT: Duration = Duration::from_millis(400);
/// The same after a fling, which keeps moving the list with no more input.
pub const KINETIC_INPUT: Duration = Duration::from_secs(3);

/// What a page's size depends on.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// The window's height in logical pixels. That's what `innerHeight` is
    /// in the client, so a scaled display asks for what its logical size
    /// would, not its physical one.
    pub height: i32,
    /// The system's text scale: bigger text fills the window sooner, as the
    /// client's font size setting does.
    pub text_scale: f64,
    /// The account's compact message display.
    pub compact: bool,
}

impl Viewport {
    /// Messages to fetch when a channel opens: a window's worth (`o` and
    /// `d` in 326337).
    pub fn first_page(&self) -> u8 {
        let list = (f64::from(self.height) - WINDOW_CHROME).max(MIN_LIST_HEIGHT);
        let row = if self.compact { COMPACT_ROW } else { COZY_ROW } * (self.font_size() / DEFAULT_FONT_SIZE);
        let rows = (1.05 * (list / row).ceil()).ceil();
        rows.clamp(MIN_FIRST_PAGE, f64::from(MAX_SCROLL_PAGE)) as u8
    }

    /// Messages to fetch per page of scrollback, either way (`loadMore` in
    /// 425812).
    pub fn scroll_page(&self) -> u8 {
        MAX_SCROLL_PAGE.min(2 * self.first_page())
    }

    /// The client's font size closest to the system's text scale.
    fn font_size(&self) -> f64 {
        let wanted = DEFAULT_FONT_SIZE * self.text_scale;
        FONT_SIZES.into_iter().fold(DEFAULT_FONT_SIZE, |closest, size| {
            if (size - wanted).abs() < (closest - wanted).abs() { size } else { closest }
        })
    }
}

/// Where the view is in the loaded messages, in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Position {
    /// Above the top of the view.
    pub above: f64,
    /// Below the bottom of the view.
    pub below: f64,
}

/// What can be loaded in a channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Loadable {
    /// There are older messages to fetch.
    pub older: bool,
    /// There are newer messages to fetch, after a jump.
    pub newer: bool,
    /// A request for this channel is in flight.
    pub loading: bool,
    /// The channel has unread messages.
    pub unread: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    Older,
    Newer,
}

/// Decides when scrolling loads another page, for the open channel: the
/// client's `isScrollLoadingDisabled` and `handleScroll` in 425812, plus that
/// the list moving by itself never counts as scrolling. The client's list
/// doesn't do that, and GTK's does (#88).
#[derive(Debug, Default)]
pub struct ScrollLoader {
    /// `loadMorePausedUntilUserScroll`.
    paused: bool,
    dragging: bool,
    /// `scrollHeightBeforeLoad`.
    height_before_load: Option<f64>,
    /// Until when moves count as the user's.
    input_until: Option<Instant>,
    /// Where the view last was.
    above: Option<f64>,
}

impl ScrollLoader {
    /// Start over for another channel.
    pub fn reset(&mut self) {
        *self = Self { dragging: self.dragging, ..Self::default() };
    }

    /// The list moved to `at`. Returns whether the user moved it. If not,
    /// and it went up into the loading zone, that's a jump nobody asked for:
    /// pausing keeps a page landing from chaining off it.
    pub fn moved(&mut self, at: Position, now: Instant) -> bool {
        let asked = self.dragging || self.input_until.is_some_and(|until| now < until);
        if !asked && self.above.is_some_and(|above| at.above < above) && at.above <= LOAD_DISTANCE {
            self.paused = true;
        }
        self.above = Some(at.above);
        asked
    }

    /// The user turned the wheel, swiped or pressed a scrolling key.
    pub fn input(&mut self, now: Instant, lasts: Duration) {
        self.paused = false;
        self.input_until = Some(now + lasts);
    }

    pub fn press(&mut self) {
        self.dragging = true;
    }

    pub fn release(&mut self, now: Instant) {
        if self.dragging {
            self.input(now, SCROLL_INPUT);
        }
        self.dragging = false;
    }

    /// We put the view at the newest messages ourselves.
    pub fn arrived(&mut self) {
        self.paused = false;
    }

    /// A page was requested, with the list `height` tall.
    pub fn load_started(&mut self, height: f64) {
        self.height_before_load = Some(height);
    }

    /// The page landed and the list is `height` tall.
    pub fn load_finished(&mut self, height: f64) {
        if let Some(before) = self.height_before_load.take() {
            self.paused |= height - before < MIN_PAGE_HEIGHT;
        }
    }

    /// The page to load now. `asked` is whether the user just scrolled, as
    /// opposed to a check after a page landed.
    pub fn next(&self, at: Position, can: Loadable, asked: bool) -> Option<Load> {
        if self.paused || self.dragging || can.loading {
            return None;
        }
        // With unread messages the client opens on the first of them, and
        // only loads once the user scrolls past the top. Neucord has no room
        // past the top, so that's being at the top and scrolling.
        let top = if can.unread { 0.0 } else { LOAD_DISTANCE };
        if can.older && at.above <= top && (asked || !can.unread) {
            return Some(Load::Older);
        }
        (can.newer && at.below <= LOAD_DISTANCE).then_some(Load::Newer)
    }
}

/// What a page around a jump target says about what lies past it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Around {
    pub more_before: bool,
    pub more_after: bool,
}

impl Around {
    /// `page` is oldest first; `newest` is the channel's last message. The
    /// sums are `fetchMessages`' in 148494: the target counts with what's
    /// after it, if it still exists.
    pub fn of(page: &[Snowflake], target: Snowflake, newest: Option<Snowflake>) -> Self {
        let half = usize::from(AROUND_PAGE) / 2;
        let before = page.iter().filter(|&&id| id < target).count();
        let at_newest = page.last().is_some_and(|&last| Some(last) == newest);
        Self { more_before: before >= half - 1, more_after: page.len() - before >= half && !at_newest }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport(height: i32) -> Viewport {
        Viewport { height, text_scale: 1.0, compact: false }
    }

    #[test]
    fn first_page_follows_the_window_like_the_client() {
        // `o` and `d` in 326337: (h - 145) over 91px rows, 5% extra, 10 to 30.
        assert_eq!(viewport(1080).first_page(), 12);
        assert_eq!(viewport(1440).first_page(), 16);
        assert_eq!(viewport(2160).first_page(), 25);
        assert_eq!(viewport(800).first_page(), 10, "the floor, up to 964px");
        assert_eq!(viewport(0).first_page(), 10, "not allocated yet: the client's 400px minimum");
        assert_eq!(viewport(4000).first_page(), 30, "the cap");
    }

    #[test]
    fn compact_rows_are_shorter() {
        assert_eq!(Viewport { compact: true, ..viewport(1080) }.first_page(), 16);
    }

    #[test]
    fn text_scale_lands_on_a_font_size_the_client_offers() {
        let at = |text_scale| Viewport { text_scale, ..viewport(1440) }.first_page();
        assert_eq!(at(1.0), 16, "16px, the default");
        assert_eq!(at(1.25), 13, "20px");
        assert_eq!(at(1.1), 14, "17.6px rounds to 18px");
        assert_eq!(at(3.0), 11, "24px is the largest");
        assert_eq!(at(0.5), 20, "12px is the smallest");
    }

    #[test]
    fn scroll_page_is_twice_the_first_up_to_the_cap() {
        assert_eq!(viewport(1080).scroll_page(), 24);
        assert_eq!(viewport(800).scroll_page(), 20);
        assert_eq!(viewport(2160).scroll_page(), 30, "2 * 25, capped at `EMb`");
    }

    fn at(above: f64, below: f64) -> Position {
        Position { above, below }
    }

    fn older() -> Loadable {
        Loadable { older: true, ..Loadable::default() }
    }

    fn newer() -> Loadable {
        Loadable { newer: true, ..Loadable::default() }
    }

    #[test]
    fn loads_within_500px_of_the_ends() {
        let loader = ScrollLoader::default();
        assert_eq!(loader.next(at(500.0, 9000.0), older(), true), Some(Load::Older));
        assert_eq!(loader.next(at(501.0, 9000.0), older(), true), None, "was three screens");
        assert_eq!(loader.next(at(9000.0, 500.0), newer(), true), Some(Load::Newer));
        assert_eq!(loader.next(at(9000.0, 501.0), newer(), true), None, "was three screens");
    }

    #[test]
    fn one_request_at_a_time_older_first() {
        let loader = ScrollLoader::default();
        let both = Loadable { older: true, newer: true, ..Loadable::default() };
        assert_eq!(loader.next(at(0.0, 0.0), both, true), Some(Load::Older));
        assert_eq!(loader.next(at(0.0, 0.0), Loadable { loading: true, ..both }, true), None);
    }

    #[test]
    fn nothing_to_load_loads_nothing() {
        assert_eq!(ScrollLoader::default().next(at(0.0, 0.0), Loadable::default(), true), None);
    }

    #[test]
    fn unread_messages_hold_loading_back_to_the_very_top() {
        // `getOffsetToTriggerLoading`: the client opens on the first unread
        // and doesn't load from there until the user scrolls past the top.
        let loader = ScrollLoader::default();
        let unread = Loadable { unread: true, ..older() };
        assert_eq!(loader.next(at(1.0, 9000.0), unread, true), None);
        assert_eq!(loader.next(at(0.0, 9000.0), unread, true), Some(Load::Older));
        assert_eq!(loader.next(at(0.0, 0.0), unread, false), None, "not without the user");
    }

    #[test]
    fn a_page_that_adds_under_100px_pauses_until_the_user_scrolls() {
        let mut loader = ScrollLoader::default();
        loader.load_started(1000.0);
        loader.load_finished(1099.0);
        assert_eq!(loader.next(at(0.0, 9000.0), older(), false), None, "the chain stops");
        assert_eq!(loader.next(at(0.0, 9000.0), older(), true), None, "so does a move on its own");
        loader.input(Instant::now(), SCROLL_INPUT);
        assert_eq!(loader.next(at(0.0, 9000.0), older(), true), Some(Load::Older));
    }

    #[test]
    fn a_page_of_100px_keeps_going() {
        let mut loader = ScrollLoader::default();
        loader.load_started(1000.0);
        loader.load_finished(1100.0);
        assert_eq!(loader.next(at(0.0, 9000.0), older(), false), Some(Load::Older));
    }

    #[test]
    fn a_new_channel_forgets_the_pause() {
        let mut loader = ScrollLoader::default();
        loader.load_started(1000.0);
        loader.load_finished(1000.0);
        loader.reset();
        assert_eq!(loader.next(at(0.0, 9000.0), older(), false), Some(Load::Older));
    }

    #[test]
    fn dragging_the_scrollbar_loads_nothing_until_release() {
        let mut loader = ScrollLoader::default();
        loader.load_started(1000.0);
        loader.load_finished(1000.0);
        loader.press();
        assert!(loader.moved(at(0.0, 9000.0), Instant::now()), "a drag is the user's");
        assert_eq!(loader.next(at(0.0, 9000.0), older(), true), None);
        loader.release(Instant::now());
        assert_eq!(loader.next(at(0.0, 9000.0), older(), true), Some(Load::Older), "and lifts a pause");
    }

    #[test]
    fn a_jump_nobody_asked_for_loads_nothing() {
        let mut loader = ScrollLoader::default();
        let later = Instant::now();
        assert!(!loader.moved(at(4000.0, 4000.0), later));
        assert!(!loader.moved(at(0.0, 8000.0), later), "the list moved by itself");
        assert_eq!(loader.next(at(0.0, 8000.0), older(), false), None, "and a page landing can't chain off it");
        loader.input(later, SCROLL_INPUT);
        assert!(loader.moved(at(0.0, 8000.0), later + Duration::from_millis(50)));
        assert_eq!(loader.next(at(0.0, 8000.0), older(), true), Some(Load::Older), "the user can still go on");
    }

    #[test]
    fn a_page_pushing_the_view_down_is_no_jump() {
        let mut loader = ScrollLoader::default();
        let later = Instant::now();
        loader.moved(at(100.0, 8000.0), later);
        assert!(!loader.moved(at(300.0, 8000.0), later));
        assert_eq!(loader.next(at(300.0, 8000.0), older(), false), Some(Load::Older));
    }

    #[test]
    fn input_wears_off() {
        let mut loader = ScrollLoader::default();
        let start = Instant::now();
        loader.input(start, SCROLL_INPUT);
        assert!(loader.moved(at(4000.0, 0.0), start + SCROLL_INPUT - Duration::from_millis(1)));
        assert!(!loader.moved(at(4000.0, 0.0), start + SCROLL_INPUT));
        loader.input(start, KINETIC_INPUT);
        assert!(loader.moved(at(4000.0, 0.0), start + SCROLL_INPUT), "a fling coasts for longer");
    }

    #[test]
    fn arriving_at_the_newest_messages_clears_a_jump() {
        // Switching channels passes through the top on the way to the end.
        let mut loader = ScrollLoader::default();
        let later = Instant::now();
        loader.moved(at(4000.0, 0.0), later);
        loader.moved(at(0.0, 8000.0), later);
        loader.arrived();
        assert_eq!(loader.next(at(200.0, 0.0), older(), false), Some(Load::Older), "a tall window still fills");
    }

    fn ids(ids: std::ops::RangeInclusive<u64>) -> Vec<Snowflake> {
        ids.map(Snowflake).collect()
    }

    #[test]
    fn a_page_around_a_message_shows_where_the_channel_ends() {
        // `fetchMessages` in 148494, with a limit of 30.
        let target = Snowflake(100);
        let full = Around::of(&ids(85..=114), target, Some(Snowflake(500)));
        assert_eq!(full, Around { more_before: true, more_after: true });
        let start = Around::of(&ids(90..=119), target, Some(Snowflake(500)));
        assert!(!start.more_before, "only 10 before it: that's the start of the channel");
        assert!(Around::of(&ids(86..=115), target, None).more_before, "14 before it can still be a full side");
        assert!(!Around::of(&ids(87..=116), target, None).more_before, "13 can't");
        let end = Around::of(&ids(80..=110), target, Some(Snowflake(500)));
        assert!(!end.more_after, "only 10 after it: that's the present");
        assert!(Around::of(&ids(80..=114), target, None).more_after, "14 after it and itself");
        let newest = Around::of(&ids(85..=114), target, Some(Snowflake(114)));
        assert!(!newest.more_after, "the page holds the channel's last message");
    }

    #[test]
    fn a_deleted_target_counts_the_same_without_itself() {
        let page: Vec<_> = ids(85..=115).into_iter().filter(|&id| id != Snowflake(100)).collect();
        let around = Around::of(&page, Snowflake(100), None);
        assert_eq!(around, Around { more_before: true, more_after: true });
    }
}
