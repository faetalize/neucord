//! When animated media plays, as the official client decides it.
//!
//! The client's pictures show a still until a rule lets them move: the
//! pointer on a server icon; the pointer on an avatar, or with
//! `gif_auto_play` on its message or row; `gif_auto_play` for GIFs;
//! `animate_emoji` and `animate_stickers` for emoji and stickers (the pointer
//! on one, or on its message, moves it whatever those say). All of it waits
//! for the window to have focus. The client's hooks are `useShouldAnimate`
//! (GIFs, avatars), `useCanAnimate` (decorations), the sticker `Qn`, and the
//! emoji component with its visibility manager.

use crate::settings::{MediaSettings, StickerAnimation};

/// What is animated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A server's icon in the rail.
    ServerIcon,
    /// A user's picture. Its row (the message, the list entry) counts as the
    /// pointer being on it where the client lets that move it.
    Avatar,
    /// An attachment or embed GIF.
    Gif,
    /// A custom emoji in text. Its message counts as the pointer on it.
    Emoji,
    /// A sticker. Its message counts as the pointer on it.
    Sticker,
}

/// Where the pointer is, and what the window is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The window has focus.
    pub focused: bool,
    /// Some of it is in the part of its scroller that shows.
    pub on_screen: bool,
    /// The pointer is on it.
    pub hovered: bool,
    /// The pointer is on its row or message.
    pub row_hovered: bool,
}

/// Whether the media should show moving rather than as its still, whatever
/// the window is doing: the part of the rule the settings and the pointer
/// decide. A still is swapped back in when this ends, so a hover-only avatar
/// doesn't stay mid-animation.
pub fn moving(subject: Subject, media: &MediaSettings, facts: Facts) -> bool {
    let Facts { hovered, row_hovered, .. } = facts;
    match subject {
        Subject::ServerIcon => hovered,
        Subject::Avatar => hovered || (row_hovered && media.gif_auto_play),
        Subject::Gif => hovered || media.gif_auto_play,
        Subject::Emoji => hovered || row_hovered || media.animate_emoji,
        Subject::Sticker => match media.animate_stickers {
            StickerAnimation::Always => true,
            StickerAnimation::OnInteraction => hovered || row_hovered,
            StickerAnimation::Never => false,
        },
    }
}

/// Whether its frames advance now. Moving media that can't be seen (a window
/// without focus, a row scrolled out of view) keeps its frame and costs no
/// timer.
pub fn plays(subject: Subject, media: &MediaSettings, facts: Facts) -> bool {
    facts.focused && facts.on_screen && moving(subject, media, facts)
}

/// How many animated emoji the client lets play at once (`animatingComponents`
/// in its visibility manager).
pub const EMOJI_LIMIT: u32 = 100;

/// Who of the emoji on screen may play, `limit` at a time, in the order they
/// ask. The client prefers the ones nearest the bottom of the list when it
/// has to choose; here the ones that asked first keep playing, and a freed
/// place goes to the longest waiting.
#[derive(Debug)]
pub struct Budget {
    limit: u32,
    /// Who holds a place, and how many it takes.
    playing: Vec<(u64, u32)>,
    waiting: Vec<(u64, u32)>,
}

impl Budget {
    pub fn new(limit: u32) -> Self {
        Budget { limit, playing: Vec::new(), waiting: Vec::new() }
    }

    fn used(&self) -> u32 {
        self.playing.iter().map(|(_, n)| n).sum()
    }

    /// `who` wants to play `count` emoji. True if it may; otherwise it waits
    /// for [`release`](Self::release) to hand it a place. Asking again keeps
    /// its answer.
    pub fn ask(&mut self, who: u64, count: u32) -> bool {
        if self.playing.iter().any(|(id, _)| *id == who) {
            return true;
        }
        // Something that is alone always plays, even if it is over the limit.
        if self.used() + count <= self.limit || self.playing.is_empty() {
            self.waiting.retain(|(id, _)| *id != who);
            self.playing.push((who, count));
            return true;
        }
        if !self.waiting.iter().any(|(id, _)| *id == who) {
            self.waiting.push((who, count));
        }
        false
    }

