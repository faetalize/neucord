//! The media viewer: which pictures and videos of a message it opens, how
//! big it shows them, and what the menus on media offer.
//!
//! The rules follow Discord's own client: the viewer is the media modal
//! (2323), opened from the media of a message; a message's attachments open
//! together, an embed's images with each other.

use crate::cdn;
use crate::embed::{self, Display, Large, Media};
use crate::model::{Attachment, Message, Snowflake};
use crate::settings::MediaSettings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

/// Where in a message a piece of media is. A message's attachments open
/// together, an embed's images with each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Attachment(Snowflake),
    /// The embed at this place among the message's visible ones.
    Embed(usize),
}

impl Source {
    /// What a source opens with: all the attachments, or one embed's images.
    fn group(self) -> Option<usize> {
        match self {
            Source::Attachment(_) => None,
            Source::Embed(embed) => Some(embed),
        }
    }
}

/// A picture or video the viewer can show.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub source: Source,
    /// Where the file is: the attachment's CDN link, or the page's own for
    /// an embed's. Links point there; embeds' are never loaded.
    pub url: String,
    /// The same through the media proxy, which resizes images.
    pub proxy_url: Option<String>,
    pub size: Option<(u32, u32)>,
    /// A GIF, or an animated WebP or AVIF.
    pub animated: bool,
    pub content_type: Option<String>,
    pub filename: Option<String>,
    /// In bytes.
    pub bytes: Option<u64>,
}

const IMAGE_EXTENSIONS: [&str; 10] = ["png", "jpg", "jpeg", "jfif", "webp", "gif", "heic", "heif", "dng", "avif"];
const VIDEO_EXTENSIONS: [&str; 4] = ["mp4", "webm", "mov", "qt"];
/// What the client's Save Image writes (`canSaveImage` in 253400).
const SAVED: [&str; 9] = ["jpg", "jpeg", "jfif", "png", "webp", "gif", "tiff", "bmp", "avif"];
/// What its Copy Image copies (`canCopyImage`).
const COPIED: [&str; 6] = ["jpg", "jpeg", "jfif", "png", "webp", "avif"];

impl Item {
    fn attachment(attachment: &Attachment) -> Option<Item> {
        let extension = attachment.filename.rsplit_once('.')?.1.to_ascii_lowercase();
        let kind = if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            Kind::Image
        } else if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
            Kind::Video
        } else {
            return None;
        };
        let size = attachment.width.zip(attachment.height).filter(|&(w, h)| w > 0 && h > 0);
        // The client shows a video only with its size and the proxy's file.
        if kind == Kind::Video && (size.is_none() || attachment.proxy_url.is_none()) {
            return None;
        }
        Some(Item {
            kind,
            source: Source::Attachment(attachment.id),
            url: attachment.url.clone(),
            proxy_url: attachment.proxy_url.clone(),
            size,
            animated: attachment.is_animated(),
            content_type: attachment.content_type.clone(),
            filename: Some(attachment.filename.clone()),
            bytes: attachment.size,
        })
    }

    fn embedded(media: &Media, embed: usize, kind: Kind) -> Item {
        Item {
            kind,
            source: Source::Embed(embed),
            url: media.original.clone(),
            proxy_url: Some(media.url.clone()),
            size: Some((media.width, media.height)),
            animated: media.animated,
            content_type: None,
            filename: None,
            bytes: None,
        }
    }

    /// What the viewer loads: the proxy's file when there is one.
    pub fn file_url(&self) -> &str {
        self.proxy_url.as_deref().filter(|u| !u.is_empty()).unwrap_or(&self.url)
    }

    /// The link Open in Browser and Copy Link give (`XW` in 803316).
    pub fn link(&self) -> String {
        cdn::original(&self.url, self.content_type.as_deref(), None)
    }

    /// The file to fetch for Save and Copy Image: the original, from a
    /// Discord host or not at all.
    pub fn download_url(&self) -> Option<String> {
        let url = cdn::original(&self.url, self.content_type.as_deref(), Some("png"));
        // An embed's page is not Discord's: its picture comes from the
        // proxy, which has it at full size.
        if cdn::is_asset(&url) {
            Some(url)
        } else {
            let proxy = cdn::original(self.proxy_url.as_deref()?, self.content_type.as_deref(), Some("png"));
            cdn::is_asset(&proxy).then_some(proxy)
        }
    }

    /// What the zoomed picture loads: the original on Discord's CDN, else
    /// what is shown already, which the proxy has at its size.
    pub fn zoom_url(&self) -> &str {
        if cdn::is_asset(&self.url) { &self.url } else { self.file_url() }
    }

    /// Save Image is offered (`e7` in 803316).
    pub fn can_save(&self) -> bool {
        self.kind == Kind::Image
            && can_save_image(&self.url, self.content_type.as_deref())
            && self.download_url().is_some()
    }

    /// Copy Image is offered (`PK` in 803316).
    pub fn can_copy(&self) -> bool {
        self.kind == Kind::Image
            && can_copy_image(&self.url, self.content_type.as_deref())
            && self.download_url().is_some()
    }

    /// Copy Link and Open in Browser are offered (`fW`).
    pub fn can_link(&self) -> bool {
        link_allowed(&self.url)
    }

    /// A name for the file Save writes.
    pub fn save_name(&self) -> String {
        save_name(&self.download_url().unwrap_or_else(|| self.url.clone()), self.content_type.as_deref())
    }
}

