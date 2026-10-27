//! Render a parsed Discord message ([`discord::markup`]) into GTK widgets.
//!
//! Inline spans become Pango markup in a [`RichText`], which draws custom
//! emoji among the words and covers spoilers until they are clicked. Code
//! blocks and quotes become their own widgets (they need distinct surfaces),
//! so one message is a vertical box of text and blocks.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use discord::markup::{Entity, Span, Style};
use discord::system::Part;
use gtk::prelude::*;
use gtk::{gdk, pango};

use crate::highlight;
use crate::images::Images;
use crate::rich_text::{self, RichText};

/// Names we can resolve while rendering, gathered from the message and the
/// open guild. Missing ids fall back to a readable placeholder.
#[derive(Clone)]
pub struct Names {
    pub users: HashMap<u64, String>,
    pub channels: HashMap<u64, String>,
}

/// What rendering draws on besides the spans.
pub struct Context<'a> {
    pub names: &'a Names,
    /// Where custom emoji load from. Without, they read `:name:`.
    pub images: Option<&'a Images>,
    /// Remembers revealed spoilers across rebuilds of the same text.
    pub spoilers: Option<Spoilers>,
    /// The text is only emoji, shown large.
    pub jumbo: bool,
}

impl<'a> Context<'a> {
    pub fn new(names: &'a Names, images: Option<&'a Images>) -> Self {
        Context { names, images, spoilers: None, jumbo: false }
    }
}

/// The spoilers of one text the reader has revealed, by number in reading
/// order, and what to tell when they reveal another.
#[derive(Clone)]
pub struct Spoilers {
    pub revealed: HashSet<usize>,
    pub on_reveal: Rc<dyn Fn(usize)>,
}

// Pango markup can't use the stylesheet's colours. A grey shade reads on
// light and dark backgrounds alike; mentions take the accent colour, which
// the UI keeps current with `set_accent`.
const CODE_OPEN: &str = "<tt><span background=\"#808080\" bgalpha=\"15%\">";
const SPOILER_OPEN: &str = "<span background=\"#808080\" bgalpha=\"40%\">";
/// Custom emoji in running text, and alone (the official client's sizes).
const EMOJI: i32 = 22;
const JUMBO_EMOJI: i32 = 48;
/// Unicode emoji alone, about as large as custom ones.
const JUMBO_TEXT: &str = "<span size=\"300%\">";

thread_local! {
    /// Mention background and text colours, as Pango colour strings.
    static ACCENT: RefCell<(String, String)> = RefCell::new(("#3584e4".into(), "#1c71d8".into()));
}

/// Set the colours mentions are drawn in. Messages rendered before keep the
/// old ones until they are rendered again.
pub fn set_accent(background: &gdk::RGBA, foreground: &gdk::RGBA) {
    let hex = |c: &gdk::RGBA| {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", byte(c.red()), byte(c.green()), byte(c.blue()))
    };
    ACCENT.with(|accent| *accent.borrow_mut() = (hex(background), hex(foreground)));
}

/// Build the content widget for one message body. `edited` appends a faint
/// "(edited)" marker after the content.
pub fn render(spans: &[Span], cx: &Context, edited: bool) -> gtk::Widget {
    let column = Blocks { cx, next_spoiler: 0 }.column(spans, edited);
    // An empty body (e.g. a message that was only an attachment) renders nothing.
    if column.first_child().is_none() {
        column.append(&Inline::new(cx, 0).widget(cx));
    }
    column.upcast()
}

/// Builds block widgets, numbering spoilers across all of them.
struct Blocks<'c, 'a> {
    cx: &'c Context<'a>,
    next_spoiler: usize,
}

impl Blocks<'_, '_> {
    /// A column of `spans`: runs of inline spans as text, and blocks.
    fn column(&mut self, spans: &[Span], edited: bool) -> gtk::Box {
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        let mut inline: Vec<&Span> = Vec::new();
        for span in spans {
            let block = match span {
                Span::CodeBlock { lang, code } => code_block(lang.as_deref(), code),
                Span::Styled(Style::Quote, children) => {
                    let bar = gtk::Box::builder().css_classes(["quote"]).build();
                    bar.append(&self.column(children, false));
                    bar.upcast()
                }
                // libadwaita's type scale, from the largest that suits a message.
                Span::Heading { level, children } => {
                    let class = match level {
                        1 => "title-2",
                        2 => "title-3",
                        _ => "title-4",
                    };
                    self.text(children, &[class])
                }
                Span::Subtext(children) => self.text(children, &["caption", "dimmed"]),
                Span::List { start, items } => self.list(*start, items),
                other => {
                    inline.push(other);
                    continue;
                }
            };
            self.flush(&column, &mut inline, false);
            column.append(&block);
        }
        // Trail the marker on the last inline line, or on its own if the
        // content ended with a block.
        self.flush(&column, &mut inline, edited);
        column
    }

