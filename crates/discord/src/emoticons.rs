//! Typed emoticons like `:)` that become an emoji as the message is sent,
//! when the account's "convert emoticons" setting is on (the `emoticon`
//! rule in 451909, with the table of 132565 and the pattern of 7584).

/// Each emoticon and the name of the emoji it stands for, in the order the
/// client's pattern tries them: the first that starts the text is the one,
/// even where a later one would also fit.
const TABLE: &[(&str, &str)] = &[
    (">:(", "angry"),
    (">:-(", "angry"),
    (">=(", "angry"),
    (">=-(", "angry"),
    (":\")", "blush"),
    (":-\")", "blush"),
    ("=\")", "blush"),
    ("=-\")", "blush"),
    ("</3", "broken_heart"),
    ("<\\3", "broken_heart"),
    (":-\\", "confused"),
    (":-/", "confused"),
    ("=-\\", "confused"),
    ("=-/", "confused"),
    (":'(", "cry"),
    (":'-(", "cry"),
    (":,(", "cry"),
    (":,-(", "cry"),
    ("='(", "cry"),
    ("='-(", "cry"),
    ("=,(", "cry"),
    ("=,-(", "cry"),
    (":(", "slight_frown"),
    (":-(", "slight_frown"),
    ("=(", "slight_frown"),
    ("=-(", "slight_frown"),
    ("<3", "heart"),
    ("♡", "heart"),
    ("]:(", "imp"),
    ("]:-(", "imp"),
    ("]=(", "imp"),
    ("]=-(", "imp"),
    ("o:)", "innocent"),
    ("O:)", "innocent"),
    ("o:-)", "innocent"),
    ("O:-)", "innocent"),
    ("0:)", "innocent"),
    ("0:-)", "innocent"),
    ("o=)", "innocent"),
    ("O=)", "innocent"),
    ("o=-)", "innocent"),
    ("O=-)", "innocent"),
    ("0=)", "innocent"),
    ("0=-)", "innocent"),
    (":'D", "joy"),
    (":'-D", "joy"),
    (":,D", "joy"),
    (":,-D", "joy"),
    ("='D", "joy"),
    ("='-D", "joy"),
    ("=,D", "joy"),
    ("=,-D", "joy"),
    (":*", "kissing"),
    (":-*", "kissing"),
    ("=*", "kissing"),
    ("=-*", "kissing"),
    ("x-)", "laughing"),
    ("X-)", "laughing"),
    (":|", "neutral_face"),
    (":-|", "neutral_face"),
    ("=|", "neutral_face"),
    ("=-|", "neutral_face"),
    (":o", "open_mouth"),
    (":-o", "open_mouth"),
    (":O", "open_mouth"),
    (":-O", "open_mouth"),
    ("=o", "open_mouth"),
    ("=-o", "open_mouth"),
    ("=O", "open_mouth"),
    ("=-O", "open_mouth"),
    (":@", "rage"),
    (":-@", "rage"),
    ("=@", "rage"),
    ("=-@", "rage"),
    (":D", "smile"),
    (":-D", "smile"),
    ("=D", "smile"),
    ("=-D", "smile"),
    (":')", "smiling_face_with_tear"),
    (":'-)", "smiling_face_with_tear"),
    (":,)", "smiling_face_with_tear"),
    (":,-)", "smiling_face_with_tear"),
    ("=')", "smiling_face_with_tear"),
    ("='-)", "smiling_face_with_tear"),
    ("=,)", "smiling_face_with_tear"),
    ("=,-)", "smiling_face_with_tear"),
    (":)", "slight_smile"),
    (":-)", "slight_smile"),
    ("=)", "slight_smile"),
    ("=-)", "slight_smile"),
    ("]:)", "smiling_imp"),
    ("]:-)", "smiling_imp"),
    ("]=)", "smiling_imp"),
    ("]=-)", "smiling_imp"),
    (":,'(", "sob"),
    (":,'-(", "sob"),
    (";(", "sob"),
    (";-(", "sob"),
    ("=,'(", "sob"),
    ("=,'-(", "sob"),
    (":P", "stuck_out_tongue"),
    (":-P", "stuck_out_tongue"),
    ("=P", "stuck_out_tongue"),
    ("=-P", "stuck_out_tongue"),
    ("8-)", "sunglasses"),
    ("B-)", "sunglasses"),
    (",:(", "sweat"),
    (",:-(", "sweat"),
    (",=(", "sweat"),
    (",=-(", "sweat"),
    (",:)", "sweat_smile"),
    (",:-)", "sweat_smile"),
    (",=)", "sweat_smile"),
    (",=-)", "sweat_smile"),
    (":s", "unamused"),
    (":-S", "unamused"),
    (":z", "unamused"),
    (":-Z", "unamused"),
    (":$", "unamused"),
    (":-$", "unamused"),
    ("=s", "unamused"),
    ("=-S", "unamused"),
    ("=z", "unamused"),
    ("=-Z", "unamused"),
    ("=$", "unamused"),
    ("=-$", "unamused"),
    (";)", "wink"),
    (";-)", "wink"),
];

/// The emoji name of the emoticon that starts `rest`, and its length in
/// characters. It counts only at the start of the text or after
/// whitespace (`after_space`), and only before the end of the text, a space
/// or a newline; the client takes the first emoticon that fits and doesn't
/// look for another when that one isn't followed by one of them.
pub fn at(rest: &[char], after_space: bool) -> Option<(&'static str, usize)> {
    if !after_space {
        return None;
    }
    let (emoticon, name) = TABLE.iter().find(|(emoticon, _)| {
        rest.len() >= emoticon.chars().count() && emoticon.chars().zip(rest).all(|(a, &b)| a == b)
    })?;
    let len = emoticon.chars().count();
    matches!(rest.get(len), None | Some(' ' | '\n')).then_some((name, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_start(text: &str) -> Option<(&'static str, usize)> {
        at(&text.chars().collect::<Vec<_>>(), true)
    }

    #[test]
    fn emoticons_stand_alone_before_a_space_a_newline_or_the_end() {
        assert_eq!(at_start(":)"), Some(("slight_smile", 2)));
        assert_eq!(at_start(":-) hi"), Some(("slight_smile", 3)));
        assert_eq!(at_start("<3\nx"), Some(("heart", 2)));
        assert_eq!(at_start(";-)"), Some(("wink", 3)));
        assert_eq!(at_start(":)x"), None, "it has to end the word");
        assert_eq!(at_start(":'D"), Some(("joy", 3)), "not :' alone");
        assert_eq!(at(&[':', ')'], false), None, "and follow a space");
    }

    #[test]
    fn the_first_that_fits_decides() {
        // `:'\)` is tried before `:)`; `:\` (confused) isn't `:/`.
        assert_eq!(at_start(":')"), Some(("smiling_face_with_tear", 3)));
        assert_eq!(at_start("]:)"), Some(("smiling_imp", 3)));
        assert_eq!(at_start("O:)"), Some(("innocent", 3)));
        // `:o` is open mouth, and `:ok` is no emoticon.
        assert_eq!(at_start(":o"), Some(("open_mouth", 2)));
        assert_eq!(at_start(":ok"), None);
    }

    #[test]
    fn every_name_is_an_emoji() {
        for (_, name) in TABLE {
            assert!(crate::emoji::unicode(name).is_some(), "{name}");
        }
    }
}