/// All the media of `message` the viewer opens: its attachments (except the
/// thumbnails of others), then each visible embed's large images, and its video.
pub fn items(message: &Message, settings: &MediaSettings) -> Vec<Item> {
    let mut items = Vec::new();
    // A poll's attachments are its answers' pictures, and a message of
    // components shows its own.
    if message.poll.is_none() && !message.is_components_v2() {
        items.extend(message.attachments.iter().filter(|a| !a.is_hidden()).filter_map(Item::attachment));
    }
    for (n, embed) in embed::visible(message, settings).iter().enumerate() {
        match embed.display(settings.inline_embed_media) {
            Display::Bare(large) | Display::Card { media: Some(large), .. } => match large {
                Large::Image(media) => items.push(Item::embedded(&media, n, Kind::Image)),
                Large::Gallery(media) => items.extend(media.iter().map(|media| Item::embedded(media, n, Kind::Image))),
                // It plays in the embed; the viewer is its fullscreen.
                Large::Video { video: Some(video), .. } => items.push(Item::embedded(&video, n, Kind::Video)),
                Large::Video { video: None, .. } | Large::Gifv { .. } => {}
            },
            Display::Card { media: None, .. } => {}
        }
    }
    items
}

/// What opens with `items[at]`: the media of its source, and where it is among them.
pub fn gallery(items: &[Item], at: usize) -> (Vec<Item>, usize) {
    let group = items[at].source.group();
    let group: Vec<usize> = (0..items.len()).filter(|&n| items[n].source.group() == group).collect();
    let place = group.iter().position(|&n| n == at).unwrap_or(0);
    (group.into_iter().map(|n| items[n].clone()).collect(), place)
}

/// The room the viewer gives a picture of `size` in a window of `window`
/// (`A` in 967758): the larger of two fits, one with room for the arrows and
/// the strip of thumbnails, one that leaves the sides free. Without a size, all of the first.
pub fn viewer_box(window: (i32, i32), several: bool, size: Option<(u32, u32)>) -> (u32, u32) {
    let side = |n: i32| n.max(1) as u32;
    let (width, height) = window;
    let tight = (side(width - if several { 76 } else { 24 } * 2), side(height - 176));
    let Some(size) = size else { return tight };
    let first = cdn::fit(size, tight);
    let second = cdn::fit(size, (side(width - 544), side(height - if several { 88 } else { 36 } * 2)));
    if first.0 >= second.0 { first } else { second }
}

/// Whether a file is a type the client's Save Image writes (`canSaveImage`):
/// one it knows, or none it can tell.
pub fn can_save_image(url: &str, content_type: Option<&str>) -> bool {
    cdn::extension(url, content_type).is_none_or(|e| SAVED.contains(&e.as_str()))
}