    fn flush(&mut self, column: &gtk::Box, inline: &mut Vec<&Span>, edited: bool) {
        let mut text = Inline::new(self.cx, self.next_spoiler);
        let spans = trim_breaks(inline);
        if self.cx.jumbo && !spans.is_empty() {
            text.tag(JUMBO_TEXT);
            text.spans(spans);
            text.tag("</span>");
        } else {
            text.spans(spans);
        }
        if edited {
            text.edited_marker();
        }
        self.next_spoiler = text.next_spoiler;
        if !text.is_empty() {
            column.append(&text.widget(self.cx));
        }
        inline.clear();
    }

    fn text(&mut self, spans: &[Span], classes: &[&str]) -> gtk::Widget {
        let mut text = Inline::new(self.cx, self.next_spoiler);
        text.spans(spans);
        self.next_spoiler = text.next_spoiler;
        let widget = text.widget(self.cx);
        for class in classes {
            widget.add_css_class(class);
        }
        widget
    }

    /// Items beside their bullets or numbers, which line up on the right.
    /// Not a grid: one measures wrapping text at its narrowest, so rows
    /// would come out too tall.
    fn list(&mut self, start: Option<u32>, items: &[Vec<Span>]) -> gtk::Widget {
        let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
        let markers = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        for (n, item) in items.iter().enumerate() {
            let marker = match start {
                Some(start) => format!("{}.", start as u64 + n as u64),
                None => "•".to_owned(),
            };
            let marker = gtk::Label::builder().label(marker).xalign(1.0).valign(gtk::Align::Start).build();
            markers.add_widget(&marker);
            let content = self.column(item, false);
            content.set_hexpand(true);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(&marker);
            row.append(&content);
            list.append(&row);
        }
        list.upcast()
    }
}

/// Line breaks at either end of a run of inline spans take room for nothing.
fn trim_breaks<'s>(spans: &[&'s Span]) -> Vec<&'s Span> {
    let start = spans.iter().position(|s| **s != Span::Break).unwrap_or(spans.len());
    let end = spans.iter().rposition(|s| **s != Span::Break).map_or(start, |e| e + 1);
    spans[start..end].to_vec()
}

fn code_block(lang: Option<&str>, code: &str) -> gtk::Widget {
    let container = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["code-block"]).build();
    let lang = lang.filter(|l| !l.is_empty());
    if let Some(lang) = lang {
        container.append(&gtk::Label::builder().label(lang).xalign(0.0).css_classes(["code-lang"]).build());
    }
    let label = gtk::Label::builder()
        .label(code)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .css_classes(["code-body"])
        .build();
    let dark = adw::StyleManager::default().is_dark();
    let markup = match lang {
        Some(lang) if lang.eq_ignore_ascii_case("ansi") => Some(ansi_markup(code)),
        Some(lang) => highlight::markup(lang, code, dark),
        None => None,
    };
    if let Some(markup) = markup {
        label.set_markup(&markup);
    }
    container.append(&label);
    container.upcast()
}

/// An `ansi` block without its escape codes, bold and underlined where they
/// say. Their colours are left out.
fn ansi_markup(code: &str) -> String {
    let mut out = String::new();
    for part in discord::markup::ansi(code) {
        let text = escape(&part.text);
        out += &match (part.bold, part.underline) {
            (false, false) => text,
            (bold, underline) => format!(
                "<span{}{}>{text}</span>",
                if bold { " weight=\"bold\"" } else { "" },
                if underline { " underline=\"single\"" } else { "" }
            ),
        };
    }
    out
}

/// A custom emoji waiting for its image.
struct Emoji {
    id: u64,
    animated: bool,
}

/// Pango markup for a run of inline spans, with where its images and
/// spoilers fall in the text it parses to.
struct Inline<'a> {
    names: &'a Names,
    /// Custom emoji show as images (else as `:name:`).
    images: bool,
    emoji_size: i32,
    markup: String,
    /// Bytes of text so far: the markup without its tags and escapes.
    len: u32,
    emoji: Vec<(rich_text::Image, Emoji)>,
    spoilers: Vec<(Range<u32>, usize)>,
    /// The number the next spoiler gets, counted across the whole message.
    next_spoiler: usize,
}

impl<'a> Inline<'a> {
    fn new(cx: &Context<'a>, next_spoiler: usize) -> Self {
        Inline {
            names: cx.names,
            images: cx.images.is_some(),
            emoji_size: if cx.jumbo { JUMBO_EMOJI } else { EMOJI },
            markup: String::new(),
            len: 0,
            emoji: Vec::new(),
            spoilers: Vec::new(),
            next_spoiler,
        }
    }

