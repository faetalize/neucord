//! Pictures decoded into plain RGBA, for the app to put in textures: the
//! decoders that need no GTK, so they run on the thread pool, where a 12
//! megapixel photo or a GIF of a hundred frames takes the time it takes
//! without a frame of the window waiting for it (`gdk::Texture::from_bytes`
//! can only be called on the main loop; gtk4-rs checks).
//!
//! [`decode_still`] and [`decode_animation`] take the bytes by reference and
//! give owned data. WebP and APNG have decoders of their own ([`crate::webp`],
//! [`crate::apng`]); the rest, GIF included, goes through gdk-pixbuf, whose
//! pictures are 8 bits a channel (GTK's own loaders keep 16 bits and a
//! profile, which nothing here draws any differently from the proxy's WebP).
//! An animation takes at most [`crate::apng::MAX_BYTES`] of frames, whatever
//! its format.

use std::time::{Duration, SystemTime};

use gdk_pixbuf::prelude::*;
use gdk_pixbuf::{PixbufAnimation, PixbufLoader};

use crate::apng::{self, Animation, Frame, MAX_BYTES};
use crate::images::rgba;
use crate::webp::{self, Still};

/// The picture of `bytes`: of an animation, its first frame. `None` for what
/// gdk-pixbuf has no loader for either.
pub fn decode_still(bytes: &[u8]) -> Option<Still> {
    if webp::is_webp(bytes) {
        return webp::decode_still(bytes);
    }
    let loader = PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    let (width, height) = (u32::try_from(pixbuf.width()).ok()?, u32::try_from(pixbuf.height()).ok()?);
    Some(Still { width, height, rgba: rgba(&pixbuf) })
}

/// Every frame of an animated WebP, APNG or GIF, each whole. `None` for a
/// picture that doesn't move, one that doesn't hold together, and one whose
/// frames would take more than [`MAX_BYTES`].
pub fn decode_animation(bytes: &[u8]) -> Option<Animation> {
    if webp::is_webp(bytes) {
        webp::decode_animation(bytes)
    } else if apng::is_apng(bytes) {
        apng::decode(bytes)
    } else {
        decode_gif(bytes)
    }
}

/// A GIF's frames, drawn over one another as the file says (gdk-pixbuf's
/// iterator does that), each with the time it shows for. Its loop count isn't
/// read: it goes on for ever, as it does in a browser's `<img>` for a file
/// that asks for it, and as it has here.
fn decode_gif(bytes: &[u8]) -> Option<Animation> {
    let shape = gif_shape(bytes)?;
    let canvas = (shape.width as usize).checked_mul(shape.height as usize)?.checked_mul(4)?;
    // Decided before gdk-pixbuf decodes anything.
    if shape.frames < 2 || canvas == 0 || canvas.checked_mul(shape.frames)? > MAX_BYTES {
        return None;
    }
    let loader = PixbufLoader::with_type("gif").ok()?;
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let animation: PixbufAnimation = loader.animation().filter(|animation| !animation.is_static_image())?;
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let iter = animation.iter(Some(start));
    let mut now = start;
    let mut frames = Vec::with_capacity(shape.frames);
    for _ in 0..shape.frames {
        let pixbuf = iter.pixbuf();
        if pixbuf.width() as u32 != shape.width || pixbuf.height() as u32 != shape.height {
            return None;
        }
        // No delay is the old default of a GIF with none.
        let delay = iter.delay_time().unwrap_or(Duration::from_millis(100));
        frames.push(Frame { rgba: rgba(&pixbuf), delay_ms: u32::try_from(delay.as_millis()).unwrap_or(u32::MAX) });
        // Always forward, or a frame with no delay would be read again.
        now += delay.max(Duration::from_millis(1));
        iter.advance(now);
    }
    Some(Animation { width: shape.width, height: shape.height, plays: 0, frames })
}

struct GifShape {
    width: u32,
    height: u32,
    frames: usize,
}

/// A GIF's canvas and how many pictures it holds, read from its blocks:
/// the iterator of gdk-pixbuf loops without saying where it ends.
fn gif_shape(bytes: &[u8]) -> Option<GifShape> {
    if !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return None;
    }
    let size = |at: usize| Some(u32::from(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?)));
    let (width, height) = (size(6)?, size(8)?);
    let mut at = 13 + color_table(*bytes.get(10)?);
    let mut frames = 0;
    loop {
        match *bytes.get(at)? {
            0x3B => return Some(GifShape { width, height, frames }),
            // An extension: its label, then its data in sub-blocks.
            0x21 => at = after_sub_blocks(bytes, at + 2)?,
            // A picture: its descriptor, a local palette, the code size, the data.
            0x2C => {
                at += 10 + color_table(*bytes.get(at + 9)?) + 1;
                at = after_sub_blocks(bytes, at)?;
                frames += 1;
            }
            _ => return None,
        }
    }
}

