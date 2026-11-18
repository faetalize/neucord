//! WebP, which the gdk-pixbuf of this system has no loader for, though the
//! media proxy and the CDN serve it wherever the official client asks for it
//! (see [`crate::cdn`]). Decoded here into RGBA, still or animated, for the
//! caller to put in a texture.
//!
//! Both [`decode_still`] and [`decode_animation`] take the bytes by
//! reference and give owned plain data, so they run on the thread pool.
//! They give up (`None`) on a file that doesn't hold together or whose frames
//! would take more than [`crate::apng::MAX_BYTES`] of pixels.

use std::io::Cursor;

use image_webp::{LoopCount, WebPDecoder};

use crate::apng::{Animation, Frame, MAX_BYTES};

/// Whether `bytes` is a WebP file: `RIFF`, a size, `WEBP`.
pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// A picture as RGBA with straight alpha.
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The picture of `bytes`: of an animation, its first frame.
pub fn decode_still(bytes: &[u8]) -> Option<Still> {
    let mut decoder = decoder(bytes)?;
    let (width, height) = decoder.dimensions();
    let mut raw = vec![0; decoder.output_buffer_size()?];
    if decoder.is_animated() {
        decoder.read_frame(&mut raw).ok()?;
    } else {
        decoder.read_image(&mut raw).ok()?;
    }
    Some(Still { width, height, rgba: rgba(raw, decoder.has_alpha()) })
}

/// Every frame of an animated WebP, each whole, as the browsers draw it
/// (blending and disposal are the decoder's). `None` for a picture that
/// doesn't move.
pub fn decode_animation(bytes: &[u8]) -> Option<Animation> {
    let mut decoder = decoder(bytes).filter(WebPDecoder::is_animated)?;
    let (width, height) = decoder.dimensions();
    let canvas = decoder.output_buffer_size()?;
    let has_alpha = decoder.has_alpha();
    let pixels = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if pixels.checked_mul(decoder.num_frames() as usize)? > MAX_BYTES {
        return None;
    }
    let mut frames = Vec::with_capacity(decoder.num_frames() as usize);
    let mut raw = vec![0; canvas];
    for _ in 0..decoder.num_frames() {
        let delay_ms = decoder.read_frame(&mut raw).ok()?;
        frames.push(Frame { rgba: rgba(raw.clone(), has_alpha), delay_ms });
    }
    let plays = match decoder.loop_count() {
        LoopCount::Forever => 0,
        LoopCount::Times(times) => u32::from(times.get()),
    };
    Some(Animation { width, height, plays, frames })
}

fn decoder(bytes: &[u8]) -> Option<WebPDecoder<Cursor<&[u8]>>> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes)).ok()?;
    // A still of any size the format allows would take gigabytes.
    decoder.set_memory_limit(MAX_BYTES);
    let (width, height) = decoder.dimensions();
    (width > 0 && height > 0 && (width as usize).checked_mul(height as usize)?.checked_mul(4)? <= MAX_BYTES)
        .then_some(decoder)
}

/// `raw` as four bytes a pixel: the decoder gives three for a picture
/// without alpha.
fn rgba(raw: Vec<u8>, has_alpha: bool) -> Vec<u8> {
    if has_alpha {
        return raw;
    }
    raw.chunks_exact(3).flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with Pillow: solid colours, so any decoder reads one set of pixels.
    const SOLID: &[u8] = include_bytes!("../testdata/solid.webp");
    const LOSSY: &[u8] = include_bytes!("../testdata/lossy.webp");
    const ALPHA: &[u8] = include_bytes!("../testdata/alpha.webp");
    const ANIMATED: &[u8] = include_bytes!("../testdata/animated.webp");

    fn pixel(still: &Still, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * still.width + x) * 4) as usize;
        still.rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_webp_is_known_by_its_header() {
        assert!(is_webp(SOLID) && is_webp(ANIMATED));
        assert!(!is_webp(b"RIFF....WAVE"));
        assert!(!is_webp(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_webp(b"RIFF"));
    }

    #[test]
    fn a_lossless_picture_without_alpha_decodes_to_opaque_rgba() {
        let still = decode_still(SOLID).expect("decodes");
        assert_eq!((still.width, still.height, still.rgba.len()), (4, 2, 4 * 2 * 4));
        assert_eq!(pixel(&still, 0, 0), [200, 30, 60, 255]);
        assert_eq!(pixel(&still, 3, 1), [200, 30, 60, 255]);
    }

    #[test]
    fn a_lossy_picture_decodes_close_to_its_colour() {
        let still = decode_still(LOSSY).expect("decodes");
        assert_eq!((still.width, still.height), (16, 16));
        let [r, g, b, a] = pixel(&still, 8, 8);
        assert_eq!(a, 255);
        for (got, want) in [(r, 200), (g, 30), (b, 60)] {
            assert!(got.abs_diff(want) <= 6, "{got} is not near {want}");
        }
    }

    #[test]
    fn alpha_stays_straight() {
        let still = decode_still(ALPHA).expect("decodes");
        assert_eq!(pixel(&still, 0, 0)[3], 0);
        assert_eq!(pixel(&still, 1, 1), [10, 120, 250, 255]);
    }

    #[test]
    fn an_animation_has_all_its_frames_whole_with_their_delays() {
        let animation = decode_animation(ANIMATED).expect("decodes");
        assert_eq!((animation.width, animation.height, animation.plays), (4, 2, 0));
        let delays: Vec<u32> = animation.frames.iter().map(|frame| frame.delay_ms).collect();
        assert_eq!(delays, [50, 100, 150]);
        // The encoder is off by one in a channel here and there.
        let near =
            |frame: &Frame, want: [u8; 4]| frame.rgba[..4].iter().zip(want).all(|(got, want)| got.abs_diff(want) <= 2);
        for (frame, want) in animation.frames.iter().zip([[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]) {
            assert!(near(frame, want), "{:?} is not near {want:?}", &frame.rgba[..4]);
        }
        assert!(animation.frames.iter().all(|frame| frame.rgba.len() == 4 * 2 * 4));
    }

    #[test]
    fn the_still_of_an_animation_is_its_first_frame() {
        let still = decode_still(ANIMATED).expect("decodes");
        assert_eq!(pixel(&still, 2, 1), [255, 0, 0, 255], "the first of red, green and blue");
    }

    #[test]
    fn a_picture_that_doesnt_move_has_no_animation() {
        assert!(decode_animation(SOLID).is_none());
    }

    #[test]
    fn what_isnt_a_whole_webp_gives_nothing() {
        assert!(decode_still(b"").is_none());
        assert!(decode_still(&SOLID[..SOLID.len() - 8]).is_none());
        assert!(decode_animation(b"RIFF\x04\0\0\0WEBP").is_none());
    }
}
