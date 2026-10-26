//! Embeds: bot cards, link previews, and the media behind image, video and
//! GIF links, plus the rules for what of them to show.
//!
//! The rules follow Discord's own client: it reshapes each embed (which
//! media counts as the large image, the thumbnail, a playable video),
//! merges embeds that share a URL into one with an image gallery, and only
//! then decides between a card and bare media.
//!
//! Media is only ever loaded through Discord's media proxy (`proxy_url`),
//! never from the original site, so third parties don't see the user's IP.
//! The official client falls back to original URLs; Neucord shows nothing.

use serde::{Deserialize, Deserializer};

use crate::Message;
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
    Audio,
    /// Types that need their own UI (poll results, automod notices,
    /// component previews…), which the official client doesn't draw as cards
    /// either. Not shown.
    Hidden,
    /// Anything else (`tweet`, `text`, newer types): a plain card, as in the
    /// official client.
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
            "audio" => Self::Audio,
            "auto_moderation_message"
            | "auto_moderation_notification"
            | "age_verification_system_notification"
            | "application_news"
            | "post_preview"
            | "safety_policy_notice"
            | "safety_system_notification"
            | "voice_channel"
            | "poll_result"
            | "gift"
            | "components" => Self::Hidden,
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
    /// Images of later embeds with the same `url`, gathered by [`merge`].
    #[serde(skip)]
    pub gallery: Vec<EmbedMedia>,
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
    pub width: u32,
    pub height: u32,
    /// An animated image (a GIF, not a GIF link's video).
    pub animated: bool,
}

/// Embed media and attachment flag: an animated image.
const IS_ANIMATED: u64 = 1 << 5;

/// Whether an image animates, by the official client's rule: a `.gif`
/// original, or a WebP or AVIF one flagged as animated.
pub fn looks_animated(original: &str, flags: u64) -> bool {
    let path = original.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    path.ends_with(".gif") || (flags & IS_ANIMATED != 0 && (path.ends_with(".webp") || path.ends_with(".avif")))
}

impl EmbedMedia {
    /// Media without a size is dropped, as in the official client.
    fn sized(&self) -> Option<(u32, u32)> {
        self.width.zip(self.height).filter(|&(w, h)| w > 0 && h > 0)
    }

    fn proxied(&self) -> Option<Media> {
        let (width, height) = self.sized()?;
        let url = self.proxy_url.clone().filter(|u| !u.is_empty())?;
        let original = self.url.as_deref().filter(|u| !u.is_empty()).unwrap_or(&url);
        let animated = looks_animated(original, self.flags);
        Some(Media { url, width, height, animated })
    }
}

/// The embed's media after the official client's reshaping.
#[derive(Default)]
struct Shaped {
    image: Option<Media>,
    thumbnail: Option<Media>,
    /// A video Discord would show. `None` inside when it has no proxied file,
    /// as for YouTube: then only its still can be shown.
    video: Option<Option<Media>>,
    gallery: Vec<Media>,
}

/// Large media, shown in a card or bare.
#[derive(Clone, Debug, PartialEq)]
pub enum Large {
    Image(Media),
    /// Images of several embeds sharing a URL (multi-image posts), up to four.
    Gallery(Vec<Media>),
    /// `video` is the playable file, if proxied; otherwise only the still shows.
    Video {
        video: Option<Media>,
        still: Media,
    },
    /// A GIF link's muted looping video, over its still.
    Gifv {
        video: Media,
        still: Media,
    },
}

/// How an embed shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Display {
    /// Bare media with no card, for image and GIF links and the like.
    Bare(Large),
    /// A card, with large media below its text and a small thumbnail beside it.
    Card { media: Option<Large>, thumbnail: Option<Media> },
}

