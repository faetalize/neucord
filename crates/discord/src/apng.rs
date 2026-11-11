//! Animated PNG, which the PNG loader of gdk-pixbuf shows as its first frame:
//! the frames are cut out of the file, each decoded as the PNG it is, and
//! drawn over one another as the file says, so every frame comes out whole.
//! The official client shows these stickers animated (a browser does).
//!
//! [`decode`] gives up, and the first frame stays, on a file that doesn't
//! hold together or would take more than [`MAX_BYTES`] of frames.

use gdk_pixbuf::prelude::*;
use gdk_pixbuf::{Pixbuf, PixbufLoader};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// All the frames of one animation, whole, as RGBA. A sticker is small, but a
/// long one at a high size isn't.
pub const MAX_BYTES: usize = 48 * 1024 * 1024;

pub struct Animation {
    pub width: u32,
    pub height: u32,
    /// How many times it plays; 0 is for ever.
    pub plays: u32,
    pub frames: Vec<Frame>,
}

pub struct Frame {
    /// The whole picture at this frame, RGBA with straight alpha.
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// What `fcTL` says of a frame.
#[derive(Clone, Copy)]
struct Control {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    delay_ms: u32,
    /// 0 leaves it, 1 clears its region, 2 puts back what was there.
    dispose: u8,
    /// 0 replaces the pixels, 1 draws over them.
    blend: u8,
}

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    /// The whole chunk as it is in the file, length and CRC too.
    raw: &'a [u8],
}

fn chunks(bytes: &[u8]) -> Option<Vec<Chunk<'_>>> {
    let mut rest = bytes.strip_prefix(&SIGNATURE)?;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let length = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let total = length.checked_add(12)?;
        let raw = rest.get(..total)?;
        out.push(Chunk { kind: raw[4..8].try_into().ok()?, data: &raw[8..8 + length], raw });
        rest = &rest[total..];
    }
    Some(out)
}

/// Whether `bytes` is a PNG with animation: an `acTL` before the image.
pub fn is_apng(bytes: &[u8]) -> bool {
    let Some(chunks) = chunks(bytes) else { return false };
    chunks.iter().take_while(|c| &c.kind != b"IDAT").any(|c| &c.kind == b"acTL")
}

/// The CRC-32 a PNG chunk ends with.
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = u32::MAX;
    for byte in parts.iter().flat_map(|p| p.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    out
}

pub fn decode(bytes: &[u8]) -> Option<Animation> {
    let chunks = chunks(bytes)?;
    let header = chunks.first().filter(|c| &c.kind == b"IHDR" && c.data.len() == 13)?;
    let number = |at: usize| u32::from_be_bytes(header.data[at..at + 4].try_into().unwrap());
    let (width, height) = (number(0), number(4));
    let plays = chunks.iter().find(|c| &c.kind == b"acTL").and_then(|c| c.data.get(4..8))?;
    let plays = u32::from_be_bytes(plays.try_into().ok()?);

    // What goes before the image in each frame's own PNG: palette, transparency, colour.
    let mut shared: Vec<u8> = Vec::new();
    let mut frames: Vec<(Control, Vec<u8>)> = Vec::new();
    let mut image_seen = false;
    for c in &chunks[1..] {
        match &c.kind {
            b"fcTL" => frames.push((control(c.data)?, Vec::new())),
            // The first image is a frame only if a `fcTL` came before it.
            b"IDAT" => {
                image_seen = true;
                if let Some((_, data)) = frames.last_mut() {
                    data.extend_from_slice(c.data);
                }
            }
            b"fdAT" => frames.last_mut()?.1.extend_from_slice(c.data.get(4..)?),
            b"acTL" | b"IEND" => {}
            _ if !image_seen => shared.extend_from_slice(c.raw),
            _ => {}
        }
    }
    if frames.len() < 2 || frames.iter().any(|(_, data)| data.is_empty()) {
        return None;
    }
    let canvas_bytes = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if canvas_bytes == 0 || canvas_bytes.checked_mul(frames.len())? > MAX_BYTES {
        return None;
    }

    let mut canvas = vec![0u8; canvas_bytes];
    let mut out = Vec::with_capacity(frames.len());
    let mut undo: Option<(Control, Option<Vec<u8>>)> = None;
    for (i, (control, data)) in frames.into_iter().enumerate() {
        // The last frame's disposal comes before this one is drawn.
        if let Some((last, before)) = undo.take() {
            match (last.dispose, before) {
                (2, Some(before)) => canvas = before,
                (1 | 2, _) => clear(&mut canvas, width, &last),
                _ => {}
            }
        }
        if control.x.checked_add(control.width)? > width || control.y.checked_add(control.height)? > height {
            return None;
        }
        let mut png = SIGNATURE.to_vec();
        let mut ihdr = header.data.to_vec();
        ihdr[0..4].copy_from_slice(&control.width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&control.height.to_be_bytes());
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&shared);
        png.extend(chunk(b"IDAT", &data));
        png.extend(chunk(b"IEND", &[]));
        let pixels = rgba_of(&png, control.width, control.height)?;

        // A first frame can't go back to what isn't there.
        let before = (control.dispose == 2 && i > 0).then(|| canvas.clone());
        draw(&mut canvas, width, &control, &pixels);
        out.push(Frame { rgba: canvas.clone(), delay_ms: control.delay_ms });
        undo = Some((control, before));
    }
    Some(Animation { width, height, plays, frames: out })
}

