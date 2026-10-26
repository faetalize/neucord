//! Embeds: bot cards, link previews, and the media behind image, video and
//! GIF links, plus the rules for what of them to show.
//!
//! Media is only ever loaded through Discord's media proxy (`proxy_url`),
//! never from the original site, so third parties don't see the user's IP.
//! Anything without a proxy URL is not shown.

use serde::{Deserialize, Deserializer};

use crate::markup::{Span, Style};
use crate::settings::MediaSettings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmbedKind {
    /// Bot and webhook cards.
    #[default]
    Rich,
    /// A link to an image: shown as just the image.
    Image,
    /// A link to a video page (YouTube and the like).
    Video,
    /// A GIF link (Tenor, Giphy): a short looping video, shown as just that.
    Gifv,
    Article,
    Link,
    /// Poll results, component previews and the like, which need their own
    /// UI. Not shown.
    Other,
}

impl<'de> Deserialize<'de> for EmbedKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match <std::borrow::Cow<'de, str>>::deserialize(d)?.as_ref() {
            "rich" => Self::Rich,
            "image" => Self::Image,
            "video" => Self::Video,
            "gifv" => Self::Gifv,
            "article" => Self::Article,
            "link" => Self::Link,
            _ => Self::Other,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Embed {
    #[serde(rename = "type", default)]
    pub kind: EmbedKind,
    /// The link the embed is for.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Markup, like message content.
    #[serde(default)]
    pub description: Option<String>,
    /// 0xRRGGBB for the bar down the left.
    #[serde(default)]
    pub color: Option<u32>,
    /// ISO 8601, shown in the footer.
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub author: Option<EmbedAuthor>,
    /// The site, for link previews.
    #[serde(default)]
    pub provider: Option<EmbedProvider>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub video: Option<EmbedMedia>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedAuthor {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedProvider {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedFooter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedField {
    #[serde(default)]
    pub name: String,
    /// Markup.
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedMedia {
    /// The original. Never loaded: see the module docs.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub flags: u64,
}

/// Something to load through the media proxy and show.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// An animated image (a GIF, not a GIF link's video).
    pub animated: bool,
}

/// Embed media and attachment flag: an animated image.
pub(crate) const IS_ANIMATED: u64 = 1 << 5;

impl EmbedMedia {
    fn proxied(&self) -> Option<Media> {
        let url = self.proxy_url.clone().filter(|u| !u.is_empty())?;
        // The flag is newer; older embeds only tell by type or extension.
        let animated = self.flags & IS_ANIMATED != 0
            || self.content_type.as_deref() == Some("image/gif")
            || url.split('?').next().is_some_and(|path| path.to_ascii_lowercase().ends_with(".gif"));
        Some(Media { url, width: self.width, height: self.height, animated })
    }
}

/// How an embed shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Display {
    /// An image link: only the image.
    Image(Media),
    /// A GIF link: only the looping video, with a still to show until it
    /// loads (or instead, when GIFs don't autoplay).
    Gifv { video: Media, still: Option<Media> },
    /// A card. `media` is shown large inside it: the image, or for a video
    /// page the playable video if proxied, else its still. `thumbnail` sits
    /// small to the right.
    Card { media: Option<CardMedia>, thumbnail: Option<Media> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum CardMedia {
    Image(Media),
    Video { video: Media, still: Option<Media> },
}

impl Embed {
    /// How to show this embed, with media dropped when `show_media` is off.
    pub fn display(&self, show_media: bool) -> Display {
        let proxied = |m: &Option<EmbedMedia>| m.as_ref().and_then(EmbedMedia::proxied).filter(|_| show_media);
        let (image, thumbnail, video) = (proxied(&self.image), proxied(&self.thumbnail), proxied(&self.video));
        match self.kind {
            // Discord puts an image link's image in `thumbnail`.
            EmbedKind::Image => match image.or(thumbnail) {
                Some(media) => Display::Image(media),
                None => Display::Card { media: None, thumbnail: None },
            },
            EmbedKind::Gifv => match (video, thumbnail) {
                (Some(video), still) => Display::Gifv { video, still },
                (None, Some(still)) => Display::Image(still),
                (None, None) => Display::Card { media: None, thumbnail: None },
            },
            // A video page's thumbnail is its still, shown large.
            EmbedKind::Video => {
                let media = match (video, thumbnail) {
                    (Some(video), still) => Some(CardMedia::Video { video, still }),
                    (None, still) => still.map(CardMedia::Image),
                };
                Display::Card { media: media.or(image.map(CardMedia::Image)), thumbnail: None }
            }
            _ => Display::Card { media: image.map(CardMedia::Image), thumbnail },
        }
    }

    /// Whether this shows as bare media, with no card around it.
    pub fn is_bare_media(&self, show_media: bool) -> bool {
        matches!(self.display(show_media), Display::Image(_) | Display::Gifv { .. })
    }
}

/// The embeds of a message to show, given the viewer's settings and the
/// message's flags.
pub fn visible<'a>(embeds: &'a [Embed], suppressed: bool, settings: &MediaSettings) -> Vec<&'a Embed> {
    if suppressed || !settings.render_embeds {
        return Vec::new();
    }
    embeds.iter().filter(|e| e.kind != EmbedKind::Other).collect()
}

/// Links the message content puts inside spoilers. As in Discord, their
/// embeds stay hidden until the reader reveals them.
pub fn spoilered_links(content: &str) -> Vec<String> {
    fn walk(spans: &[Span], in_spoiler: bool, out: &mut Vec<String>) {
        for span in spans {
            match span {
                Span::Styled(style, children) => walk(children, in_spoiler || *style == Style::Spoiler, out),
                Span::Link { url, children } => {
                    if in_spoiler {
                        out.push(url.clone());
                    }
                    walk(children, in_spoiler, out);
                }
                _ => {}
            }
        }
    }
    let mut links = Vec::new();
    walk(&crate::markup::parse(content), false, &mut links);
    links
}

/// Discord hides the text of a message that is nothing but the link of a
/// bare-media embed (a pasted GIF or image link): the media says it all.
pub fn hides_content(content: &str, embeds: &[&Embed], show_media: bool) -> bool {
    let content = content.trim();
    !content.is_empty() && embeds.iter().any(|e| e.url.as_deref() == Some(content) && e.is_bare_media(show_media))
}

/// Lay fields out in rows as Discord does: up to three inline fields share
/// a row, a field that isn't inline takes a row of its own.
pub fn field_rows(fields: &[EmbedField]) -> Vec<&[EmbedField]> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < fields.len() {
        let mut end = start + 1;
        if fields[start].inline {
            while end < fields.len() && end - start < 3 && fields[end].inline {
                end += 1;
            }
        }
        rows.push(&fields[start..end]);
        start = end;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn embed(value: serde_json::Value) -> Embed {
        serde_json::from_value(value).unwrap()
    }

    fn media(url: &str) -> Media {
        Media { url: url.into(), width: None, height: None, animated: false }
    }

    #[test]
    fn gif_link_plays_the_proxied_video() {
        let tenor = embed(json!({
            "type": "gifv", "url": "https://tenor.com/view/x",
            "provider": {"name": "Tenor", "url": "https://tenor.co"},
            "thumbnail": {"url": "https://media.tenor.com/x.png", "proxy_url": "https://images-ext-1.discordapp.net/x.png"},
            "video": {"url": "https://media.tenor.com/x.mp4", "proxy_url": "https://images-ext-1.discordapp.net/x.mp4"}
        }));
        assert_eq!(
            tenor.display(true),
            Display::Gifv {
                video: media("https://images-ext-1.discordapp.net/x.mp4"),
                still: Some(media("https://images-ext-1.discordapp.net/x.png")),
            }
        );
        let embeds = [&tenor];
        assert!(hides_content(" https://tenor.com/view/x ", &embeds, true));
        assert!(!hides_content("look https://tenor.com/view/x", &embeds, true));
        // Without media there's nothing to stand in for the link.
        assert_eq!(embeds[0].display(false), Display::Card { media: None, thumbnail: None });
        assert!(!hides_content("https://tenor.com/view/x", &embeds, false));
    }

    #[test]
    fn media_without_a_proxy_is_never_loaded() {
        let youtube = embed(json!({
            "type": "video", "url": "https://youtu.be/x", "title": "T",
            "thumbnail": {"url": "https://i.ytimg.com/x.jpg", "proxy_url": "https://images-ext-1.discordapp.net/y.jpg"},
            "video": {"url": "https://www.youtube.com/embed/x", "width": 1280, "height": 720}
        }));
        assert_eq!(
            youtube.display(true),
            Display::Card {
                media: Some(CardMedia::Image(media("https://images-ext-1.discordapp.net/y.jpg"))),
                thumbnail: None
            }
        );
        let unproxied = embed(json!({"type": "image", "url": "u", "thumbnail": {"url": "https://example.com/a.png"}}));
        assert_eq!(unproxied.display(true), Display::Card { media: None, thumbnail: None });
    }

    #[test]
    fn link_preview_has_a_side_thumbnail() {
        let article = embed(json!({
            "type": "article", "title": "News", "description": "Words",
            "thumbnail": {"url": "o", "proxy_url": "p", "width": 400, "height": 200}
        }));
        let thumbnail = Some(Media { url: "p".into(), width: Some(400), height: Some(200), animated: false });
        assert_eq!(article.display(true), Display::Card { media: None, thumbnail });
        assert_eq!(article.display(false), Display::Card { media: None, thumbnail: None });
    }

    #[test]
    fn animation_from_flag_type_or_extension() {
        let animated =
            |image: serde_json::Value| match embed(json!({"type": "image", "thumbnail": image})).display(true) {
                Display::Image(media) => media.animated,
                other => panic!("{other:?}"),
            };
        assert!(animated(json!({"url": "o", "proxy_url": "p.webp", "flags": 32})));
        assert!(animated(json!({"url": "o", "proxy_url": "p", "content_type": "image/gif"})));
        assert!(animated(json!({"url": "o", "proxy_url": "https://x/a.GIF?width=1"})));
        assert!(!animated(json!({"url": "o", "proxy_url": "p.png", "content_type": "image/png"})));
    }

    #[test]
    fn spoilered_links_are_found() {
        assert_eq!(spoilered_links("see ||https://a.com|| and https://b.com"), ["https://a.com"]);
        assert_eq!(spoilered_links("||**[x](https://c.com)**||"), ["https://c.com"]);
        assert!(spoilered_links("https://b.com").is_empty());
    }

    #[test]
    fn settings_and_suppression() {
        let embeds = vec![Embed::default(), embed(json!({"type": "poll_result"}))];
        let on = MediaSettings::default();
        let off = MediaSettings { render_embeds: false, ..on };
        assert_eq!(visible(&embeds, false, &on).len(), 1);
        assert!(visible(&embeds, true, &on).is_empty(), "SUPPRESS_EMBEDS");
        assert!(visible(&embeds, false, &off).is_empty());
    }

    #[test]
    fn fields_share_rows_up_to_three() {
        let field = |inline| EmbedField { name: String::new(), value: String::new(), inline };
        let fields = [field(true), field(true), field(true), field(true), field(false), field(true)];
        let lengths: Vec<usize> = field_rows(&fields).iter().map(|row| row.len()).collect();
        assert_eq!(lengths, [3, 1, 1, 1]);
        assert!(field_rows(&[]).is_empty());
    }

    #[test]
    fn unknown_kinds_parse() {
        assert_eq!(embed(json!({"type": "poll_result"})).kind, EmbedKind::Other);
        assert_eq!(embed(json!({})).kind, EmbedKind::Rich);
    }
}