    fn is_empty(&self) -> bool {
        self.markup.is_empty()
    }

    fn text(&mut self, text: &str) {
        // The object character means an image here; text can't have one.
        let text = text.replace(rich_text::OBJECT, "");
        self.markup.push_str(&escape(&text));
        self.len += text.len() as u32;
    }

    fn tag(&mut self, tag: &str) {
        self.markup.push_str(tag);
    }

    fn spans(&mut self, spans: impl IntoIterator<Item = &'a Span>) {
        for span in spans {
            self.span(span);
        }
    }

    fn span(&mut self, span: &'a Span) {
        match span {
            Span::Text(t) => self.text(t),
            Span::Break => self.text("\n"),
            Span::Styled(Style::Spoiler, children) => {
                let start = self.len;
                let number = self.next_spoiler;
                self.next_spoiler += 1;
                self.tag(SPOILER_OPEN);
                self.spans(children);
                self.tag("</span>");
                self.spoilers.push((start..self.len, number));
            }
            Span::Styled(style, children) => {
                let (open, close) = tags(*style);
                self.tag(open);
                self.spans(children);
                self.tag(close);
            }
            Span::Code(code) => {
                self.tag(CODE_OPEN);
                self.text(&format!(" {code} "));
                self.tag("</span></tt>");
            }
            Span::CodeBlock { code, .. } => {
                // A code block nested where only inline is expected: show inline.
                self.tag("<tt>");
                self.text(code);
                self.tag("</tt>");
            }
            Span::Link { url, children } => {
                self.tag(&format!("<a href=\"{}\">", escape(url)));
                self.spans(children);
                self.tag("</a>");
            }
            Span::Entity(Entity::Emoji { name, id, animated }) if self.images => {
                let image = rich_text::Image {
                    index: self.len,
                    size: self.emoji_size,
                    alt: format!(":{name}:"),
                    paintable: None,
                };
                self.emoji.push((image, Emoji { id: *id, animated: *animated }));
                self.text_object();
            }
            // Blocks met where only text can go read as their text.
            Span::Heading { children, .. } | Span::Subtext(children) => self.spans(children),
            Span::List { items, .. } => {
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        self.text("\n");
                    }
                    self.spans(item);
                }
            }
            Span::Entity(entity @ Entity::Emoji { .. }) => self.text(&entity_text(entity, self.names)),
            Span::Entity(entity) => {
                let (background, foreground) = ACCENT.with(|accent| accent.borrow().clone());
                self.tag(&format!("<span background=\"{background}\" bgalpha=\"25%\" foreground=\"{foreground}\">"));
                self.text(&format!("\u{a0}{}\u{a0}", entity_text(entity, self.names)));
                self.tag("</span>");
            }
        }
    }

    fn text_object(&mut self) {
        let mut buf = [0; 4];
        self.markup.push_str(rich_text::OBJECT.encode_utf8(&mut buf));
        self.len += rich_text::OBJECT.len_utf8() as u32;
    }

    fn edited_marker(&mut self) {
        self.text(" ");
        self.tag("<span fgalpha=\"55%\" size=\"small\">");
        self.text("(edited)");
        self.tag("</span>");
    }

    /// The widget, its emoji loading.
    fn widget(self, cx: &Context) -> gtk::Widget {
        let (images, emoji): (Vec<_>, Vec<_>) = self.emoji.into_iter().unzip();
        let revealed = |n: &usize| cx.spoilers.as_ref().is_some_and(|s| s.revealed.contains(n));
        let numbers: Vec<usize> = self.spoilers.iter().map(|(_, n)| *n).collect();
        let spoilers = self.spoilers.into_iter().map(|(range, n)| (range, revealed(&n))).collect();
        let text = RichText::new(&self.markup, images, spoilers);
        if let Some(memory) = &cx.spoilers {
            let on_reveal = memory.on_reveal.clone();
            text.connect_reveal(move |i| on_reveal(numbers[i]));
        }
        if let Some(images) = cx.images {
            let size = if cx.jumbo { JUMBO_EMOJI } else { EMOJI };
            let request = emoji_request(size, images.scale());
            for (n, emoji) in emoji.into_iter().enumerate() {
                let weak = text.downgrade();
                let url = discord::cdn::emoji_url(emoji.id, emoji.animated, request);
                images.load(&url, emoji.animated, move |paintable| {
                    if let Some(text) = weak.upgrade() {
                        text.set_image(n, paintable);
                    }
                });
            }
        }
        text.upcast()
    }
}

/// The CDN size to ask for an emoji shown at `size`: enough pixels for the
/// screen's scale, in the powers of two the CDN serves.
fn emoji_request(size: i32, scale: i32) -> u16 {
    ((size * scale.max(1)) as u32).next_power_of_two().clamp(16, 4096) as u16
}