fn control(data: &[u8]) -> Option<Control> {
    let number = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let short = |at: usize| Some(u32::from(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?)));
    let (numerator, denominator) = (short(20)?, short(22)?);
    // A denominator of 0 is 100.
    let delay_ms = numerator * 1000 / if denominator == 0 { 100 } else { denominator };
    Some(Control {
        width: number(4)?,
        height: number(8)?,
        x: number(12)?,
        y: number(16)?,
        delay_ms,
        dispose: *data.get(24)?,
        blend: *data.get(25)?,
    })
}

/// A PNG's pixels as RGBA, `width` by `height`.
fn rgba_of(png: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let loader = PixbufLoader::with_type("png").ok()?;
    loader.write(png).ok()?;
    loader.close().ok()?;
    let mut pixbuf: Pixbuf = loader.pixbuf()?;
    if !pixbuf.has_alpha() {
        pixbuf = pixbuf.add_alpha(false, 0, 0, 0).ok()?;
    }
    if pixbuf.width() as u32 != width || pixbuf.height() as u32 != height {
        return None;
    }
    let stride = pixbuf.rowstride() as usize;
    let pixels = pixbuf.read_pixel_bytes();
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        out.extend_from_slice(pixels.get(row * stride..row * stride + width as usize * 4)?);
    }
    Some(out)
}

fn clear(canvas: &mut [u8], width: u32, region: &Control) {
    for row in 0..region.height {
        let start = (((region.y + row) * width + region.x) * 4) as usize;
        canvas[start..start + region.width as usize * 4].fill(0);
    }
}

