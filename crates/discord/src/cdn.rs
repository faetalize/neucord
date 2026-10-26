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

#[cfg(test)]
mod tests {
    use super::*;

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
}
