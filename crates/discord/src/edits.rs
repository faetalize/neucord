//! What saving an edit does, as the official client decides it (205120's
//! `onSubmit`).

use crate::limits::{Limits, TooLong};

/// What saving the message box as an edit comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit<'a> {
    /// Nothing is left of a message with nothing else on it: offer to
    /// delete it.
    Delete,
    /// Too long: it stays open.
    Refuse(TooLong),
    /// The text is what the message already says.
    Unchanged,
    /// Send this as the new content.
    Save(&'a str),
}

/// `text` is the message box. An edit isn't trimmed, unlike a new message:
/// the client sends the box's value without its trailing empty lines
/// (`ignoreTrailingEmptyNodes`, 323350). `bare` is a message with no
/// attachments, components or stickers.
pub fn decide<'a>(limits: &Limits, text: &'a str, content: &str, bare: bool) -> Edit<'a> {
    let value = text.trim_end_matches('\n');
    if value.is_empty() && bare {
        return Edit::Delete;
    }
    if let Err(too_long) = limits.check_length(value) {
        return Edit::Refuse(too_long);
    }
    if value == content { Edit::Unchanged } else { Edit::Save(value) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiments::Experiments;
    use crate::limits::Nitro;
    use crate::model::Snowflake;

    fn decided<'a>(text: &'a str, content: &str, bare: bool) -> Edit<'a> {
        let (nitro, experiments) = (Nitro::default(), Experiments::default());
        let limits = Limits { nitro: &nitro, experiments: &experiments, me: Snowflake(1) };
        decide(&limits, text, content, bare)
    }

    #[test]
    fn whitespace_alone_is_sent_not_a_delete() {
        assert_eq!(decided("", "hi", true), Edit::Delete);
        assert_eq!(decided("\n\n", "hi", true), Edit::Delete, "trailing empty lines are dropped");
        assert_eq!(decided("  ", "hi", true), Edit::Save("  "), "the client's emptiness check is untrimmed");
    }

    #[test]
    fn the_box_is_sent_as_it_is() {
        assert_eq!(decided("  hi \n", "hi", true), Edit::Save("  hi "));
        assert_eq!(decided("hi", "hi", true), Edit::Unchanged);
        assert_eq!(decided("hi\n", "hi", true), Edit::Unchanged);
        assert_eq!(decided("hi ", "hi", true), Edit::Save("hi "));
        assert_eq!(decided("", "hi", false), Edit::Save(""), "a message with files may lose its text");
    }

    #[test]
    fn too_long_is_refused_before_unchanged() {
        let long = "a".repeat(2001);
        assert_eq!(decided(&long, &long, true), Edit::Refuse(TooLong { length: 2001, max: 2000 }));
        // Counted untrimmed: the spaces are the ones over.
        let padded = format!("{}  ", "a".repeat(2000));
        assert!(matches!(decided(&padded, "hi", true), Edit::Refuse(_)));
    }
}
