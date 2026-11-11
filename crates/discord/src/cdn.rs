//! Build Discord CDN URLs for avatars, guild icons and custom emoji, and
//! sized requests to the media proxy.
//!
//! We always request an explicit extension so the bytes are a format
//! gdk-pixbuf decodes (PNG for static, GIF for animated): `a_`-prefixed hashes
//! are animated. `size` is a power of two in 16..=4096, or another size the
//! client asks for ([`asset_size`]).

const CDN: &str = "https://cdn.discordapp.com";
const CDN_HOST: &str = "cdn.discordapp.com";

fn ext(hash: &str) -> &'static str {
    if hash.starts_with("a_") { "gif" } else { "png" }
}

/// True when the asset hash denotes an animated (GIF) image.
pub fn is_animated(hash: &str) -> bool {
    hash.starts_with("a_")
}

pub fn avatar_url(user_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/avatars/{user_id}/{hash}.{}?size={size}", ext(hash))
}

/// A picture someone set for one server.
pub fn member_avatar_url(guild_id: u64, user_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/guilds/{guild_id}/users/{user_id}/avatars/{hash}.{}?size={size}", ext(hash))
}

/// A profile banner, still even where animated: the client animates one
/// only for a pointer on it.
pub fn banner_url(user_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/banners/{user_id}/{hash}.png?size={size}")
}

pub fn member_banner_url(guild_id: u64, user_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/guilds/{guild_id}/users/{user_id}/banners/{hash}.png?size={size}")
}

/// An avatar decoration of the collectibles shop, as a still image.
pub fn decoration_url(sku: &str) -> String {
    format!("{CDN}/media/v1/collectibles-shop/{sku}/static")
}

/// One of the older preset decorations, as a still image.
pub fn decoration_preset_url(asset: &str, size: u16) -> String {
    format!("{CDN}/avatar-decoration-presets/{asset}.png?size={size}&passthrough=false")
}

/// The sizes the client asks the CDN for (the list `kr` in 776231 picks
/// from, anchored by `3072, 4096`).
const SIZES: [u16; 31] = [
    16, 20, 22, 24, 28, 32, 40, 44, 48, 56, 60, 64, 80, 96, 100, 128, 160, 240, 256, 300, 320, 480, 512, 600, 640,
    1024, 1280, 1536, 2048, 3072, 4096,
];

/// The size to ask for an image shown `px` pixels wide: the smallest the
/// client asks for that holds it, or the largest if none does.
pub fn asset_size(px: u32) -> u16 {
    SIZES.iter().copied().find(|&size| u32::from(size) >= px).unwrap_or(SIZES[SIZES.len() - 1])
}

pub fn badge_icon_url(icon: &str) -> String {
    format!("{CDN}/badge-icons/{icon}.png")
}

pub fn guild_icon_url(guild_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/icons/{guild_id}/{hash}.{}?size={size}", ext(hash))
}

/// A guild icon as a still PNG, even for animated icons (their first frame).
pub fn guild_icon_still_url(guild_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/icons/{guild_id}/{hash}.png?size={size}")
}

pub fn group_dm_icon_url(channel_id: u64, hash: &str, size: u16) -> String {
    format!("{CDN}/channel-icons/{channel_id}/{hash}.png?size={size}")
}

pub fn emoji_url(emoji_id: u64, animated: bool, size: u16) -> String {
    let ext = if animated { "gif" } else { "png" };
    format!("{CDN}/emojis/{emoji_id}.{ext}?size={size}")
}

/// Ask Discord's media proxy for an image scaled down to `width`×`height`,
/// so it downloads and decodes at the size it is shown. Only the proxy
/// resizes, so any other URL comes back unchanged.
pub fn proxy_sized(url: &str, width: u32, height: u32) -> String {
    let host = url.strip_prefix("https://").and_then(|rest| rest.split(['/', '?']).next()).unwrap_or_default();
    let proxied =
        host == "media.discordapp.net" || (host.starts_with("images-ext-") && host.ends_with(".discordapp.net"));
    if !proxied {
        return url.to_owned();
    }
    with_params(url, &[("width", &width.to_string()), ("height", &height.to_string())])
}

