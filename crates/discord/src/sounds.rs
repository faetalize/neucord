//! How the client's Notifications settings show one sound of its list
//! (`a1` in 212909): a switch for the sound, off and not to be touched while
//! "Disable All Notification Sounds" is on (the sound's own choice is kept
//! under it) or, for the new message sound, while Focus Mode is on (`jy`,
//! with `jC.kB` as its `useDisabled`).

/// What one sound's switch shows and allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// Whether the switch reads on: the sound would play.
    pub on: bool,
    /// Whether the switch can be changed.
    pub enabled: bool,
}

/// `all_sounds` is Neucord's master switch (on unless the client's
/// `disableAllSounds`), `own` the sound's choice, and `held_back` whether
/// something else disables this sound's row, as Focus Mode does the new
/// message one.
pub fn row(all_sounds: bool, own: bool, held_back: bool) -> Row {
    Row { on: all_sounds && own && !held_back, enabled: all_sounds && !held_back }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_follows_its_own_choice() {
        assert_eq!(row(true, true, false), Row { on: true, enabled: true });
        assert_eq!(row(true, false, false), Row { on: false, enabled: true });
    }

    #[test]
    fn all_sounds_off_shows_every_sound_off_and_fixed() {
        for own in [true, false] {
            assert_eq!(row(false, own, false), Row { on: false, enabled: false });
        }
    }

    #[test]
    fn a_held_back_row_is_off_and_fixed_whatever_the_choice() {
        for own in [true, false] {
            assert_eq!(row(true, own, true), Row { on: false, enabled: false });
        }
    }
}