fn draw(canvas: &mut [u8], width: u32, region: &Control, pixels: &[u8]) {
    for row in 0..region.height {
        let at = (((region.y + row) * width + region.x) * 4) as usize;
        let from = (row * region.width * 4) as usize;
        let (line, new) =
            (&mut canvas[at..at + region.width as usize * 4], &pixels[from..from + region.width as usize * 4]);
        if region.blend == 0 {
            line.copy_from_slice(new);
            continue;
        }
        for (old, new) in line.chunks_exact_mut(4).zip(new.chunks_exact(4)) {
            let (new_alpha, old_alpha) = (u32::from(new[3]), u32::from(old[3]));
            // Straight alpha, new over old: the colours weigh by their share.
            let out_alpha = new_alpha * 255 + old_alpha * (255 - new_alpha);
            if out_alpha == 0 {
                old.fill(0);
                continue;
            }
            for c in 0..3 {
                let mixed = u32::from(new[c]) * new_alpha * 255 + u32::from(old[c]) * old_alpha * (255 - new_alpha);
                old[c] = ((mixed + out_alpha / 2) / out_alpha) as u8;
            }
            old[3] = ((out_alpha + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use gdk_pixbuf::Colorspace;

    use super::*;

    /// A PNG of one colour.
    fn solid(width: i32, height: i32, rgba: [u8; 4]) -> Vec<u8> {
        let pixels: Vec<u8> = rgba.iter().copied().cycle().take((width * height * 4) as usize).collect();
        let bytes = gdk_pixbuf::glib::Bytes::from_owned(pixels);
        let pixbuf = Pixbuf::from_bytes(&bytes, Colorspace::Rgb, true, 8, width, height, width * 4);
        pixbuf.save_to_bufferv("png", &[]).unwrap()
    }

    /// What a PNG made by [`solid`] holds, to build frames from.
    fn image_data(png: &[u8]) -> Vec<u8> {
        chunks(png).unwrap().iter().filter(|c| &c.kind == b"IDAT").flat_map(|c| c.data.to_vec()).collect()
    }

    /// One frame of an animation: a colour, where and how big, how long, and what happens to it.
    struct Part {
        rgba: [u8; 4],
        size: (u32, u32),
        at: (u32, u32),
        delay: (u16, u16),
        dispose: u8,
        blend: u8,
    }

    fn part(rgba: [u8; 4]) -> Part {
        Part { rgba, size: (2, 2), at: (0, 0), delay: (1, 10), dispose: 0, blend: 0 }
    }

    /// An APNG of 2×2 whose first frame is the default image, as most are.
    fn animation(parts: &[Part], plays: u32) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        out.extend(chunk(b"IHDR", &ihdr));
        let mut actl = (parts.len() as u32).to_be_bytes().to_vec();
        actl.extend_from_slice(&plays.to_be_bytes());
        out.extend(chunk(b"acTL", &actl));
        let mut sequence = 0u32;
        for (i, p) in parts.iter().enumerate() {
            let mut fctl = sequence.to_be_bytes().to_vec();
            sequence += 1;
            for n in [p.size.0, p.size.1, p.at.0, p.at.1] {
                fctl.extend_from_slice(&n.to_be_bytes());
            }
            fctl.extend_from_slice(&p.delay.0.to_be_bytes());
            fctl.extend_from_slice(&p.delay.1.to_be_bytes());
            fctl.extend_from_slice(&[p.dispose, p.blend]);
            out.extend(chunk(b"fcTL", &fctl));
            let data = image_data(&solid(p.size.0 as i32, p.size.1 as i32, p.rgba));
            if i == 0 {
                out.extend(chunk(b"IDAT", &data));
            } else {
                let mut fdat = sequence.to_be_bytes().to_vec();
                sequence += 1;
                fdat.extend_from_slice(&data);
                out.extend(chunk(b"fdAT", &fdat));
            }
        }
        out.extend(chunk(b"IEND", &[]));
        out
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 4] {
        frame.rgba[(y * 2 + x) * 4..(y * 2 + x) * 4 + 4].try_into().unwrap()
    }

    #[test]
    fn a_plain_png_is_not_an_animation() {
        assert!(!is_apng(&solid(2, 2, RED)));
        assert!(decode(&solid(2, 2, RED)).is_none());
        assert!(!is_apng(b"GIF89a") && !is_apng(&[]));
    }

    #[test]
    fn frames_come_out_whole_with_their_delays() {
        let file = animation(&[part(RED), Part { delay: (3, 0), ..part(BLUE) }], 0);
        assert!(is_apng(&file));
        let animation = decode(&file).unwrap();
        assert_eq!((animation.width, animation.height, animation.plays, animation.frames.len()), (2, 2, 0, 2));
        assert_eq!(
            (animation.frames[0].delay_ms, animation.frames[1].delay_ms),
            (100, 30),
            "1/10 s, and 3 of the default 1/100"
        );
        assert!(animation.frames[0].rgba.chunks(4).all(|p| p == RED));
        assert!(animation.frames[1].rgba.chunks(4).all(|p| p == BLUE));
    }

    #[test]
    fn a_frame_that_covers_part_leaves_the_rest_of_the_last() {
        let corner = Part { size: (1, 1), at: (1, 1), ..part(BLUE) };
        let animation = decode(&animation(&[part(RED), corner], 3)).unwrap();
        assert_eq!(animation.plays, 3);
        let second = &animation.frames[1];
        assert_eq!((pixel(second, 0, 0), pixel(second, 1, 1)), (RED, BLUE));
    }

    #[test]
    fn disposing_clears_or_puts_back_before_the_next_frame() {
        let corner = |dispose, rgba| Part { size: (1, 1), at: (1, 1), dispose, ..part(rgba) };
        let green = [0, 255, 0, 255];
        // The corner is cleared after its frame.
        let cleared = decode(&animation(&[part(RED), corner(1, BLUE), corner(0, green)], 0)).unwrap();
        // Only the third's corner is drawn: what the second left there is gone, to clear, not to red.
        assert_eq!(pixel(&cleared.frames[2], 1, 1), green);
        let cleared =
            decode(&animation(&[part(RED), corner(1, BLUE), Part { size: (1, 1), at: (0, 0), ..part(green) }], 0))
                .unwrap();
        assert_eq!((pixel(&cleared.frames[2], 1, 1), pixel(&cleared.frames[2], 0, 0)), (CLEAR, green));
        // Put back what was there: the red.
        let restored =
            decode(&animation(&[part(RED), corner(2, BLUE), Part { size: (1, 1), at: (0, 0), ..part(green) }], 0))
                .unwrap();
        assert_eq!((pixel(&restored.frames[2], 1, 1), pixel(&restored.frames[2], 0, 0)), (RED, green));
        // On the first frame there is nothing to put back: it clears.
        let first =
            decode(&animation(&[Part { dispose: 2, ..part(RED) }, Part { size: (1, 1), ..part(BLUE) }], 0)).unwrap();
        assert_eq!((pixel(&first.frames[1], 1, 1), pixel(&first.frames[1], 0, 0)), (CLEAR, BLUE));
    }

    #[test]
    fn blending_draws_over_or_replaces() {
        let half = [0, 0, 255, 128];
        let over = decode(&animation(&[part(RED), Part { blend: 1, ..part(half) }], 0)).unwrap();
        let mixed = pixel(&over.frames[1], 0, 0);
        assert_eq!(
            (mixed[3], mixed[0] > 100 && mixed[2] > 100),
            (255, true),
            "half blue over red keeps both: {mixed:?}"
        );
        let source = decode(&animation(&[part(RED), part(half)], 0)).unwrap();
        assert_eq!(pixel(&source.frames[1], 0, 0), half, "replaced, not mixed");
        let nothing = decode(&animation(&[part(CLEAR), Part { blend: 1, ..part(CLEAR) }], 0)).unwrap();
        assert_eq!(pixel(&nothing.frames[1], 0, 0), CLEAR);
    }

    #[test]
    fn what_doesnt_hold_together_has_no_animation() {
        let beyond = Part { at: (1, 1), ..part(BLUE) };
        assert!(decode(&animation(&[part(RED), beyond], 0)).is_none(), "a frame past the picture");
        assert!(decode(&animation(&[part(RED)], 0)).is_none(), "one frame is a still");
        let mut cut = animation(&[part(RED), part(BLUE)], 0);
        cut.truncate(cut.len() - 20);
        assert!(decode(&cut).is_none());
    }

    #[test]
    fn crc_is_pngs() {
        // The CRC of "IEND", which every PNG ends with.
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
    }
}