/// An image of Discord's media proxy asked to stay animated, as the GIF
/// picker's previews are (`F` in 884037): a WebP or AVIF of the proxy's
/// `/external/` or of an attachment says `animated=true`, and a GIF there
/// is asked for as an animated WebP. Any other URL comes back unchanged.
pub fn animated(url: &str) -> String {
    let Some(parsed) = parse(url) else { return url.to_owned() };
    let proxy = parsed.host.starts_with("images-ext-") && parsed.host.ends_with(".discordapp.net");
    let external = proxy && parsed.path.starts_with("/external/");
    let attachment = is_cdn_host(&parsed.host)
        && ["/attachments/", "/ephemeral-attachments/"].iter().any(|prefix| parsed.path.starts_with(prefix));
    let path = parsed.path.to_ascii_lowercase();
    if !(external || attachment) {
        return url.to_owned();
    }
    if path.ends_with(".webp") || path.ends_with(".avif") {
        with_params(url, &[("animated", "true")])
    } else if path.ends_with(".gif") {
        with_params(url, &[("format", "webp"), ("animated", "true")])
    } else {
        url.to_owned()
    }
}

/// Set query parameters on a URL, replacing any it already has by those names.
pub fn with_params(url: &str, set: &[(&str, &str)]) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut params: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty() && !set.iter().any(|(name, _)| p.split('=').next() == Some(*name)))
        .map(String::from)
        .collect();
    params.extend(set.iter().map(|(name, value)| format!("{name}={value}")));
    format!("{base}?{}", params.join("&"))
}

/// Whether a link is the CDN's or the media proxy's own (`isDiscordAssetUrl`
/// in 998218). The client counts the GIF providers' hosts too; Neucord
/// never fetches from those, so it doesn't.
pub fn is_asset(url: &str) -> bool {
    parse(url).is_some_and(|url| url.host == CDN_HOST || url.host.ends_with(".discordapp.net"))
}

/// A file's type as the client names it (`U` in 229531): by the content
/// type if it has one of an image or video, else the last bit of the path
/// if that is short enough to be an extension.
pub fn extension(url: &str, content_type: Option<&str>) -> Option<String> {
    let kind = content_type.map(str::to_ascii_lowercase);
    if let Some(kind) = kind.as_deref().filter(|k| k.starts_with("image/") || k.starts_with("video/")) {
        return Some(match kind {
            "image/jpeg" => "jpg".to_owned(),
            "video/quicktime" => "mov".to_owned(),
            other => other.split('/').nth(1).unwrap_or_default().to_owned(),
        });
    }
    let path = parse(url)?.path;
    // `pathname.split(".").pop()`: with no dot, the whole path.
    let last = path.rsplit('.').next().unwrap_or(path);
    (last.len() <= 5).then(|| last.to_ascii_lowercase())
}

/// A link to the file itself, as Open in Browser and the copies take it
/// (`XW` in 803316): the proxy's `media.discordapp.net` is the CDN's
/// `cdn.discordapp.com`, and the size and format asked of the proxy go.
/// A file of no known type gets `format` instead, if one is given. The
/// CDN's own links stay as they are.
pub fn original(url: &str, content_type: Option<&str>, format: Option<&str>) -> String {
    let Some(parsed) = parse(url) else { return url.to_owned() };
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_owned() };
    if parsed.host == CDN_HOST {
        return url.to_owned();
    }
    let (rest, fragment) = rest.split_once('#').map_or((rest, ""), |(rest, fragment)| (rest, fragment));
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    let authority = if scheme == "https" && authority == "media.discordapp.net" { CDN_HOST } else { authority };
    let mut params: Vec<String> = query
        .split('&')
        .filter(|param| {
            let name = param.split('=').next().unwrap_or_default();
            !param.is_empty() && !["width", "height", "quality", "size", "format"].contains(&name)
        })
        .map(String::from)
        .collect();
    if let Some(format) = format.filter(|_| extension(url, content_type).is_none()) {
        params.push(format!("format={format}"));
    }
    let query = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
    let fragment = if fragment.is_empty() { String::new() } else { format!("#{fragment}") };
    format!("{scheme}://{authority}{path}{query}{fragment}")
}