/// The bytes of a palette, from the packed field of a descriptor.
fn color_table(packed: u8) -> usize {
    if packed & 0x80 != 0 { 3 << ((packed & 7) + 1) } else { 0 }
}

/// Where the sub-blocks that start at `at` end: each is a length and that
/// many bytes, and a length of 0 closes them.
fn after_sub_blocks(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let length = usize::from(*bytes.get(at)?);
        at += 1 + length;
        if length == 0 {
            return Some(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with ImageMagick. `PIC` is 4 by 3: red, a blue pixel at (1, 1) and a
    // clear one at (3, 2). `TWO` is two frames of that size, red for 100 ms and
    // blue for 300; `ONE` is the red alone.
    const PIC: &[u8] = include_bytes!("../testdata/pic.png");
    const TWO: &[u8] = include_bytes!("../testdata/two.gif");
    const ONE: &[u8] = include_bytes!("../testdata/one.gif");
    const RED: [u8; 4] = [200, 30, 60, 255];
    const BLUE: [u8; 4] = [10, 120, 250, 255];

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_png_decodes_to_straight_rgba() {
        let still = decode_still(PIC).expect("decodes");
        assert_eq!((still.width, still.height, still.rgba.len()), (4, 3, 4 * 3 * 4));
        assert_eq!(pixel(&still.rgba, 4, 0, 0), RED);
        assert_eq!(pixel(&still.rgba, 4, 1, 1), BLUE);
        assert_eq!(pixel(&still.rgba, 4, 3, 2)[3], 0);
    }

    #[test]
    fn a_picture_without_alpha_comes_out_opaque() {
        let still = decode_still(ONE).expect("decodes");
        assert_eq!((still.width, still.height), (4, 3));
        assert_eq!(pixel(&still.rgba, 4, 2, 2), RED);
    }

    #[test]
    fn the_still_of_an_animation_is_its_first_frame() {
        let still = decode_still(TWO).expect("decodes");
        assert_eq!(pixel(&still.rgba, 4, 0, 0), RED);
    }

    #[test]
    fn what_no_loader_reads_has_no_still() {
        assert!(decode_still(b"not a picture").is_none());
        assert!(decode_still(&PIC[..PIC.len() / 2]).is_none());
        assert!(decode_still(b"").is_none());
    }

    #[test]
    fn a_webp_goes_to_its_own_decoder() {
        let webp = include_bytes!("../testdata/solid.webp");
        assert_eq!(decode_still(webp).map(|still| (still.width, still.height)), Some((4, 2)));
        let moving = include_bytes!("../testdata/animated.webp");
        assert!(decode_animation(moving).is_some());
        assert!(decode_animation(webp).is_none(), "a still WebP doesn't move");
    }

    #[test]
    fn a_gif_has_every_frame_whole_with_its_delay() {
        let animation = decode_animation(TWO).expect("decodes");
        assert_eq!((animation.width, animation.height, animation.plays), (4, 3, 0));
        let delays: Vec<_> = animation.frames.iter().map(|frame| frame.delay_ms).collect();
        assert_eq!(delays, [100, 300]);
        assert_eq!(pixel(&animation.frames[0].rgba, 4, 2, 1), RED);
        assert_eq!(pixel(&animation.frames[1].rgba, 4, 2, 1), BLUE);
        assert!(animation.frames.iter().all(|frame| frame.rgba.len() == 4 * 3 * 4));
    }

    #[test]
    fn a_gif_that_holds_one_picture_does_not_move() {
        assert!(decode_animation(ONE).is_none());
        assert!(decode_animation(PIC).is_none(), "nor does a PNG");
    }

    #[test]
    fn a_gif_is_counted_by_its_blocks() {
        let shape = gif_shape(TWO).expect("a GIF");
        assert_eq!((shape.width, shape.height, shape.frames), (4, 3, 2));
        assert_eq!(gif_shape(ONE).map(|shape| shape.frames), Some(1));
        assert!(gif_shape(&TWO[..TWO.len() - 1]).is_none(), "no trailer, no GIF");
        assert!(gif_shape(PIC).is_none());
    }

    #[test]
    fn a_gif_whose_frames_would_not_fit_is_not_decoded() {
        // The same two frames on a canvas of 30000 by 30000 pixels.
        let mut huge = TWO.to_vec();
        huge[6..10].copy_from_slice(&[0x30, 0x75, 0x30, 0x75]);
        assert_eq!(gif_shape(&huge).map(|shape| shape.width), Some(30000));
        assert!(decode_animation(&huge).is_none());
    }
}
