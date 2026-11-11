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
use crate::frecency::GifFormat;
use crate::gifs::Starred;
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
    /// A closed poll's results, which its system message shows.
    PollResult,
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
            "poll_result" => Self::PollResult,
            "auto_moderation_message"
            | "auto_moderation_notification"
            | "age_verification_system_notification"
            | "application_news"
            | "post_preview"
            | "safety_policy_notice"
            | "safety_system_notification"
            | "voice_channel"
            | "gift"
            | "components" => Self::Hidden,
            _ => Self::Other,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct EmbedAuthor {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct EmbedProvider {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct EmbedFooter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
pub struct EmbedField {
    #[serde(default)]
    pub name: String,
    /// Markup.
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
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
    /// What the star on it favourites, where the client puts one.
    pub star: Option<Starred>,
}

/// Embed media and attachment flag: an animated image.
pub(crate) const IS_ANIMATED: u64 = 1 << 5;

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
        Some(Media { url, width, height, animated, star: None })
    }

    /// An image of a gallery. The client gives each its own star (`e_`,
    /// 265705) when it animates: itself, with no video or provider to go by.
    fn gallery_item(&self) -> Option<Media> {
        let mut item = self.proxied()?;
        if item.animated {
            let link = [self.url.as_deref(), self.proxy_url.as_deref()].into_iter().flatten().find(|u| !u.is_empty());
            item.star = Some(Starred {
                url: link.unwrap_or_default().to_owned(),
                src: item.url.clone(),
                gif_src: None,
                width: item.width,
                height: item.height,
                format: GifFormat::Image,
            });
        }
        Some(item)
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
        let large = self.image.as_ref().filter(|m| m.proxied().is_some()).or(match self.kind {
            EmbedKind::Article | EmbedKind::Image => self.thumbnail.as_ref().filter(|m| m.proxied().is_some()),
            _ => None,
        });
        shaped.image = large.and_then(EmbedMedia::proxied);
        if !matches!(self.kind, EmbedKind::Article | EmbedKind::Image) {
            shaped.thumbnail = proxied(&self.thumbnail);
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
                shaped.thumbnail = Some(Media { url: still, width, height, animated: false, star: None });
            }
            let shown = file.is_some() || video.url.as_deref().is_some_and(|u| u.starts_with("https:"));
            if shaped.thumbnail.is_some() && shown {
                shaped.video = Some(file.map(|url| Media { url, width, height, animated: false, star: None }));
            }
        }
        if !self.gallery.is_empty() {
            shaped.gallery =
                large.into_iter().chain(&self.gallery).filter_map(EmbedMedia::gallery_item).take(4).collect();
        }
        shaped
    }

    /// The embed's picture and video as the client keeps them (`fK` in
    /// 659674), by their original `url`: an image or article's picture is its
    /// thumbnail, unless it has an `image`; a video counts only with a still
    /// (a thumbnail, or one made from its own proxied file), and isn't
    /// Sketchfab's. Media without a size is dropped. A message older than
    /// 2017, which the client lets only a few providers' videos through, can
    /// carry none of the signed links this serves.
    pub(crate) fn client_media(&self) -> (Option<&EmbedMedia>, Option<&EmbedMedia>) {
        let sized = |media: &&EmbedMedia| media.sized().is_some();
        let thumbnail = self.thumbnail.as_ref().filter(sized);
        let mut image = match self.kind {
            EmbedKind::Article | EmbedKind::Image => thumbnail,
            _ => None,
        };
        image = self.image.as_ref().filter(sized).or(image);
        let video = self.video.as_ref().filter(sized).filter(|video| {
            let has_still = (thumbnail.is_some() && !matches!(self.kind, EmbedKind::Article | EmbedKind::Image))
                || video.proxy_url.is_some();
            let url = video.url.as_deref().unwrap_or_default().to_ascii_lowercase();
            let sketchfab = self
                .provider
                .as_ref()
                .and_then(|p| p.name.as_deref())
                .is_some_and(|name| name.to_ascii_lowercase().contains("sketchfab"))
                || url.starts_with("https://sketchfab.com");
            has_still && !sketchfab && (video.proxy_url.is_some() || url.starts_with("https:"))
        });
        (image, video)
    }

    /// What the star on the embed's picture or GIF link favourites: the inputs
    /// `cP` (679636) gives `b` (343552). The picture is the image or else the
    /// video, through the proxy where there is one. A GIF link is filed
    /// under its page, another embed under its image's or video's own link.
    /// Klipy's still-animated file is its thumbnail. A video makes it a
    /// video.
    pub fn star(&self) -> Option<Starred> {
        let (image, video) = self.client_media();
        let shown = image.or(video)?;
        let (width, height) = shown.sized()?;
        let proxy = shown.proxy_url.as_deref().filter(|u| !u.is_empty());
        let src = proxy.or(shown.url.as_deref())?.to_owned();
        let page = match self.kind {
            EmbedKind::Gifv => self.url.as_deref(),
            _ => image.and_then(|i| i.url.as_deref()).or(video.and_then(|v| v.url.as_deref())),
        };
        let url = [page, proxy, shown.url.as_deref()].into_iter().flatten().find(|u| !u.is_empty())?.to_owned();
        let klipy = self.provider.as_ref().and_then(|p| p.name.as_deref()) == Some("Klipy");
        Some(Starred {
            url,
            src,
            gif_src: if klipy { self.client_thumbnail() } else { None },
            width,
            height,
            format: if video.is_some() { GifFormat::Video } else { GifFormat::Image },
        })
    }

    /// The thumbnail as the client keeps it (`fK`, 659674): the embed's own,
    /// except for an image or article, which show it large; or, where there is
    /// none, a still of the video's file as WebP. Its proxied link, else its own.
    fn client_thumbnail(&self) -> Option<String> {
        let own = match self.kind {
            EmbedKind::Article | EmbedKind::Image => None,
            _ => self.thumbnail.as_ref().filter(|media| media.sized().is_some()),
        };
        if let Some(own) = own {
            return own.proxy_url.clone().or_else(|| own.url.clone());
        }
        let file = self.video.as_ref().filter(|video| video.sized().is_some())?.proxy_url.as_deref()?;
        Some(crate::cdn::with_params(file, &[("format", "webp")]))
    }

    /// `media` with the star the client puts on an animated picture of the
    /// embed, from the embed rather than the picture.
    fn starred(&self, media: Media) -> Media {
        let star = if media.animated { self.star() } else { None };
        Media { star, ..media }
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
                Some(Large::Gifv { video: Media { star: self.star(), ..video }, still })
            }
            (EmbedKind::Gifv, _, still) => {
                shaped.image.clone().map(|image| self.starred(image)).or(still).map(Large::Image)
            }
            (_, Some(video), Some(still)) => Some(Large::Video { video, still }),
            _ if shaped.gallery.len() > 1 => Some(Large::Gallery(shaped.gallery.clone())),
            _ => shaped.image.clone().map(|image| self.starred(image)).map(Large::Image),
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