/// Fit `size` into `max` keeping its aspect ratio, without enlarging (`Uj`
/// in 515718): rounded after each of the two cuts, as the client does.
pub fn fit(size: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let ((mut width, mut height), (max_width, max_height)) = ((f64::from(size.0), f64::from(size.1)), max);
    let (max_width, max_height) = (f64::from(max_width), f64::from(max_height));
    if width != max_width || height != max_height {
        let by_width = if width > max_width { max_width / width } else { 1.0 };
        width = (width * by_width).round();
        height = (height * by_width).round();
        let by_height = if height > max_height { max_height / height } else { 1.0 };
        width = (width * by_height).round();
        height = (height * by_height).round();
    }
    (width as u32, height as u32)
}

/// How much of its size `size` keeps in `max` (`U8` in 515718).
fn ratio(size: (u32, u32), max: (u32, u32)) -> f64 {
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    let by_width = if width > f64::from(max.0) { f64::from(max.0) / width } else { 1.0 };
    let height = (height * by_width).round();
    let by_height = if height > f64::from(max.1) { f64::from(max.1) / height } else { 1.0 };
    (by_width * by_height).min(1.0)
}

/// The largest side the client asks the proxy for.
const MAX_SIDE: u32 = 4096;

/// An image of the media viewer as the client asks for it (`AE` in 776231,
/// with the ratio `getRatio` in 619517 gives): the proxy's version of
/// `size` scaled down to what fits `fit`, at the screen's `scale`, up to
/// 4096 a side, and only if that is not its size already. The CDN's own
/// links are never sized, nor is anything else but the proxy's.
pub fn modal_url(src: &str, size: (u32, u32), fit_in: (u32, u32), scale: u32, animated: bool) -> String {
    if parse(src).is_none_or(|url| url.host == CDN_HOST) {
        return src.to_owned();
    }
    let ratio = ratio(size, fit_in);
    let mut target = size;
    if ratio < 1.0 {
        target = ((f64::from(size.0) * ratio).round() as u32, (f64::from(size.1) * ratio).round() as u32);
    }
    let scale = scale.max(1);
    let target = fit((target.0 * scale, target.1 * scale), (MAX_SIDE, MAX_SIDE));
    let mut url = src.to_owned();
    // An animated WebP or AVIF is asked to stay animated.
    let path = parse(src).map(|url| url.path.to_ascii_lowercase()).unwrap_or_default();
    if animated && (path.ends_with(".webp") || path.ends_with(".avif")) {
        url = with_params(&url, &[("animated", "true")]);
    }
    if target != size {
        url = with_params(&url, &[("width", &target.0.to_string()), ("height", &target.1.to_string())]);
    }
    url
}

/// How long before its `ex` a signed link counts as expired.
const EXPIRY_MARGIN_MS: u64 = 60 * 60 * 1000;

/// The parts of an absolute URL that expiry looks at.
struct Url<'a> {
    host: String,
    path: &'a str,
    query: &'a str,
}

fn parse(url: &str) -> Option<Url<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let rest = rest.split('#').next()?;
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = rest.find('/').map_or((rest, "/"), |at| rest.split_at(at));
    let host = authority.rsplit('@').next()?.split(':').next()?.to_ascii_lowercase();
    Some(Url { host, path, query })
}

/// A host of Discord's CDN or media proxy: the host itself, a subdomain of
/// it, or a sibling that takes the first label as a prefix (`cdn-1`).
fn is_cdn_host(host: &str) -> bool {
    ["cdn.discordapp.com", "media.discordapp.net"].iter().any(|base| {
        if host == *base || host.strip_suffix(base).is_some_and(|rest| rest.ends_with('.')) {
            return true;
        }
        let (Some((base_label, base_rest)), Some((label, rest))) = (base.split_once('.'), host.split_once('.')) else {
            return false;
        };
        rest == base_rest && label.strip_prefix(base_label).is_some_and(|dash| dash.len() > 1 && dash.starts_with('-'))
    })
}

/// A signed link's `ex`, in seconds since the epoch, read as `parseInt(.., 16)`
/// does: up to the first character that isn't a digit.
fn expires(query: &str) -> Option<u64> {
    let value = query.split('&').find_map(|param| param.strip_prefix("ex="))?;
    let digits = value.len() - value.trim_start_matches(|c: char| c.is_ascii_hexdigit()).len();
    u64::from_str_radix(&value[..digits], 16).ok()
}

fn ends_within_margin(url: &Url, now_ms: u64) -> bool {
    expires(url.query).is_none_or(|ex| ex.saturating_mul(1000) <= now_ms + EXPIRY_MARGIN_MS)
}

