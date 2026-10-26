//! Render a parsed Discord message ([`discord::markup`]) into GTK widgets.
//!
//! Inline spans become Pango markup inside one wrapping `Label`; code blocks
//! and quotes become their own widgets (they need distinct surfaces), so one
//! message is a vertical box of labels and blocks.

use std::collections::HashMap;
use std::fmt::Write;

use discord::markup::{Entity, Span, Style};
use gtk::pango;
use gtk::prelude::*;

/// Names we can resolve while rendering, gathered from the message and the
/// open guild. Missing ids fall back to a readable placeholder.
pub struct Names {
    pub users: HashMap<u64, String>,
    pub channels: HashMap<u64, String>,
}

impl Names {
    /// Used when the App is gone (window closing): mentions fall back to ids.
    pub fn empty() -> Self {
        Names { users: HashMap::new(), channels: HashMap::new() }
    }
}

const MENTION_BG: &str = "#3c4270";
const MENTION_FG: &str = "#c9cdfb";
const CODE_BG: &str = "#1e1f22";
const SPOILER_OPEN: &str = "<span background=\"#17181b\">";

const EDITED_MARKER: &str = " <span foreground=\"#949BA4\" size=\"small\">(edited)</span>";

/// Build the content widget for one message body. `edited` appends a faint
/// "(edited)" marker after the content.
pub fn render(spans: &[Span], names: &Names, edited: bool) -> gtk::Widget {
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
    let mut inline = String::new();

    let flush = |inline: &mut String, column: &gtk::Box| {
        let trimmed = inline.trim_matches('\n');
        if !trimmed.is_empty() {
            column.append(&inline_label(trimmed));
        }
        inline.clear();
    };

    for span in spans {
        match span {
            Span::CodeBlock { lang, code } => {
                flush(&mut inline, &column);
                column.append(&code_block(lang.as_deref(), code));
            }
            Span::Styled(Style::Quote, children) => {
                flush(&mut inline, &column);
                column.append(&quote_block(children, names));
            }
            other => inline_markup(other, names, &mut inline),
        }
    }
    // Trail the marker on the last inline line, or on its own if the content
    // ended with a block (code/quote).
    if edited {
        inline.push_str(EDITED_MARKER);
    }
    flush(&mut inline, &column);

    // An empty body (e.g. a message that was only an attachment) renders nothing.
    if column.first_child().is_none() {
        column.append(&inline_label(""));
    }
    column.upcast()
}

fn inline_label(markup: &str) -> gtk::Label {
    gtk::Label::builder()
        .use_markup(true)
        .label(markup)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .build()
}

fn code_block(lang: Option<&str>, code: &str) -> gtk::Widget {
    let container = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["code-block"]).build();
    if let Some(lang) = lang.filter(|l| !l.is_empty()) {
        container.append(&gtk::Label::builder().label(lang).xalign(0.0).css_classes(["code-lang"]).build());
    }
    container.append(
        &gtk::Label::builder()
            .label(code)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .xalign(0.0)
            .selectable(true)
            .css_classes(["code-body"])
            .build(),
    );
    container.upcast()
}

fn quote_block(children: &[Span], names: &Names) -> gtk::Widget {
    let mut markup = String::new();
    for span in children {
        inline_markup(span, names, &mut markup);
    }
    let bar = gtk::Box::builder().css_classes(["quote"]).build();
    bar.append(&inline_label(markup.trim_matches('\n')));
    bar.upcast()
}

/// Append the Pango markup for one inline span.
fn inline_markup(span: &Span, names: &Names, out: &mut String) {
    match span {
        Span::Text(t) => out.push_str(&escape(t)),
        Span::Break => out.push('\n'),
        Span::Styled(style, children) => {
            let (open, close) = tags(*style);
            out.push_str(open);
            for child in children {
                inline_markup(child, names, out);
            }
            out.push_str(close);
        }
        Span::Code(code) => {
            let _ = write!(out, "<tt><span background=\"{CODE_BG}\"> {} </span></tt>", escape(code));
        }
        Span::CodeBlock { code, .. } => {
            // A code block nested where only inline is expected: show inline.
            let _ = write!(out, "<tt>{}</tt>", escape(code));
        }
        Span::Link { url, children } => {
            let mut inner = String::new();
            for child in children {
                inline_markup(child, names, &mut inner);
            }
            let _ = write!(out, "<a href=\"{}\">{}</a>", escape(url), inner);
        }
        Span::Entity(entity) => out.push_str(&entity_markup(entity, names)),
    }
}