impl Embed {
    fn shaped(&self) -> Shaped {
        let proxied = |m: &Option<EmbedMedia>| m.as_ref().and_then(EmbedMedia::proxied);
        let mut shaped = Shaped::default();
        // Image and article links carry their picture as a thumbnail, but
        // show it large.
        match self.kind {
            EmbedKind::Article | EmbedKind::Image => shaped.image = proxied(&self.thumbnail),
            _ => shaped.thumbnail = proxied(&self.thumbnail),
        }
        if let Some(image) = proxied(&self.image) {
            shaped.image = Some(image);
        }
        if let Some(video) = &self.video
            && let Some((width, height)) = video.sized()
        {
            let file = video.proxy_url.clone().filter(|u| !u.is_empty());
            // A video without a still gets one from the proxy (the official
            // client asks for WebP; PNG is what GTK can decode everywhere).
            if shaped.thumbnail.is_none()
                && let Some(file) = &file
            {
                let still = crate::cdn::with_params(file, &[("format", "png")]);
                shaped.thumbnail = Some(Media { url: still, width, height, animated: false });
            }
            let shown = file.is_some() || video.url.as_deref().is_some_and(|u| u.starts_with("https:"));
            if shaped.thumbnail.is_some() && shown {
                shaped.video = Some(file.map(|url| Media { url, width, height, animated: false }));
            }
        }
        if !self.gallery.is_empty() {
            shaped.gallery = shaped
                .image
                .iter()
                .cloned()
                .chain(self.gallery.iter().filter_map(EmbedMedia::proxied))
                .take(4)
                .collect();
        }
        shaped
    }

    /// How to show this embed. With `show_media` off (the viewer's
    /// `inline_embed_media`), every embed is a card without media.
    pub fn display(&self, show_media: bool) -> Display {
        if !show_media {
            return Display::Card { media: None, thumbnail: None };
        }
        let shaped = self.shaped();
        let bare = (shaped.image.is_some() || shaped.video.is_some())
            && (self.kind == EmbedKind::Gifv
                || (self.kind != EmbedKind::Rich && self.author.is_none() && self.title.is_none()));
        let media = match (self.kind, shaped.video.clone(), shaped.thumbnail.clone()) {
            // A GIF link plays only with both its page URL and a still.
            (EmbedKind::Gifv, Some(Some(video)), Some(still)) if self.url.is_some() => {
                Some(Large::Gifv { video, still })
            }
            (EmbedKind::Gifv, _, still) => shaped.image.clone().or(still).map(Large::Image),
            (_, Some(video), Some(still)) => Some(Large::Video { video, still }),
            _ if shaped.gallery.len() > 1 => Some(Large::Gallery(shaped.gallery.clone())),
            _ => shaped.image.clone().map(Large::Image),
        };
        if bare && let Some(media) = media {
            return Display::Bare(media);
        }
        // Cards and audio keep their thumbnail unless a video plays; other
        // types show it only when there is no large media.
        let thumbnail = match self.kind {
            EmbedKind::Rich | EmbedKind::Audio => shaped.thumbnail.filter(|_| shaped.video.is_none()),
            _ => shaped.thumbnail.filter(|_| media.is_none()),
        };
        Display::Card { media, thumbnail }
    }

    /// Image, video and GIF links show no description.
    pub fn shows_description(&self) -> bool {
        !matches!(self.kind, EmbedKind::Image | EmbedKind::Video | EmbedKind::Gifv)
    }

    /// Only bot cards have fields, and only their text is markup: a link
    /// preview's title and description come from a website and are shown as
    /// they are.
    pub fn is_rich(&self) -> bool {
        self.kind == EmbedKind::Rich
    }

    /// The colour of the bar down the card. White means none.
    pub fn bar_colour(&self) -> Option<u32> {
        self.color.filter(|&c| c != 0xFFFFFF)
    }
}

/// Merge embeds that share a URL, as the official client does: the first
/// keeps its place and gathers the others' images into a gallery; the rest
/// are dropped.
pub fn merge(embeds: &[Embed]) -> Vec<Embed> {
    let mut merged: Vec<Embed> = Vec::new();
    for embed in embeds {
        let first = embed.url.as_ref().and_then(|url| merged.iter_mut().find(|e| e.url.as_ref() == Some(url)));
        match first {
            None => merged.push(embed.clone()),
            Some(first) => first.gallery.extend(embed.image.clone()),
        }
    }
    merged
}

/// Message types people write, the only ones whose embeds show.
const USER_MESSAGES: [u8; 9] = [0, 19, 20, 23, 26, 41, 45, 47, 68];

/// The embeds of `message` to show, merged, given the viewer's settings.
/// The `SUPPRESS_EMBEDS` flag is honoured too, though Discord already
/// removes the embeds of such messages.
pub fn visible(message: &Message, settings: &MediaSettings) -> Vec<Embed> {
    if message.embeds_suppressed() || !settings.render_embeds || !USER_MESSAGES.contains(&message.kind) {
        return Vec::new();
    }
    merge(&message.embeds).into_iter().filter(|e| e.kind != EmbedKind::Hidden).collect()
}