fn attachment_expired(url: &str, now_ms: u64) -> bool {
    parse(url).is_some_and(|url| ends_within_margin(&url, now_ms))
}

/// Media of the CDN that is signed, or is an attachment and so should be.
fn media_expired(media: Option<&crate::embed::EmbedMedia>, now_ms: u64) -> bool {
    let Some(url) = media.and_then(|media| media.url.as_deref()).and_then(parse) else { return false };
    let signed = url.query.split('&').any(|param| param.starts_with("ex="));
    let attachment = ["/attachments/", "/ephemeral-attachments/"].iter().any(|prefix| url.path.starts_with(prefix));
    is_cdn_host(&url.host)
        && !url.path.starts_with("/external/")
        && (signed || attachment)
        && ends_within_margin(&url, now_ms)
}

/// Whether the client throws away a channel holding this message instead of
/// showing it: a link in it has expired, or is within an hour of it
/// (`$r` in 679164). Attachments are judged by their signature alone, embeds
/// by it only on Discord's own hosts. Embeds sharing a `url` are one in the
/// client, which keeps the first one's video and every one's image.
pub fn links_expired(message: &crate::Message, now_ms: u64) -> bool {
    if message.attachments.iter().any(|attachment| attachment_expired(&attachment.url, now_ms)) {
        return true;
    }
    let mut seen = Vec::new();
    message.embeds.iter().any(|embed| {
        let first = embed.url.as_ref().is_none_or(|url| {
            let first = !seen.contains(&url);
            seen.push(url);
            first
        });
        let (image, video) = embed.client_media();
        media_expired(image, now_ms) || (first && media_expired(video, now_ms))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_of_the_media_proxy_are_asked_to_animate() {
        let external = "https://images-ext-1.discordapp.net/external/x/https/example.test/a";
        assert_eq!(animated(&format!("{external}.webp")), format!("{external}.webp?animated=true"));
        assert_eq!(animated(&format!("{external}.gif?a=b")), format!("{external}.gif?a=b&format=webp&animated=true"));
        let attachment = "https://cdn.discordapp.com/attachments/1/2/a.avif?ex=1";
        assert_eq!(animated(attachment), format!("{attachment}&animated=true"));
        // Neither a still, nor another host, nor another path of the proxy.
        assert_eq!(
            animated("https://images-ext-1.discordapp.net/external/x/a.png"),
            "https://images-ext-1.discordapp.net/external/x/a.png"
        );
        assert_eq!(animated("https://static.example.test/a.webp"), "https://static.example.test/a.webp");
        assert_eq!(
            animated("https://cdn.discordapp.com/avatars/1/a.webp"),
            "https://cdn.discordapp.com/avatars/1/a.webp"
        );
    }

    #[test]
    fn static_avatar() {
        assert_eq!(avatar_url(5, "abcdef", 64), "https://cdn.discordapp.com/avatars/5/abcdef.png?size=64");
    }

    #[test]
    fn animated_avatar_uses_gif() {
        assert!(avatar_url(5, "a_1234", 64).ends_with("a_1234.gif?size=64"));
        assert!(is_animated("a_1234"));
        assert!(!is_animated("plain"));
    }

    #[test]
    fn profile_pictures() {
        assert_eq!(
            member_avatar_url(9, 5, "a_x", 64),
            "https://cdn.discordapp.com/guilds/9/users/5/avatars/a_x.gif?size=64"
        );
        assert_eq!(banner_url(5, "a_b", 600), "https://cdn.discordapp.com/banners/5/a_b.png?size=600");
        assert_eq!(
            member_banner_url(9, 5, "b", 600),
            "https://cdn.discordapp.com/guilds/9/users/5/banners/b.png?size=600"
        );
        assert_eq!(decoration_url("5"), "https://cdn.discordapp.com/media/v1/collectibles-shop/5/static");
        assert_eq!(
            decoration_preset_url("a_x", 128),
            "https://cdn.discordapp.com/avatar-decoration-presets/a_x.png?size=128&passthrough=false"
        );
        assert_eq!(badge_icon_url("i"), "https://cdn.discordapp.com/badge-icons/i.png");
    }

    #[test]
    fn asset_size_is_the_clients_next_size_up() {
        // The popout's banner is 300 wide, the panel's 340, on a screen of scale 1 and 2.
        assert_eq!([300, 340, 600, 680].map(asset_size), [300, 480, 600, 1024]);
        assert_eq!([1, 16, 17].map(asset_size), [16, 16, 20]);
        assert_eq!(asset_size(5000), 4096);
    }

    #[test]
    fn icon_and_emoji() {
        assert_eq!(guild_icon_url(9, "ic", 48), "https://cdn.discordapp.com/icons/9/ic.png?size=48");
        assert_eq!(guild_icon_still_url(9, "a_ic", 32), "https://cdn.discordapp.com/icons/9/a_ic.png?size=32");
        assert_eq!(emoji_url(42, true, 48), "https://cdn.discordapp.com/emojis/42.gif?size=48");
    }

    #[test]
    fn proxy_requests_carry_the_shown_size() {
        assert_eq!(
            proxy_sized("https://images-ext-1.discordapp.net/external/abc/https/e.com/a.png", 400, 300),
            "https://images-ext-1.discordapp.net/external/abc/https/e.com/a.png?width=400&height=300"
        );
        // Signed attachment URLs keep their signature; an old size is replaced.
        assert_eq!(
            proxy_sized("https://media.discordapp.net/attachments/1/2/a.gif?ex=1&hm=2&width=9", 80, 60),
            "https://media.discordapp.net/attachments/1/2/a.gif?ex=1&hm=2&width=80&height=60"
        );
        assert_eq!(
            proxy_sized("https://cdn.discordapp.com/avatars/5/a.png?size=64", 1, 1),
            "https://cdn.discordapp.com/avatars/5/a.png?size=64"
        );
        assert_eq!(
            proxy_sized("https://evil.com/images-ext-1.discordapp.net", 1, 1),
            "https://evil.com/images-ext-1.discordapp.net"
        );
    }

    fn message(attachments: &[&str], embeds: serde_json::Value) -> crate::Message {
        let attachments: Vec<_> = attachments
            .iter()
            .enumerate()
            .map(|(id, url)| serde_json::json!({"id": id.to_string(), "filename": "a.png", "url": url}))
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": "9", "channel_id": "1", "author": {"id": "5", "username": "a"}, "content": "",
            "timestamp": "2026-10-26T00:00:00+00:00", "attachments": attachments, "embeds": embeds
        }))
        .unwrap()
    }

    // 0x10000 seconds, which is 65536000 ms, and the hour the client allows.
    const NOW: u64 = 65_536_000 - EXPIRY_MARGIN_MS - 1;

    #[test]
    fn an_attachment_link_expires_an_hour_early() {
        let live =
            message(&["https://cdn.discordapp.com/attachments/1/2/a.png?ex=10000&is=1&hm=2"], serde_json::json!([]));
        assert!(!links_expired(&live, NOW));
        assert!(links_expired(&live, NOW + 1), "within the hour of its `ex`");
    }

    #[test]
    fn an_attachment_without_a_usable_signature_has_expired() {
        let url = |url| message(&[url], serde_json::json!([]));
        assert!(links_expired(&url("https://cdn.discordapp.com/attachments/1/2/a.png"), 0), "no `ex`");
        assert!(links_expired(&url("https://cdn.discordapp.com/attachments/1/2/a.png?ex=zz"), 0), "NaN");
        assert!(!links_expired(&url("https://cdn.discordapp.com/attachments/1/2/a.png?ex=ffffffff"), 0));
        assert!(!links_expired(&url("not a url"), 0), "the client can't parse it either");
    }

    #[test]
    fn a_signature_reads_up_to_the_first_non_digit() {
        // `parseInt("ffffffffzz", 16)`.
        let m = message(&["https://cdn.discordapp.com/attachments/1/2/a.png?ex=ffffffffzz"], serde_json::json!([]));
        assert!(!links_expired(&m, 0));
    }

    fn image(url: &str) -> serde_json::Value {
        serde_json::json!({"type": "rich", "url": "https://e.com/p", "image": {"url": url, "width": 8, "height": 8}})
    }

    #[test]
    fn an_embed_expires_only_on_discords_hosts() {
        let m = |url| message(&[], serde_json::json!([image(url)]));
        assert!(links_expired(&m("https://media.discordapp.net/attachments/1/2/a.png?ex=1"), 2_000));
        assert!(links_expired(&m("https://cdn.discordapp.com/attachments/1/2/a.png"), 0), "an attachment is signed");
        assert!(
            !links_expired(&m("https://cdn.discordapp.com/avatars/5/a.png"), 0),
            "neither signed nor an attachment"
        );
        assert!(!links_expired(&m("https://example.com/attachments/1/2/a.png?ex=1"), 2_000), "someone else's host");
        assert!(
            !links_expired(&m("https://media.discordapp.net/external/x/a.png?ex=1"), 2_000),
            "proxied from elsewhere"
        );
        assert!(
            links_expired(&m("https://media-2.discordapp.net/attachments/1/2/a.png?ex=1"), 2_000),
            "a sibling host"
        );
        assert!(!links_expired(&m("https://media-.discordapp.net/attachments/1/2/a.png?ex=1"), 2_000));
    }

    #[test]
    fn embeds_of_one_link_keep_the_first_ones_video_and_all_the_images() {
        let video = |url| {
            serde_json::json!({"type": "video", "url": "https://e.com/p", "thumbnail": {"url": "https://e.com/t.png", "width": 8, "height": 8},
                "video": {"url": url, "width": 8, "height": 8}})
        };
        let old = "https://cdn.discordapp.com/attachments/1/2/a.mp4?ex=1";
        let first = message(&[], serde_json::json!([video(old)]));
        assert!(links_expired(&first, 2_000));
        let second = message(&[], serde_json::json!([image("https://e.com/a.png"), video(old)]));
        assert!(!links_expired(&second, 2_000), "merged into the first, which has no video");
        let separate = message(
            &[],
            serde_json::json!([{"type": "video", "url": "https://e.com/q", "video": {"url": old, "width": 8, "height": 8},
                "thumbnail": {"url": "https://e.com/t.png", "width": 8, "height": 8}}]),
        );
        assert!(links_expired(&separate, 2_000));
        let later_image = message(
            &[],
            serde_json::json!([
                image("https://e.com/a.png"),
                image("https://cdn.discordapp.com/attachments/1/2/b.png?ex=1")
            ]),
        );
        assert!(links_expired(&later_image, 2_000));
    }

    #[test]
    fn a_pasted_attachment_link_expires_through_its_thumbnail() {
        // The client takes an image or article embed's picture from its thumbnail
        // (`fK` in 659674), which is where Discord puts it for a pasted link.
        let embed = |kind: &str, size: u32| {
            serde_json::json!([{"type": kind, "url": "https://cdn.discordapp.com/attachments/1/2/a.png?ex=1",
                "thumbnail": {"url": "https://cdn.discordapp.com/attachments/1/2/a.png?ex=1", "width": size, "height": size}}])
        };
        assert!(links_expired(&message(&[], embed("image", 8)), 2_000));
        assert!(links_expired(&message(&[], embed("article", 8)), 2_000));
        assert!(!links_expired(&message(&[], embed("rich", 8)), 2_000), "a small picture of a rich embed, not media");
        assert!(!links_expired(&message(&[], embed("image", 0)), 2_000), "without a size the client drops it");
    }

    #[test]
    fn a_video_needs_its_still_and_isnt_sketchfabs() {
        let old = "https://cdn.discordapp.com/attachments/1/2/a.mp4?ex=1";
        let video = |extra: serde_json::Value| {
            let mut embed = serde_json::json!({"type": "video", "url": "https://e.com/p", "video": {"url": old, "width": 8, "height": 8}});
            embed.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            message(&[], serde_json::json!([embed]))
        };
        let still = serde_json::json!({"thumbnail": {"url": "https://e.com/t.png", "width": 8, "height": 8}});
        assert!(!links_expired(&video(serde_json::json!({})), 2_000), "no still, no video");
        assert!(links_expired(&video(still.clone()), 2_000));
        let sketchfab = serde_json::json!({"thumbnail": still["thumbnail"], "provider": {"name": "Sketchfab"}});
        assert!(!links_expired(&video(sketchfab), 2_000), "never shown");
    }

    #[test]
    fn the_original_of_a_proxied_file_is_the_cdns() {
        let proxied =
            "https://media.discordapp.net/attachments/1/2/a.png?ex=1&width=100&height=50&format=webp&quality=lossless";
        assert_eq!(original(proxied, None, None), "https://cdn.discordapp.com/attachments/1/2/a.png?ex=1");
        // The CDN's own links are untouched, size parameters and all.
        let cdn = "https://cdn.discordapp.com/attachments/1/2/a.png?ex=1&width=3";
        assert_eq!(original(cdn, None, Some("png")), cdn);
        // Other hosts keep theirs; a file of no known type is asked as `format`.
        let external = "https://images-ext-1.discordapp.net/external/x/https/e.test/a?width=1&height=1#top";
        assert_eq!(original(external, None, None), "https://images-ext-1.discordapp.net/external/x/https/e.test/a#top");
        assert_eq!(original("https://e.test/video", None, Some("png")), "https://e.test/video?format=png");
        assert_eq!(original("https://e.test/video", Some("image/gif"), Some("png")), "https://e.test/video");
        assert_eq!(
            original("https://media.discordapp.net:8443/a.png", None, None),
            "https://media.discordapp.net:8443/a.png"
        );
    }

    #[test]
    fn a_files_type_is_its_content_type_then_its_extension() {
        assert_eq!(extension("https://e.test/a.PNG?x=1", None).as_deref(), Some("png"));
        assert_eq!(extension("https://e.test/a.png", Some("image/jpeg")).as_deref(), Some("jpg"));
        assert_eq!(extension("https://e.test/a.png", Some("video/quicktime")).as_deref(), Some("mov"));
        assert_eq!(extension("https://e.test/a.png", Some("text/plain")).as_deref(), Some("png"));
        assert_eq!(extension("https://e.test/a/file.longext", None), None);
        assert_eq!(extension("https://e.test/a/file", None), None);
    }

    #[test]
    fn only_discords_own_hosts_are_assets() {
        assert!(is_asset("https://cdn.discordapp.com/a.png"));
        assert!(is_asset("https://media.discordapp.net/a.png"));
        assert!(is_asset("https://images-ext-2.discordapp.net/external/x"));
        assert!(!is_asset("https://media.tenor.com/a.gif"));
        assert!(!is_asset("https://discordapp.net.evil.test/a.png"));
        assert!(!is_asset("https://notdiscordapp.com/a.png"));
    }

    #[test]
    fn fitting_rounds_after_each_cut() {
        assert_eq!(fit((4000, 2000), (1000, 1000)), (1000, 500));
        assert_eq!(fit((2000, 4000), (1000, 1000)), (500, 1000));
        assert_eq!(fit((300, 200), (1000, 1000)), (300, 200), "never enlarged");
        assert_eq!(fit((1001, 7), (1000, 1000)), (1000, 7));
        assert_eq!(fit((10, 4000), (100, 100)), (0, 100), "a sliver rounds away");
    }

    #[test]
    fn the_viewer_asks_the_proxy_for_the_size_it_shows() {
        let proxy = "https://media.discordapp.net/attachments/1/2/a.png?ex=1&is=2&hm=3&";
        // Shown at 1000x500 of 4000x2000 on a screen at twice the scale.
        assert_eq!(
            modal_url(proxy, (4000, 2000), (1000, 500), 2, false),
            "https://media.discordapp.net/attachments/1/2/a.png?ex=1&is=2&hm=3&width=2000&height=1000"
        );
        // At its own size the proxy is not asked to resize.
        assert_eq!(modal_url(proxy, (1000, 500), (1000, 500), 1, false), proxy);
        // Never above 4096 a side.
        let huge = modal_url(proxy, (9000, 4500), (9000, 4500), 1, false);
        assert!(huge.ends_with("width=4096&height=2048"), "{huge}");
        // The CDN's own links, and anything not a link, are loaded as they are.
        let cdn = "https://cdn.discordapp.com/attachments/1/2/a.png?ex=1";
        assert_eq!(modal_url(cdn, (4000, 2000), (1000, 500), 2, false), cdn);
        // An animated WebP stays animated.
        let webp = "https://media.discordapp.net/attachments/1/2/a.webp?ex=1";
        assert_eq!(modal_url(webp, (10, 10), (10, 10), 1, true), format!("{webp}&animated=true"));
        assert_eq!(modal_url(webp, (10, 10), (10, 10), 1, false), webp);
    }
}