/// Whether the client's Copy Image copies it (`canCopyImage`).
pub fn can_copy_image(url: &str, content_type: Option<&str>) -> bool {
    cdn::extension(url, content_type).is_none_or(|e| COPIED.contains(&e.as_str()))
}

/// Every link can be copied and opened but role icons' (`u` in 803316).
pub fn link_allowed(url: &str) -> bool {
    !url.starts_with("https://cdn.discordapp.com/role-icons") && !(url.contains("/roles/") && url.contains("/icons/"))
}

/// What a link menu calls its link (777933).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// A picture of the CDN.
    Image,
    /// A picture or video of anywhere.
    Media,
    Other,
}

pub fn link_kind(url: &str) -> LinkKind {
    let Some((host, path)) = url.strip_prefix("https://").map(|rest| {
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        let (head, _) = rest.split_at(end);
        head.split_once('/').map_or((head, ""), |(host, path)| (host, path))
    }) else {
        return LinkKind::Other;
    };
    let extension = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let image = ["png", "jpg", "jpeg", "webp", "avif", "bmp", "svg"].contains(&extension.as_str());
    if host == "cdn.discordapp.com" && image {
        LinkKind::Image
    } else if image || ["gif", "mp4", "webm", "mov"].contains(&extension.as_str()) {
        LinkKind::Media
    } else {
        LinkKind::Other
    }
}

/// The name Save Image proposes for the file at `url` (`saveImage` in
/// 253400): its last path segment, cleaned of what file names can't hold,
/// with the extension the `format` says, or that of its type if it has none.
pub fn save_name(url: &str, content_type: Option<&str>) -> String {
    let (path, query) = url.split_once('#').map_or(url, |(url, _)| url).split_once('?').unwrap_or((url, ""));
    let segment = path.rsplit('/').next().unwrap_or_default();
    let mut name = unescape(segment);
    // `name.png:large` and `name@png`, as Twitter-style links have them.
    if let Some(at) = name.rfind(':').filter(|&at| {
        let before = &name[..at];
        before.rsplit_once('.').is_some_and(|(_, e)| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
            && !name[at + 1..].contains('.')
    }) {
        name.truncate(at);
    }
    if let Some((stem, tag)) = name
        .rsplit_once('@')
        .filter(|(stem, tag)| !stem.is_empty() && !tag.is_empty() && tag.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        name = format!("{stem}.{tag}");
    }
    let mut name: String = name.chars().map(|c| if "<>:\"/\\|?*@".contains(c) { '_' } else { c }).collect();
    let format = query.split('&').find_map(|p| p.strip_prefix("format="));
    if let Some(format) = format {
        let extension: String = format.chars().filter(char::is_ascii_alphanumeric).collect();
        if !extension.is_empty() {
            if let Some(dot) = name.rfind('.') {
                name.truncate(dot);
            }
            name = format!("{name}.{}", extension.to_ascii_lowercase());
        }
    } else if !name.contains('.') {
        let extension = cdn::extension(url, content_type).unwrap_or_else(|| "png".to_owned());
        name = format!("{name}.{extension}");
    }
    name
}

/// `%`-escapes undone, or the text itself where they are not valid.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) =
                (bytes.get(at + 1).and_then(|&b| hex(b)), bytes.get(at + 2).and_then(|&b| hex(b)))
        {
            out.push((high * 16 + low) as u8);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// The volume and mute a player starts with, and keeps for the next
/// (`MediaPlayerVolume` in 587481).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    pub volume: f64,
    pub muted: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Playback { volume: 1.0, muted: false }
    }
}

impl Playback {
    /// After the volume is dragged to `volume`: zero mutes, and anything
    /// above it unmutes (`handleDrag` in 153440).
    pub fn dragged_to(self, volume: f64) -> Playback {
        let volume = volume.clamp(0.0, 1.0);
        Playback { volume, muted: volume == 0.0 }
    }
}

