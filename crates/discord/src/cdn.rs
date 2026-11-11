//! Build Discord CDN URLs for avatars, guild icons and custom emoji, and
//! sized requests to the media proxy.
//!
//! We always request an explicit extension so the bytes are a format
//! gdk-pixbuf decodes (PNG for static, GIF for animated): `a_`-prefixed hashes
//! are animated. `size` must be a power of two in 16..=4096.

const CDN: &str = "https://cdn.discordapp.com";

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
}