fn tags(style: Style) -> (&'static str, &'static str) {
    match style {
        Style::Bold => ("<b>", "</b>"),
        Style::Italic => ("<i>", "</i>"),
        Style::Underline => ("<u>", "</u>"),
        Style::Strikethrough => ("<s>", "</s>"),
        // Tinted once revealed; until then the text covers it.
        Style::Spoiler => (SPOILER_OPEN, "</span>"),
        // Quotes are handled as blocks; if one is nested inline, pass through.
        Style::Quote => ("", ""),
    }
}

/// Pango markup for a system message's line, with when it happened.
pub fn system_markup(parts: &[Part], time: &str) -> String {
    let mut out = String::new();
    for part in parts {
        out += &match part {
            Part::Text(text) => escape(text),
            Part::Name(text) | Part::Strong(text) => format!("<b>{}</b>", escape(text)),
            Part::Jump { label, to } => format!("<a href=\"{JUMP}{to}\">{}</a>", escape(label)),
        };
    }
    out + &format!("  <span fgalpha=\"55%\" size=\"small\">{}</span>", escape(time))
}

/// Links to a message in the same channel.
const JUMP: &str = "neucord:message/";

/// The message a link made by [`system_markup`] jumps to.
pub fn jump_target(uri: &str) -> Option<discord::Snowflake> {
    uri.strip_prefix(JUMP)?.parse().ok().map(discord::Snowflake)
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
                // Blocks take their own lines.
                Span::Heading { children, .. } | Span::Subtext(children) => {
                    walk(children, names, out);
                    out.push('\n');
                }
                Span::List { items, .. } => {
                    for item in items {
                        walk(item, names, out);
                        out.push('\n');
                    }
                }
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
        let spans = parse(input);
        let mut text = Inline::new(&Context::new(&names, None), 0);
        text.spans(&spans);
        text.markup
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
        assert_eq!(inline("[x](https://e.com)"), "<a href=\"https://e.com\">x</a>");
    }

    #[test]
    fn images_and_spoilers_know_where_they_are() {
        let names = names();
        let spans = parse("a & <:blob:42> ||é <:spin:43>|| **||b||**");
        let mut text = Inline::new(&Context::new(&names, None), 3);
        text.images = true;
        text.spans(&spans);
        let parsed = pango::parse_markup(&text.markup, '\0').unwrap().1;
        let at = |index: u32| &parsed[index as usize..];
        let indices: Vec<u32> = text.emoji.iter().map(|(image, _)| image.index).collect();
        assert!(indices.iter().all(|&i| at(i).starts_with(rich_text::OBJECT)));
        assert_eq!(text.emoji.iter().map(|(image, _)| image.alt.as_str()).collect::<Vec<_>>(), [":blob:", ":spin:"]);
        let spoilers: Vec<(&str, usize)> =
            text.spoilers.iter().map(|(r, n)| (&parsed[r.start as usize..r.end as usize], *n)).collect();
        assert_eq!(spoilers, [("é \u{fffc}", 3), ("b", 4)]);
        assert_eq!(text.next_spoiler, 5);
    }

    #[test]
    fn system_lines() {
        let parts = [
            Part::Name("a&b".into()),
            Part::Text(" pinned ".into()),
            Part::Jump { label: "a message".into(), to: discord::Snowflake(9) },
        ];
        let markup = system_markup(&parts, "12:00");
        assert!(markup.starts_with("<b>a&amp;b</b> pinned <a href=\"neucord:message/9\">a message</a>"));
        assert_eq!(jump_target("neucord:message/9"), Some(discord::Snowflake(9)));
        assert_eq!(jump_target("https://e.com"), None);
    }

    #[test]
    fn jumbo_markup_parses() {
        assert!(pango::parse_markup(&format!("{JUMBO_TEXT}x</span>"), '\0').is_ok());
    }

    #[test]
    fn object_character_in_text_is_dropped() {
        assert_eq!(inline("a\u{fffc}b"), "ab");
    }

    #[test]
    fn plain_text_for_notifications() {
        let names = Names { users: [(5, "alice".into())].into(), channels: HashMap::new() };
        let text = |s: &str| plain_text(&discord::markup::parse(s), &names);
        assert_eq!(text("**hey** <@5>, look: ||secret||"), "hey @alice, look: ▒▒▒▒");
        assert_eq!(text("<:blob:42>"), ":blob:");
        assert_eq!(text("# Title\n- one\n- two\nend"), "Title\none\ntwo\nend");
    }

    #[test]
    fn emoji_without_images_shows_name() {
        assert_eq!(inline("<:blob:42>"), ":blob:");
    }
}