/// The embeds of `message` to show, merged, given the viewer's settings.
/// The `SUPPRESS_EMBEDS` flag is honoured too, though Discord already
/// removes the embeds of such messages. Link embeds of Conjure projects
/// are hidden whatever the settings: the client draws its own card there.
pub fn visible(message: &Message, settings: &MediaSettings) -> Vec<Embed> {
    if message.embeds_suppressed() || !settings.render_embeds || !message.is_user_message() {
        return Vec::new();
    }
    merge(&message.embeds)
        .into_iter()
        .filter(|e| !matches!(e.kind, EmbedKind::Hidden | EmbedKind::PollResult))
        .filter(|e| e.kind == EmbedKind::Rich || !e.url.as_deref().is_some_and(is_conjure_project_link))
        .collect()
}

/// Whether `url` is a Conjure project link: `http(s)` on `conjure.town` or
/// `conjure.city` (`www.` dropped), at `/{application id}/{slug}` with an
/// optional `/{ticket}` and trailing slash. Parsed by hand, as the crate has
/// no WHATWG URL parser. Its normalisation (dot segments, stripped
/// whitespace, encoded or IDNA hosts, odd ports and schemes) isn't
/// replicated: such links stay drawn.
fn is_conjure_project_link(url: &str) -> bool {
    let word = |s: &str, min: usize, max: usize| {
        (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    let Some((scheme, rest)) = url.split_once("://") else { return false };
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => "443",
        "http" => "80",
        _ => return false,
    };
    let (authority, path) = match rest.find(['/', '\\', '?', '#']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let path = path.split(['?', '#']).next().unwrap_or_default().replace('\\', "/");
    // `URL.host` carries a non-default port, which then fails the match.
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = host.strip_suffix(&format!(":{default_port}")).unwrap_or(host).to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if !matches!(host, "conjure.town" | "conjure.city") {
        return false;
    }
    let Some(path) = path.strip_prefix('/') else { return false };
    let mut parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 2 && parts.last() == Some(&"") {
        parts.pop();
    }
    let [id, slug, ticket @ ..] = parts.as_slice() else { return false };
    let ticket_ok = match ticket {
        [] => true,
        [ticket] => ticket.split_once('.').is_some_and(|(a, b)| word(a, 16, 200) && word(b, 16, 64)),
        _ => false,
    };
    (17..=20).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && (1..=48).contains(&slug.len())
        && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && ticket_ok
}

/// Whether the content puts any link inside a spoiler. The official client
/// then covers every embed of the message, not just that link's.
pub fn has_spoilered_link(content: &str) -> bool {
    fn walk(spans: &[Span], in_spoiler: bool) -> bool {
        spans.iter().any(|span| match span {
            Span::Styled(style, children) => walk(children, in_spoiler || *style == Style::Spoiler),
            Span::Link { children, .. } => in_spoiler || walk(children, in_spoiler),
            Span::List { items, .. } => items.iter().any(|item| walk(item, in_spoiler)),
            other => walk(other.children(), in_spoiler),
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
        Media { url: url.into(), width, height, animated: false, star: None }
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

    fn starred(url: &str, src: &str, gif_src: Option<&str>, format: GifFormat) -> Option<Starred> {
        Some(Starred {
            url: url.into(),
            src: src.into(),
            gif_src: gif_src.map(Into::into),
            width: 498,
            height: 280,
            format,
        })
    }

    #[test]
    fn gif_link_plays_the_proxied_video() {
        let star =
            starred("https://tenor.com/view/x", "https://images-ext-1.discordapp.net/x.mp4", None, GifFormat::Video);
        assert_eq!(
            tenor().display(true),
            Display::Bare(Large::Gifv {
                video: Media { star, ..media("https://images-ext-1.discordapp.net/x.mp4", 498, 280) },
                still: media("https://images-ext-1.discordapp.net/x.png", 498, 280),
            })
        );
        // Without media it's a card, and there's nothing to stand in for the link.
        assert_eq!(tenor().display(false), Display::Card { media: None, thumbnail: None });
    }

    #[test]
    fn a_gif_link_is_starred_as_its_page_with_its_video_and_klipys_thumbnail_as_the_gif() {
        let tenor = tenor().star().unwrap();
        assert_eq!(tenor.url, "https://tenor.com/view/x", "a GIF link is filed under its page");
        assert_eq!(tenor.src, "https://images-ext-1.discordapp.net/x.mp4", "the proxied video");
        assert_eq!((tenor.gif_src, tenor.format), (None, GifFormat::Video));

        let mut klipy = tenor_embed("Klipy");
        klipy.thumbnail.as_mut().unwrap().proxy_url = Some("https://images-ext-1.discordapp.net/x.gif".into());
        let star = klipy.star().unwrap();
        assert_eq!(star.gif_src.as_deref(), Some("https://images-ext-1.discordapp.net/x.gif"));
        assert_eq!(star.format, GifFormat::Video, "what the star hands over; the favourite is an image once stored");
        // Without a thumbnail of its own, a still of the video's file as WebP stands in.
        klipy.thumbnail = None;
        assert_eq!(
            klipy.star().unwrap().gif_src.as_deref(),
            Some("https://images-ext-1.discordapp.net/x.mp4?format=webp")
        );
        klipy.provider = None;
        assert_eq!(klipy.star().unwrap().gif_src, None, "no other provider's");
    }

    fn tenor_embed(provider: &str) -> Embed {
        Embed { provider: Some(EmbedProvider { name: Some(provider.into()), url: None }), ..tenor() }
    }

    #[test]
    fn an_image_is_starred_only_where_it_animates_and_is_filed_under_its_own_link() {
        let image = |url: &str, flags: u64| {
            embed(json!({
                "type": "image", "url": "https://example.test/page",
                "thumbnail": {"url": url, "proxy_url": "https://images-ext-1.discordapp.net/p", "width": 60, "height": 40, "flags": flags}
            }))
        };
        let shown = |embed: Embed| match embed.display(true) {
            Display::Bare(Large::Image(media)) => media.star,
            other => panic!("{other:?}"),
        };
        let star = shown(image("https://example.test/a.gif", 0)).unwrap();
        assert_eq!(star.url, "https://example.test/a.gif", "not the page, which only a GIF link is filed under");
        assert_eq!((star.src.as_str(), star.format), ("https://images-ext-1.discordapp.net/p", GifFormat::Image));
        assert_eq!(shown(image("https://example.test/a.png", 0)), None);
        assert_eq!(shown(image("https://example.test/a.webp", 0)), None, "not flagged");
        assert!(shown(image("https://example.test/a.webp", IS_ANIMATED)).is_some());
    }

    #[test]
    fn the_images_of_a_gallery_are_starred_each_as_itself() {
        let item = |name: &str| {
            json!({"url": format!("https://example.test/{name}.gif"), "proxy_url": format!("https://images-ext-1.discordapp.net/{name}"),
                   "width": 60, "height": 40})
        };
        let first = embed(json!({"type": "image", "url": "https://example.test/post", "image": item("a")}));
        let second = embed(json!({"type": "image", "url": "https://example.test/post", "image": item("b")}));
        let merged = merge(&[first, second]);
        let Display::Bare(Large::Gallery(items)) = merged[0].display(true) else { panic!() };
        let stars: Vec<_> = items.iter().map(|m| m.star.as_ref().map(|s| (s.url.as_str(), s.src.as_str()))).collect();
        assert_eq!(
            stars,
            [
                Some(("https://example.test/a.gif", "https://images-ext-1.discordapp.net/a")),
                Some(("https://example.test/b.gif", "https://images-ext-1.discordapp.net/b")),
            ]
        );
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
    fn conjure_project_links_hide_non_rich_embeds() {
        let message: Message = serde_json::from_value(json!({
            "id": "1", "channel_id": "2", "author": {"id": "3", "username": "a"}, "timestamp": "t",
            "embeds": [
                {"type": "link", "url": "https://conjure.town/12345678901234567/my-project"},
                {"type": "rich", "url": "https://conjure.town/12345678901234567/other-project", "title": "card"},
                {"type": "link", "url": "https://example.com/12345678901234567/my-project"},
            ]
        }))
        .unwrap();
        let shown = visible(&message, &MediaSettings::default());
        assert_eq!(shown.len(), 2, "only the link embed of the project is hidden");
        assert_eq!(shown[0].kind, EmbedKind::Rich);
        assert_eq!(shown[1].url.as_deref(), Some("https://example.com/12345678901234567/my-project"));
    }

    #[test]
    fn conjure_project_link_shapes() {
        let id = "12345678901234567";
        let ticket = "abcdefghijklmnop.abcdefghijklmnop";
        for url in [
            format!("https://conjure.town/{id}/my-project-1"),
            format!("http://CONJURE.city/{id}/p/"),
            format!("https://www.conjure.town/{id}/p/{ticket}"),
            format!("https://user@conjure.town:443/{id}/p/{ticket}/?x=1#y"),
            format!("HTTPS://conjure.town/{}/p", "9".repeat(20)),
        ] {
            assert!(is_conjure_project_link(&url), "{url}");
        }
        for url in [
            format!("https://conjure.town/{id}"),
            format!("https://conjure.town/{id}/"),
            format!("https://conjure.town/{}/p", "1".repeat(16)),
            format!("https://conjure.town/{}/p", "1".repeat(21)),
            format!("https://conjure.town/{id}/P"),
            format!("https://conjure.town/{id}/{}", "a".repeat(49)),
            format!("https://conjure.town/{id}/p/short.short"),
            format!("https://conjure.town/{id}/p/{ticket}/more"),
            format!("https://conjure.town/{id}/p//"),
            format!("https://conjure.town:8080/{id}/p"),
            format!("https://sub.conjure.town/{id}/p"),
            format!("https://conjure.town.evil.com/{id}/p"),
            format!("https://evil.com/conjure.town/{id}/p"),
            format!("https://evil.com/?u=https://conjure.town/{id}/p"),
            format!("ftp://conjure.town/{id}/p"),
            format!("conjure.town/{id}/p"),
        ] {
            assert!(!is_conjure_project_link(&url), "{url}");
        }
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
        assert_eq!(embed(json!({"type": "poll_result"})).kind, EmbedKind::PollResult);
        assert_eq!(embed(json!({"type": "tweet"})).kind, EmbedKind::Other);
        assert_eq!(embed(json!({})).kind, EmbedKind::Rich);
    }
}