/// `m:ss`, or `h:mm:ss` from an hour on.
pub fn clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() { seconds.max(0.0) as u64 } else { 0 };
    match seconds {
        s if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        s => format!("{}:{:02}", s / 60, s % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(json: serde_json::Value) -> Message {
        let mut message = serde_json::json!({
            "id": "1", "channel_id": "2", "author": {"id": "3", "username": "a"}, "timestamp": "2026-01-01T00:00:00+00:00",
        });
        message.as_object_mut().unwrap().extend(json.as_object().unwrap().clone());
        serde_json::from_value(message).unwrap()
    }

    fn attachment(id: &str, name: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut attachment = serde_json::json!({
            "id": id, "filename": name,
            "url": format!("https://cdn.discordapp.com/attachments/1/{id}/{name}?ex=1"),
            "proxy_url": format!("https://media.discordapp.net/attachments/1/{id}/{name}?ex=1"),
            "width": 800, "height": 600,
        });
        attachment.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        attachment
    }

    #[test]
    fn a_message_opens_its_attachments_together_and_each_embed_apart() {
        let message = message(serde_json::json!({
            "attachments": [
                attachment("10", "a.png", serde_json::json!({})),
                attachment("11", "notes.txt", serde_json::json!({})),
                attachment("12", "b.MP4", serde_json::json!({})),
                attachment("13", "c.gif", serde_json::json!({})),
            ],
            "embeds": [
                {"type": "image", "url": "https://e.test/a.png",
                 "thumbnail": {"url": "https://e.test/a.png", "proxy_url": "https://images-ext-1.discordapp.net/external/x/https/e.test/a.png", "width": 8, "height": 8}},
                {"type": "image", "url": "https://e.test/b.png",
                 "thumbnail": {"url": "https://e.test/b.png", "proxy_url": "https://images-ext-1.discordapp.net/external/y/https/e.test/b.png", "width": 8, "height": 8}},
            ],
        }));
        let items = items(&message, &MediaSettings::default());
        let sources: Vec<_> = items.iter().map(|i| (i.kind, i.source)).collect();
        assert_eq!(
            sources,
            [
                (Kind::Image, Source::Attachment(Snowflake(10))),
                (Kind::Video, Source::Attachment(Snowflake(12))),
                (Kind::Image, Source::Attachment(Snowflake(13))),
                (Kind::Image, Source::Embed(0)),
                (Kind::Image, Source::Embed(1)),
            ]
        );
        let (group, place) = gallery(&items, 2);
        assert_eq!((group.len(), place), (3, 2), "the attachments, the third among them");
        assert_eq!(gallery(&items, 3), (vec![items[3].clone()], 0));
        // The link is the page's; the file, the proxy's.
        assert_eq!(items[3].link(), "https://e.test/a.png");
        assert!(items[3].file_url().starts_with("https://images-ext-1.discordapp.net/"));
    }

    #[test]
    fn a_video_without_a_size_or_the_proxys_file_is_left_out() {
        let message = message(serde_json::json!({"attachments": [
            attachment("10", "a.mp4", serde_json::json!({"width": null})),
            attachment("11", "b.mp4", serde_json::json!({"proxy_url": null})),
            attachment("12", "c.mp4", serde_json::json!({})),
        ]}));
        let ids: Vec<_> = items(&message, &MediaSettings::default()).into_iter().map(|i| i.source).collect();
        assert_eq!(ids, [Source::Attachment(Snowflake(12))]);
    }

    #[test]
    fn a_polls_attachments_and_thumbnails_are_not_the_messages_media() {
        let thumbnail = attachment("11", "t.png", serde_json::json!({"flags": 2}));
        let message = message(serde_json::json!({"attachments": [thumbnail]}));
        assert!(items(&message, &MediaSettings::default()).is_empty());
    }

    #[test]
    fn the_viewer_fits_the_larger_of_two_boxes() {
        // 1600x1000 window: 1552x824 with room for the sides' arrows, or
        // 1056x928 without.
        assert_eq!(viewer_box((1600, 1000), true, Some((4000, 2000))), (1448, 724));
        assert_eq!(viewer_box((1600, 1000), false, Some((4000, 2000))), (1552, 776));
        // A tall one fits the second box's height better.
        assert_eq!(viewer_box((1600, 1000), true, Some((1000, 4000))), (206, 824));
        assert_eq!(viewer_box((1600, 1000), false, None), (1552, 824));
        // Never above its own size.
        assert_eq!(viewer_box((1600, 1000), false, Some((300, 200))), (300, 200));
        // A window too small for the margins leaves a pixel, not nothing.
        assert_eq!(viewer_box((100, 100), true, None), (1, 1));
    }

    #[test]
    fn save_and_copy_follow_the_clients_types() {
        assert!(can_save_image("https://cdn.discordapp.com/a/b.gif", None));
        assert!(!can_copy_image("https://cdn.discordapp.com/a/b.gif", None), "GIFs save but aren't copied");
        assert!(can_copy_image("https://cdn.discordapp.com/a/b.webp", None));
        assert!(!can_save_image("https://cdn.discordapp.com/a/b.mp4", Some("video/mp4")));
        assert!(can_save_image("https://cdn.discordapp.com/attachments/1/2/b", None), "no type to tell by");
        assert!(!can_save_image("https://cdn.discordapp.com/a/b.png", Some("video/mp4")), "the type wins");
    }

    #[test]
    fn an_embeds_picture_is_saved_from_the_proxy_never_its_site() {
        let message = message(serde_json::json!({"embeds": [
            {"type": "image", "url": "https://e.test/a.png",
             "thumbnail": {"url": "https://e.test/a.png", "proxy_url": "https://images-ext-1.discordapp.net/external/x/https/e.test/a.png?width=8&height=8", "width": 8, "height": 8}},
        ]}));
        let item = &items(&message, &MediaSettings::default())[0];
        assert_eq!(
            item.download_url().as_deref(),
            Some("https://images-ext-1.discordapp.net/external/x/https/e.test/a.png")
        );
        assert_eq!(item.zoom_url(), item.file_url());
        assert!(item.can_save() && item.can_copy());
    }

    #[test]
    fn links_are_named_for_what_they_point_to() {
        assert_eq!(link_kind("https://cdn.discordapp.com/attachments/1/2/a.PNG?ex=1"), LinkKind::Image);
        assert_eq!(link_kind("https://cdn.discordapp.com/attachments/1/2/a.gif"), LinkKind::Media);
        assert_eq!(link_kind("https://e.test/a.webp"), LinkKind::Media);
        assert_eq!(link_kind("https://e.test/a.mp4#t=1"), LinkKind::Media);
        assert_eq!(link_kind("https://e.test/a.html"), LinkKind::Other);
        assert_eq!(link_kind("https://e.test/png"), LinkKind::Other);
    }

    #[test]
    fn saved_files_are_named_as_the_client_names_them() {
        let name = |url, content_type| save_name(url, content_type);
        assert_eq!(name("https://cdn.discordapp.com/a/1/My%20Cat.png?ex=1", None), "My Cat.png");
        assert_eq!(name("https://e.test/a/b.jpg:large", None), "b.jpg");
        assert_eq!(name("https://e.test/a/photo@png", None), "photo.png");
        assert_eq!(name("https://e.test/a/we<ir>d.png", None), "we_ir_d.png");
        // A format asked of the proxy replaces the extension; none asked, and no extension, takes the type's.
        assert_eq!(name("https://e.test/a/b.webp?format=png", None), "b.png");
        assert_eq!(name("https://e.test/a/image", Some("image/jpeg")), "image.jpg");
        assert_eq!(name("https://e.test/a/image", None), "image.png");
    }

    #[test]
    fn dragging_the_volume_to_nothing_mutes_and_back_unmutes() {
        let start = Playback::default();
        assert_eq!(start, Playback { volume: 1.0, muted: false });
        assert_eq!(start.dragged_to(0.0), Playback { volume: 0.0, muted: true });
        assert_eq!(start.dragged_to(0.0).dragged_to(0.4), Playback { volume: 0.4, muted: false });
        assert!(!Playback { volume: 0.5, muted: true }.dragged_to(0.5).muted);
        assert_eq!(start.dragged_to(7.0).volume, 1.0);
    }

    #[test]
    fn a_clock_counts_hours_only_when_there_are_some() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(75.9), "1:15");
        assert_eq!(clock(3600.0), "1:00:00");
        assert_eq!(clock(f64::NAN), "0:00");
    }
}
