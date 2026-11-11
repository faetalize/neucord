//! What the tray icon says, and its pixels.
//!
//! The icon is drawn here instead of named: no icon is installed yet (#26),
//! and a themed name that isn't there leaves an empty slot in the panel. A
//! speech bubble, with a badge in its corner when something waits.

/// Unread state, as the tray shows it. The client's tray has one badge for
/// all of it; a mention gets its own colour here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alert {
    #[default]
    Quiet,
    /// Messages in channels that aren't muted, none addressed to us.
    Unread,
    /// Mentions, direct messages and friend requests, counted together as the
    /// rail's Home badge does.
    Mentions(u32),
}

impl Alert {
    pub fn new(unread: bool, mentions: u32) -> Alert {
        match (mentions, unread) {
            (0, false) => Alert::Quiet,
            (0, true) => Alert::Unread,
            (n, _) => Alert::Mentions(n),
        }
    }

    /// The hover text under the title.
    pub fn describe(self) -> String {
        match self {
            Alert::Quiet => String::new(),
            Alert::Unread => "Unread messages".into(),
            Alert::Mentions(1) => "1 mention".into(),
            Alert::Mentions(n) => format!("{n} mentions"),
        }
    }
}

/// The sizes offered; a panel picks the nearest to its own.
pub const SIZES: [i32; 3] = [22, 32, 48];

const BUBBLE: [u8; 3] = [53, 132, 228];
const UNREAD: [u8; 3] = [245, 194, 17];
const MENTION: [u8; 3] = [224, 27, 36];

/// Samples per pixel along each axis.
const SUPERSAMPLE: i32 = 4;

/// A size-by-size image in the byte order StatusNotifierItem wants: ARGB,
/// most significant byte (alpha) first.
pub fn render(size: i32, alert: Alert) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    let samples = SUPERSAMPLE * SUPERSAMPLE;
    let scale = (size * SUPERSAMPLE) as f32;
    for y in 0..size {
        for x in 0..size {
            let (mut covered, mut sum) = (0, [0i32; 3]);
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let u = (x * SUPERSAMPLE + sx) as f32 + 0.5;
                    let v = (y * SUPERSAMPLE + sy) as f32 + 0.5;
                    if let Some(colour) = colour_at(u / scale, v / scale, alert) {
                        covered += 1;
                        for (total, channel) in sum.iter_mut().zip(colour) {
                            *total += channel as i32;
                        }
                    }
                }
            }
            if covered == 0 {
                pixels.extend([0, 0, 0, 0]);
            } else {
                let alpha = (covered * 255 + samples / 2) / samples;
                pixels.push(alpha as u8);
                pixels.extend(sum.map(|total| (total / covered) as u8));
            }
        }
    }
    pixels
}

/// The colour at a point of the unit square (y down), or nothing where the
/// icon is clear.
fn colour_at(x: f32, y: f32, alert: Alert) -> Option<[u8; 3]> {
    let badge = match alert {
        Alert::Quiet => None,
        Alert::Unread => Some(UNREAD),
        Alert::Mentions(_) => Some(MENTION),
    };
    if let Some(colour) = badge {
        let distance = (x - 0.74).hypot(y - 0.26);
        if distance <= 0.2 {
            return Some(colour);
        }
        // A clear ring keeps the badge apart from the bubble behind it.
        if distance <= 0.27 {
            return None;
        }
    }
    (in_bubble(x, y) || in_tail(x, y)).then_some(BUBBLE)
}

/// A rounded rectangle.
fn in_bubble(x: f32, y: f32) -> bool {
    const RADIUS: f32 = 0.16;
    let (left, right, top, bottom) = (0.06, 0.94, 0.12, 0.74);
    if !(left..=right).contains(&x) || !(top..=bottom).contains(&y) {
        return false;
    }
    // Only the corners are cut: measure from the nearest corner's centre.
    let cx = x.clamp(left + RADIUS, right - RADIUS);
    let cy = y.clamp(top + RADIUS, bottom - RADIUS);
    (x - cx).hypot(y - cy) <= RADIUS
}

/// The bubble's pointer, a triangle under its lower left.
fn in_tail(x: f32, y: f32) -> bool {
    let (a, b, c) = ((0.18, 0.66), (0.18, 0.90), (0.46, 0.66));
    let side = |p: (f32, f32), q: (f32, f32)| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
    let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(image: &[u8], size: i32, x: i32, y: i32) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        image[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn mentions_outrank_unread_and_a_quiet_icon_has_no_badge() {
        assert_eq!(Alert::new(false, 0), Alert::Quiet);
        assert_eq!(Alert::new(true, 0), Alert::Unread);
        assert_eq!(Alert::new(false, 3), Alert::Mentions(3));
        assert_eq!(Alert::new(true, 3), Alert::Mentions(3), "a mention is unread too");
    }

    #[test]
    fn the_hover_text_counts_mentions() {
        assert_eq!(Alert::Quiet.describe(), "");
        assert_eq!(Alert::Unread.describe(), "Unread messages");
        assert_eq!(Alert::Mentions(1).describe(), "1 mention");
        assert_eq!(Alert::Mentions(12).describe(), "12 mentions");
    }

    #[test]
    fn every_size_is_a_full_image_in_argb_order() {
        for size in SIZES {
            assert_eq!(render(size, Alert::Quiet).len(), (size * size * 4) as usize);
        }
        let image = render(32, Alert::Quiet);
        assert_eq!(pixel(&image, 32, 16, 10), [255, BUBBLE[0], BUBBLE[1], BUBBLE[2]], "alpha comes first");
        assert_eq!(pixel(&image, 32, 0, 0), [0, 0, 0, 0], "outside the bubble is clear");
        assert_eq!(pixel(&image, 32, 31, 31)[0], 0);
    }

    #[test]
    fn the_badge_is_in_the_corner_and_takes_its_colour_from_the_alert() {
        let (quiet, unread, mention) =
            (render(48, Alert::Quiet), render(48, Alert::Unread), render(48, Alert::Mentions(2)));
        let corner = |image: &[u8]| pixel(image, 48, 35, 12);
        assert_eq!(corner(&quiet)[1..], BUBBLE, "the bubble is under it");
        assert_eq!(corner(&unread), [255, UNREAD[0], UNREAD[1], UNREAD[2]]);
        assert_eq!(corner(&mention), [255, MENTION[0], MENTION[1], MENTION[2]]);
        assert_eq!(pixel(&mention, 48, 12, 32)[1..], BUBBLE, "the rest is unchanged");
    }

    #[test]
    fn a_clear_ring_keeps_the_badge_off_the_bubble() {
        let image = render(48, Alert::Mentions(1));
        // Between the badge (0.2 away from its centre) and the bubble.
        assert_eq!(pixel(&image, 48, 35, 24)[0], 0);
    }
}
