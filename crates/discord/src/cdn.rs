//! Build Discord CDN URLs for avatars, guild icons and custom emoji.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_avatar() {
        assert_eq!(
            avatar_url(5, "abcdef", 64),
            "https://cdn.discordapp.com/avatars/5/abcdef.png?size=64"
        );
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
}