    /// `who` stops (or no longer wants to play). Returns whoever was waiting
    /// and now has a place.
    pub fn release(&mut self, who: u64) -> Vec<u64> {
        self.waiting.retain(|(id, _)| *id != who);
        let before = self.playing.len();
        self.playing.retain(|(id, _)| *id != who);
        if self.playing.len() == before {
            return Vec::new();
        }
        let mut granted = Vec::new();
        let mut waiting = std::mem::take(&mut self.waiting);
        waiting.retain(|&(id, count)| {
            if self.used() + count <= self.limit || self.playing.is_empty() {
                self.playing.push((id, count));
                granted.push(id);
                false
            } else {
                true
            }
        });
        self.waiting = waiting;
        granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Facts = Facts { focused: true, on_screen: true, hovered: false, row_hovered: false };
    const HOVER: Facts = Facts { hovered: true, ..ON };
    const ROW: Facts = Facts { row_hovered: true, ..ON };

    fn media() -> MediaSettings {
        MediaSettings::default()
    }

    #[test]
    fn server_icons_move_only_under_the_pointer() {
        assert!(!plays(Subject::ServerIcon, &media(), ON));
        assert!(plays(Subject::ServerIcon, &media(), HOVER));
        assert!(!plays(Subject::ServerIcon, &media(), ROW), "no row to count");
    }

    #[test]
    fn avatars_move_for_their_row_only_with_gif_auto_play() {
        assert!(!plays(Subject::Avatar, &media(), ON), "a still at rest, whatever the settings");
        assert!(plays(Subject::Avatar, &media(), HOVER));
        assert!(plays(Subject::Avatar, &media(), ROW));
        let off = MediaSettings { gif_auto_play: false, ..media() };
        assert!(!plays(Subject::Avatar, &off, ROW), "gif_auto_play off: the avatar itself");
        assert!(plays(Subject::Avatar, &off, HOVER));
    }

    #[test]
    fn gifs_play_by_the_setting_or_the_pointer() {
        assert!(plays(Subject::Gif, &media(), ON));
        let off = MediaSettings { gif_auto_play: false, ..media() };
        assert!(!plays(Subject::Gif, &off, ON));
        assert!(plays(Subject::Gif, &off, HOVER));
        assert!(!plays(Subject::Gif, &off, ROW), "only the GIF itself");
    }

    #[test]
    fn emoji_play_by_the_setting_or_the_message_under_the_pointer() {
        assert!(plays(Subject::Emoji, &media(), ON));
        let off = MediaSettings { animate_emoji: false, ..media() };
        assert!(!plays(Subject::Emoji, &off, ON));
        assert!(plays(Subject::Emoji, &off, ROW));
        assert!(plays(Subject::Emoji, &off, HOVER));
        let gifs_off = MediaSettings { gif_auto_play: false, ..media() };
        assert!(plays(Subject::Emoji, &gifs_off, ON), "gif_auto_play is not theirs");
    }

    #[test]
    fn stickers_follow_animate_stickers_alone() {
        let with = |animate_stickers| MediaSettings { animate_stickers, gif_auto_play: false, ..media() };
        assert!(plays(Subject::Sticker, &with(StickerAnimation::Always), ON));
        assert!(!plays(Subject::Sticker, &with(StickerAnimation::OnInteraction), ON));
        assert!(plays(Subject::Sticker, &with(StickerAnimation::OnInteraction), ROW));
        assert!(!plays(Subject::Sticker, &with(StickerAnimation::Never), HOVER), "never, even under the pointer");
    }

    #[test]
    fn nothing_plays_without_focus_or_unseen_but_the_still_rule_stays() {
        let unfocused = Facts { focused: false, ..HOVER };
        let unseen = Facts { on_screen: false, ..HOVER };
        for subject in [Subject::ServerIcon, Subject::Avatar, Subject::Gif, Subject::Emoji, Subject::Sticker] {
            assert!(!plays(subject, &media(), unfocused), "{subject:?} waits for focus");
            assert!(!plays(subject, &media(), unseen), "{subject:?} waits to be seen");
        }
        assert!(moving(Subject::Gif, &media(), unfocused), "the picture stays the moving one, paused");
    }

    #[test]
    fn reduced_motion_leaves_the_pointer() {
        let reduced = media().reduced_motion();
        assert!(!plays(Subject::Gif, &reduced, ON));
        assert!(plays(Subject::Gif, &reduced, HOVER));
        assert!(!plays(Subject::Emoji, &reduced, ON));
        assert!(plays(Subject::Sticker, &reduced, ROW));
        assert!(!plays(Subject::Sticker, &reduced, ON));
    }

    #[test]
    fn the_budget_hands_places_out_in_order() {
        let mut budget = Budget::new(10);
        assert!(budget.ask(1, 6));
        assert!(budget.ask(2, 4));
        assert!(!budget.ask(3, 1), "full");
        assert!(!budget.ask(4, 5), "full");
        assert!(budget.ask(1, 6), "asking again keeps its place");
        assert_eq!(budget.release(2), vec![3], "3 asked first and fits; 4 needs 5");
        assert_eq!(budget.release(1), vec![4]);
        assert!(budget.release(99).is_empty(), "never held a place");
    }

    #[test]
    fn something_alone_plays_even_over_the_limit() {
        let mut budget = Budget::new(10);
        assert!(budget.ask(1, 40));
        assert!(!budget.ask(2, 1));
        assert_eq!(budget.release(1), vec![2]);
    }

    #[test]
    fn a_waiting_one_that_gives_up_is_forgotten() {
        let mut budget = Budget::new(1);
        assert!(budget.ask(1, 1));
        assert!(!budget.ask(2, 1));
        assert!(budget.release(2).is_empty());
        assert!(budget.release(1).is_empty(), "nobody left to hand it to");
    }
}