fn tags(style: Style) -> (&'static str, &'static str) {
    match style {
        Style::Bold => ("<b>", "</b>"),
        Style::Italic => ("<i>", "</i>"),
        Style::Underline => ("<u>", "</u>"),
        Style::Strikethrough => ("<s>", "</s>"),
        // Click-to-reveal comes later; for now mark it with a dark background.
        Style::Spoiler => (SPOILER_OPEN, "</span>"),
        // Quotes are handled as blocks; if one is nested inline, pass through.
        Style::Quote => ("", ""),
    }
}

/// What an entity reads as: `@name`, `#channel`, `:emoji:`, a date.
fn entity_text(entity: &Entity, names: &Names) -> String {
    match entity {
        Entity::User(id) => format!("@{}", names.users.get(id).map_or("unknown-user", String::as_str)),
        Entity::Channel(id) => format!("#{}", names.channels.get(id).map_or("unknown-channel", String::as_str)),
        Entity::Role(_) => "@role".into(),
        Entity::Emoji { name, .. } => format!(":{name}:"),
        Entity::Timestamp { unix, style } => format_timestamp(*unix, *style),
    }
}

fn entity_markup(entity: &Entity, names: &Names) -> String {
    let text = escape(&entity_text(entity, names));
    match entity {
        // Emoji read as plain text until they render as images.
        Entity::Emoji { .. } => text,
        _ => format!("<span background=\"{MENTION_BG}\" foreground=\"{MENTION_FG}\">\u{a0}{text}\u{a0}</span>"),
    }
}

/// A message as plain text, for desktop notifications. Spoilers stay hidden.
pub fn plain_text(spans: &[Span], names: &Names) -> String {
    fn walk(spans: &[Span], names: &Names, out: &mut String) {
        for span in spans {
            match span {
                Span::Text(text) | Span::Code(text) => out.push_str(text),
                Span::CodeBlock { code, .. } => out.push_str(code),
                Span::Styled(Style::Spoiler, _) => out.push_str("▒▒▒▒"),
                Span::Styled(_, children) | Span::Link { children, .. } => walk(children, names, out),
                Span::Entity(entity) => out.push_str(&entity_text(entity, names)),
                Span::Break => out.push('\n'),
            }
        }
    }
    let mut out = String::new();
    walk(spans, names, &mut out);
    out.trim().to_owned()
}

fn format_timestamp(unix: i64, style: char) -> String {
    let Ok(utc) = glib::DateTime::from_unix_utc(unix) else {
        return String::new();
    };
    let local = utc.to_local().unwrap_or(utc);
    let pattern = match style {
        't' => "%H:%M",
        'T' => "%H:%M:%S",
        'd' => "%Y-%m-%d",
        'D' => "%e %B %Y",
        'F' => "%A, %e %B %Y %H:%M",
        // 'R' (relative) needs a live-updating label; show an absolute time.
        _ => "%e %B %Y %H:%M",
    };
    local.format(pattern).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn escape(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use discord::markup::parse;

    fn names() -> Names {
        Names {
            users: HashMap::from([(5, "alice".to_string())]),
            channels: HashMap::from([(9, "general".to_string())]),
        }
    }

    /// Exercises the inline markup path without needing a GTK display.
    fn inline(input: &str) -> String {
        let names = names();
        let mut out = String::new();
        for span in parse(input) {
            inline_markup(&span, &names, &mut out);
        }
        out
    }

    #[test]
    fn styles_become_pango() {
        assert_eq!(inline("**hi**"), "<b>hi</b>");
        assert_eq!(inline("*hi*"), "<i>hi</i>");
        assert_eq!(inline("~~hi~~"), "<s>hi</s>");
    }

    #[test]
    fn user_and_channel_mentions_resolve() {
        assert!(inline("<@5>").contains("@alice"));
        assert!(inline("<#9>").contains("#general"));
        assert!(inline("<@999>").contains("@unknown-user"));
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(inline("a < b & c"), "a &lt; b &amp; c");
    }

    #[test]
    fn link_markup() {
        assert_eq!(
            inline("[x](https://e.com)"),
            "<a href=\"https://e.com\">x</a>"
        );
    }

    #[test]
    fn plain_text_for_notifications() {
        let names = Names { users: [(5, "alice".into())].into(), channels: HashMap::new() };
        let text = |s: &str| plain_text(&discord::markup::parse(s), &names);
        assert_eq!(text("**hey** <@5>, look: ||secret||"), "hey @alice, look: ▒▒▒▒");
        assert_eq!(text("<:blob:42>"), ":blob:");
    }

    #[test]
    fn emoji_shows_name() {
        assert_eq!(inline("<:blob:42>"), ":blob:");
    }
}