/// Whether the content puts any link inside a spoiler. The official client
/// then covers every embed of the message, not just that link's.
pub fn has_spoilered_link(content: &str) -> bool {
    fn walk(spans: &[Span], in_spoiler: bool) -> bool {
        spans.iter().any(|span| match span {
            Span::Styled(style, children) => walk(children, in_spoiler || *style == Style::Spoiler),
            Span::Link { children, .. } => in_spoiler || walk(children, in_spoiler),
            _ => false,
        })
    }
    walk(&crate::markup::parse(content), false)
}

/// The official client hides the text of a message that is nothing but
/// one link when its only embed is a bare image or GIF: the media says it
/// all. The link needn't be the embed's URL.
pub fn hides_content(content: &str, embeds: &[Embed], show_media: bool) -> bool {
    let [embed] = embeds else { return false };
    if !matches!(embed.kind, EmbedKind::Image | EmbedKind::Gifv)
        || !matches!(embed.display(show_media), Display::Bare(_))
    {
        return false;
    }
    let spans = crate::markup::parse(content);
    let mut nodes = spans.iter().filter(|s| !matches!(s, Span::Text(t) if t.trim().is_empty()));
    matches!((nodes.next(), nodes.next()), (Some(Span::Link { .. }), None))
}

/// Lay fields out in rows as Discord does: up to three inline fields share
/// a row, a field that isn't inline takes a row of its own. Each field in
/// a row is equally wide.
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

    fn media(url: &str, width: u32, height: u32) -> Media {
        Media { url: url.into(), width, height, animated: false }
    }

    fn tenor() -> Embed {
        embed(json!({
            "type": "gifv", "url": "https://tenor.com/view/x",
            "provider": {"name": "Tenor", "url": "https://tenor.co"},
            "thumbnail": {"url": "https://media.tenor.com/x.png", "proxy_url": "https://images-ext-1.discordapp.net/x.png",
                          "width": 498, "height": 280},
            "video": {"url": "https://media.tenor.com/x.mp4", "proxy_url": "https://images-ext-1.discordapp.net/x.mp4",
                      "width": 498, "height": 280}
        }))
    }

    #[test]
    fn gif_link_plays_the_proxied_video() {
        assert_eq!(
            tenor().display(true),
            Display::Bare(Large::Gifv {
                video: media("https://images-ext-1.discordapp.net/x.mp4", 498, 280),
                still: media("https://images-ext-1.discordapp.net/x.png", 498, 280),
            })
        );
        // Without media it's a card, and there's nothing to stand in for the link.
        assert_eq!(tenor().display(false), Display::Card { media: None, thumbnail: None });
    }

    #[test]
    fn content_hides_behind_a_single_bare_embed() {
        let one = [tenor()];
        assert!(hides_content("https://tenor.com/view/x", &one, true));
        assert!(hides_content("[look](https://tenor.com/view/other)", &one, true), "any single link");
        assert!(!hides_content("look https://tenor.com/view/x", &one, true));
        assert!(!hides_content("https://tenor.com/view/x", &one, false));
        assert!(!hides_content("https://tenor.com/view/x", &[tenor(), tenor()], true), "more than one embed");
    }

    #[test]
    fn video_pages_show_their_still_and_never_the_original() {
        let youtube = embed(json!({
            "type": "video", "url": "https://youtu.be/x", "title": "T",
            "thumbnail": {"url": "https://i.ytimg.com/x.jpg", "proxy_url": "https://images-ext-1.discordapp.net/y.jpg",
                          "width": 1280, "height": 720},
            "video": {"url": "https://www.youtube.com/embed/x", "width": 1280, "height": 720}
        }));
        let still = media("https://images-ext-1.discordapp.net/y.jpg", 1280, 720);
        assert_eq!(
            youtube.display(true),
            Display::Card { media: Some(Large::Video { video: None, still }), thumbnail: None }
        );
    }

    #[test]
    fn media_needs_a_proxy_and_a_size() {
        let unproxied = embed(json!({"type": "image", "url": "u",
            "thumbnail": {"url": "https://example.com/a.png", "width": 10, "height": 10}}));
        assert_eq!(unproxied.display(true), Display::Card { media: None, thumbnail: None });
        let no_size = embed(json!({"type": "image", "url": "u", "thumbnail": {"url": "o", "proxy_url": "p"}}));
        assert_eq!(no_size.display(true), Display::Card { media: None, thumbnail: None });
    }

    #[test]
    fn article_thumbnails_show_large() {
        let article = embed(json!({
            "type": "article", "title": "News", "description": "Words",
            "thumbnail": {"url": "o", "proxy_url": "p", "width": 400, "height": 200}
        }));
        assert_eq!(
            article.display(true),
            Display::Card { media: Some(Large::Image(media("p", 400, 200))), thumbnail: None }
        );
        let link = embed(json!({
            "type": "link", "title": "Site",
            "thumbnail": {"url": "o", "proxy_url": "p", "width": 80, "height": 80}
        }));
        assert_eq!(link.display(true), Display::Card { media: None, thumbnail: Some(media("p", 80, 80)) });
    }

    #[test]
    fn bot_cards_keep_their_thumbnail_beside_an_image() {
        let rich = embed(json!({
            "type": "rich", "title": "Bot",
            "thumbnail": {"url": "o", "proxy_url": "t", "width": 80, "height": 80},
            "image": {"url": "o", "proxy_url": "i", "width": 400, "height": 300}
        }));
        assert_eq!(
            rich.display(true),
            Display::Card { media: Some(Large::Image(media("i", 400, 300))), thumbnail: Some(media("t", 80, 80)) }
        );
        // Untitled, authorless media of other types shows bare.
        let image = embed(
            json!({"type": "image", "url": "u", "thumbnail": {"url": "o", "proxy_url": "p", "width": 5, "height": 5}}),
        );
        assert!(matches!(image.display(true), Display::Bare(Large::Image(_))));
    }

    #[test]
    fn same_url_embeds_merge_into_a_gallery() {
        let post = |image: &str| {
            embed(json!({"type": "rich", "url": "https://x.com/p/1", "title": "Post",
                         "image": {"url": "o", "proxy_url": image, "width": 4, "height": 3}}))
        };
        let other = embed(json!({"type": "rich", "url": "https://e.com", "title": "Other"}));
        let merged = merge(&[post("a"), other, post("b"), post("c")]);
        assert_eq!(merged.len(), 2);
        let gallery = ["a", "b", "c"].map(|u| media(u, 4, 3)).to_vec();
        assert_eq!(merged[0].display(true), Display::Card { media: Some(Large::Gallery(gallery)), thumbnail: None });
    }

    #[test]
    fn animation_by_the_original_url() {
        assert!(looks_animated("https://e.com/a.GIF?x=1", 0));
        assert!(looks_animated("https://e.com/a.webp", IS_ANIMATED));
        assert!(!looks_animated("https://e.com/a.webp", 0));
        assert!(!looks_animated("https://e.com/a.png", IS_ANIMATED));
    }

    #[test]
    fn a_spoilered_link_covers_every_embed() {
        assert!(has_spoilered_link("see ||https://a.com|| and https://b.com"));
        assert!(has_spoilered_link("||**[x](https://c.com)**||"));
        assert!(!has_spoilered_link("||secret|| https://b.com"));
    }

    #[test]
    fn settings_type_and_suppression() {
        let message = |extra: &str| -> Message {
            serde_json::from_str(&format!(
                r#"{{"id":"1","channel_id":"2","author":{{"id":"3","username":"a"}},"timestamp":"t",
                    "embeds":[{{"type":"rich","title":"x"}},{{"type":"poll_result"}}]{extra}}}"#
            ))
            .unwrap()
        };
        let on = MediaSettings::default();
        let off = MediaSettings { render_embeds: false, ..on };
        assert_eq!(visible(&message(""), &on).len(), 1, "poll results are hidden");
        assert!(visible(&message(r#","flags":4"#), &on).is_empty(), "SUPPRESS_EMBEDS");
        assert!(visible(&message(""), &off).is_empty());
        assert!(visible(&message(r#","type":7"#), &on).is_empty(), "system messages show no embeds");
    }

    #[test]
    fn white_means_no_colour() {
        assert_eq!(embed(json!({"color": 0xFFFFFF})).bar_colour(), None);
        assert_eq!(embed(json!({"color": 0x5865F2})).bar_colour(), Some(0x5865F2));
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
    fn kinds() {
        assert_eq!(embed(json!({"type": "poll_result"})).kind, EmbedKind::Hidden);
        assert_eq!(embed(json!({"type": "tweet"})).kind, EmbedKind::Other);
        assert_eq!(embed(json!({})).kind, EmbedKind::Rich);
    }
}
