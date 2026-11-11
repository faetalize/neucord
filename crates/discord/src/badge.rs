//! The unread count the client puts on its icon and in its window title
//! (`Nt` in 852799, `sF` in 726249): what is left of a server's and a DM's
//! mentions once read, plus friend requests, or a dot when there are none
//! but something is unread. Linux has no dot on the icon (`setBadgeCount` in
//! the desktop app's wrapper, 253400): it shows a count or nothing.

/// The count, or [`DOT`] for no count with something unread. `hide_dot` is
/// the client's "disable unread badge" setting.
pub fn count(mentions: u32, anything_unread: bool, hide_dot: bool) -> i64 {
    match (mentions, anything_unread && !hide_dot) {
        (0, true) => DOT,
        (n, _) => i64::from(n),
    }
}

/// No count, but something unread.
pub const DOT: i64 = -1;

/// What the window's title starts with.
pub fn title_prefix(count: i64) -> String {
    match count {
        0 => String::new(),
        DOT => "\u{2022} ".to_owned(),
        n => format!("({n}) "),
    }
}

/// What the app icon shows: a dot counts as nothing.
pub fn launcher_count(count: i64) -> u32 {
    count.clamp(0, i64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_make_the_count_and_unread_alone_a_dot() {
        assert_eq!(count(3, true, false), 3);
        assert_eq!(count(0, true, false), DOT);
        assert_eq!(count(0, false, false), 0);
        assert_eq!(count(0, true, true), 0, "the dot can be turned off");
        assert_eq!(count(2, true, true), 2, "but not the count");
    }

    #[test]
    fn title_and_icon_show_what_the_client_shows() {
        assert_eq!(title_prefix(12), "(12) ");
        assert_eq!(title_prefix(DOT), "\u{2022} ");
        assert_eq!(title_prefix(0), "");
        assert_eq!(launcher_count(12), 12);
        assert_eq!(launcher_count(DOT), 0, "no dot on Linux");
    }
}
