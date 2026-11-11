//! The conversions the official client makes to an image before uploading it,
//! with the same gates, so Discord sees the kind of file it sees from the
//! client.
//!
//! HEIC/HEIF and JPEG XR become JPEG when the file is added to the draft
//! (`N` in 111542, over `convertFileToJpeg` in 85582), and a small PNG becomes
//! WebP when its upload starts, for accounts in
//! `2025-09-image-attachment-mezzanine-v2` (`tryConvertToWebP` in 48325, over
//! `maybeConvertToWebP` in 673060). A conversion that doesn't apply or fails
//! leaves the file as it was; nothing is reported.
//!
//! The client's JPEG encoder is the operating system's, in the desktop app
//! only (the web client has none and sends the file as it is); here it is the
//! decoder and JPEG encoder of gdk-pixbuf, which GTK already loads. JPEG XR
//! has no decoder in that stack, so such a file always takes the failure path
//! and goes as it is.

use gdk_pixbuf::prelude::*;
use gdk_pixbuf::{Pixbuf, PixbufLoader};

use crate::uploads::{Data, File};

const MIB: u64 = 1024 * 1024;

/// What the client turns into JPEG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Heic,
    Jxr,
}

impl Source {
    /// By type, else by extension (`E5` and `Tn` in 452661); HEIC first.
    fn of(file: &File) -> Option<Source> {
        let ext = file.filename.rfind('.').map(|dot| file.filename[dot..].to_ascii_lowercase());
        let ext = ext.as_deref();
        let heic = ["image/heic", "image/heif", "image/heic-sequence", "image/heif-sequence"];
        let jxr = ["image/jxr", "image/vnd.ms-photo"];
        if heic.contains(&file.content_type.as_str()) || matches!(ext, Some(".heic" | ".heif" | ".hif")) {
            Some(Source::Heic)
        } else if jxr.contains(&file.content_type.as_str()) || matches!(ext, Some(".jxr" | ".wdp" | ".hdp")) {
            Some(Source::Jxr)
        } else {
            None
        }
    }

    /// The encoder's quality, out of 100 (85582).
    fn quality(self) -> u8 {
        match self {
            Source::Heic => 80,
            Source::Jxr => 85,
        }
    }

    /// Bigger files go as they are (85582's `maxFileSizeBytes`).
    fn max_size(self) -> u64 {
        match self {
            Source::Heic => 20 * MIB,
            Source::Jxr => 50 * MIB,
        }
    }

    /// The type the upload names as the original's, whatever becomes of the
    /// file: the file's own, else a guess by name (`II` and `NX` in 452661).
    /// A `.heic` file is `image/heic` whatever gio's table says (it has
    /// `image/heif`): that is the client's own guess, and what the systems
    /// it runs on report.
    fn original_type(self, file: &File) -> String {
        let heic = file.filename.to_lowercase().ends_with(".heic");
        match (file.content_type.as_str(), self) {
            (_, Source::Heic) if heic => "image/heic".to_owned(),
            ("", Source::Heic) => "image/heif".to_owned(),
            ("", Source::Jxr) => "image/jxr".to_owned(),
            (known, _) => known.to_owned(),
        }
    }
}

/// The name of the JPEG (`DP` in 452661): the extension swapped, or `.jpg`
/// added to a name that has none of the source's.
fn jpeg_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stem = [".heic", ".heif", ".hif", ".jxr", ".wdp", ".hdp"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map(|ext| &name[..name.len() - ext.len()]);
    format!("{}.jpg", stem.unwrap_or(name))
}

/// What the client does with a file as it is added to a draft: a HEIC or
/// JPEG XR file within the size limit becomes a JPEG of the same name, with
/// `original_md5` set to the original's, which Discord is told. Any other
/// file, or one that can't be converted, comes back as it was, though a
/// HEIC or JPEG XR one has its type filled in as the client does.
pub async fn prepare(mut file: File) -> File {
    let Some(source) = Source::of(&file) else { return file };
    file.content_type = source.original_type(&file);
    if file.size > source.max_size() {
        return file;
    }
    let Some(bytes) = read(&file.data).await else { return file };
    let converted = gio::spawn_blocking(move || {
        let jpeg = to_jpeg(&bytes, source.quality())?;
        Some((jpeg, md5(&bytes)))
    })
    .await;
    match converted {
        Ok(Some((jpeg, original_md5))) => File {
            filename: jpeg_name(&file.filename),
            size: jpeg.len() as u64,
            data: Data::Memory(glib::Bytes::from_owned(jpeg)),
            original_md5: Some(original_md5),
            ..file
        },
        _ => file,
    }
}

/// What the client does when an upload starts, for an account in
/// `2025-09-image-attachment-mezzanine-v2` whose variant allows files up to
/// `max` bytes: a PNG becomes a lossless WebP, under the name it had, if
/// it's no larger and nothing is lost. The upload still names the PNG as
/// its original type.
pub async fn optimize(file: &File, max: u64) -> Option<File> {
    if file.content_type != "image/png" || file.size > max {
        return None;
    }
    let bytes = read(&file.data).await?;
    let webp = gio::spawn_blocking(move || to_webp(&bytes)).await.ok()??;
    Some(File { size: webp.len() as u64, data: Data::Memory(glib::Bytes::from_owned(webp)), ..file.clone() })
}

async fn read(data: &Data) -> Option<Vec<u8>> {
    match data {
        Data::File(file) => file.load_contents_future().await.ok().map(|(bytes, _)| bytes.to_vec()),
        Data::Memory(bytes) => Some(bytes.to_vec()),
    }
}

/// The hash the client sends as `X-Discord-Original-MD5`.
fn md5(bytes: &[u8]) -> String {
    glib::compute_checksum_for_data(glib::ChecksumType::Md5, bytes).map(String::from).unwrap_or_default()
}

fn load(bytes: &[u8]) -> Option<Pixbuf> {
    let loader = PixbufLoader::new();
    let written = loader.write(bytes).and_then(|()| loader.close());
    written.ok().and(loader.pixbuf())
}

fn to_jpeg(bytes: &[u8], quality: u8) -> Option<Vec<u8>> {
    load(bytes)?.save_to_bufferv("jpeg", &[("quality", &quality.to_string())]).ok()
}

/// Why a PNG stays a PNG, in the client's words (`ConversionFailureReason`).
#[derive(Debug, PartialEq, Eq)]
enum Skip {
    Corrupted,
    Animated,
    Transparent,
    Indexed,
    Profile,
    Failed,
    PixelsDiffer,
    Larger,
}

/// A PNG made lossless WebP (`maybeConvertToWebP`): not if it is animated,
/// really transparent, indexed or in a profile other than sRGB, or if the
/// WebP differs in a pixel or is larger. The client's canvas encoder at
/// quality 1 is lossless, and compares the pixels to be sure; so does this.
fn to_webp(png: &[u8]) -> Option<Vec<u8>> {
    convert_png(png).ok()
}

fn convert_png(png: &[u8]) -> Result<Vec<u8>, Skip> {
    let header = Header::parse(png).ok_or(Skip::Corrupted)?;
    if header.animated {
        return Err(Skip::Animated);
    }
    let pixbuf = load(png).ok_or(Skip::Failed)?;
    // Only gray with alpha and RGBA count; a color key (`tRNS`) doesn't.
    if matches!(header.color_type, 4 | 6) && has_transparency(&pixbuf) {
        return Err(Skip::Transparent);
    }
    if header.color_type == 3 && header.depth <= 8 {
        return Err(Skip::Indexed);
    }
    if !header.icc.is_none_or(|(method, profile)| srgb_profile(method, profile)) {
        return Err(Skip::Profile);
    }
    let webp = pixbuf.save_to_bufferv("webp", &[("lossless", "true")]).map_err(|_| Skip::Failed)?;
    let again = load(&webp).ok_or(Skip::Failed)?;
    if rgba(&pixbuf) != rgba(&again) {
        return Err(Skip::PixelsDiffer);
    }
    if webp.len() > png.len() {
        return Err(Skip::Larger);
    }
    Ok(webp)
}

fn has_transparency(pixbuf: &Pixbuf) -> bool {
    pixbuf.has_alpha() && rgba(pixbuf).as_chunks::<4>().0.iter().any(|pixel| pixel[3] < 255)
}

/// The pixels as 8-bit RGBA, without the padding of each row.
fn rgba(pixbuf: &Pixbuf) -> Vec<u8> {
    let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
    let (channels, stride) = (pixbuf.n_channels() as usize, pixbuf.rowstride() as usize);
    let bytes = pixbuf.read_pixel_bytes();
    let mut out = Vec::with_capacity(width * height * 4);
    for row in bytes.chunks(stride).take(height) {
        for pixel in row[..width * channels].chunks_exact(channels) {
            out.extend_from_slice(&pixel[..3.min(channels)]);
            // Opaque when there is no alpha.
            out.push(if channels == 4 { pixel[3] } else { 255 });
        }
    }
    out
}

/// What the client reads of a PNG before deciding (UPNG and exifr in 673060).
struct Header<'a> {
    color_type: u8,
    depth: u8,
    /// Has an `acTL` chunk.
    animated: bool,
    /// The first `iCCP` chunk: its compression method and compressed profile.
    icc: Option<(u8, &'a [u8])>,
}

impl<'a> Header<'a> {
    fn parse(png: &'a [u8]) -> Option<Header<'a>> {
        let mut chunks = png.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
        let mut header = None;
        let (mut animated, mut icc) = (false, None);
        while chunks.len() >= 8 {
            let length = u32::from_be_bytes(chunks[..4].try_into().ok()?) as usize;
            let kind = &chunks[4..8];
            let data = chunks.get(8..8 + length)?;
            match kind {
                b"IHDR" if header.is_none() && length == 13 => header = Some((data[9], data[8])),
                // `IHDR` comes first, or there is no PNG.
                _ if header.is_none() => return None,
                b"acTL" => animated = true,
                b"iCCP" if icc.is_none() => {
                    let name = data.iter().position(|&b| b == 0)?;
                    icc = Some((*data.get(name + 1)?, data.get(name + 2..)?));
                }
                b"IEND" => break,
                _ => {}
            }
            chunks = chunks.get(12 + length..).unwrap_or_default();
        }
        let (color_type, depth) = header?;
        Some(Header { color_type, depth, animated, icc })
    }
}

/// Whether a PNG's profile says sRGB, as the client reads it (exifr in
/// 499091): the description tag must be there and name sRGB. A profile that
/// can't be read, or has no description, isn't taken for sRGB.
fn srgb_profile(method: u8, compressed: &[u8]) -> bool {
    let description = (method == 0).then(|| inflate(compressed)).flatten().and_then(|icc| profile_description(&icc));
    description.is_some_and(|text| text.to_lowercase().contains("srgb"))
}

/// A zlib stream, up to what any profile needs.
fn inflate(compressed: &[u8]) -> Option<Vec<u8>> {
    const MAX: usize = 16 * 1024 * 1024;
    let zlib = gio::ZlibDecompressor::new(gio::ZlibCompressorFormat::Zlib);
    let (mut input, mut out, mut chunk) = (compressed, Vec::new(), [0; 16 * 1024]);
    loop {
        let (result, read, written) = zlib.convert(input, &mut chunk, gio::ConverterFlags::INPUT_AT_END).ok()?;
        out.extend_from_slice(&chunk[..written]);
        input = &input[read..];
        match result {
            gio::ConverterResult::Finished => return Some(out),
            _ if (read == 0 && written == 0) || out.len() > MAX => return None,
            _ => {}
        }
    }
}

/// The text of an ICC profile's `desc` tag: the last one if there are
/// several, in any of the three forms profiles write it in. A profile whose
/// header doesn't hold together has none.
fn profile_description(icc: &[u8]) -> Option<String> {
    let u32_at = |at: usize| icc.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
    if u32_at(0)? != icc.len() || icc.len() < 84 || icc.get(36..40)? != b"acsp" {
        return None;
    }
    let mut found = None;
    for tag in 0..u32_at(128)? {
        let entry = 132 + tag * 12;
        let (Some(signature), Some(offset), Some(size)) =
            (icc.get(entry..entry + 4), u32_at(entry + 4), u32_at(entry + 8))
        else {
            break;
        };
        if offset > icc.len() {
            break;
        }
        if signature == b"desc" {
            found = tag_text(icc, offset, size).or(found);
        }
    }
    found
}

/// The text of the tag at `offset`: `desc`, `mluc` with one language, or
/// `text`. Several languages make no single text, which the client can't
/// compare either.
fn tag_text(icc: &[u8], offset: usize, size: usize) -> Option<String> {
    let u32_at = |at: usize| icc.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
    let latin =
        |from: usize, to: usize| icc.get(from..to).map(|b| b.iter().map(|&c| char::from(c)).collect::<String>());
    match icc.get(offset..offset + 4)? {
        b"desc" => {
            let length = u32_at(offset + 8)?;
            if length > size {
                return None;
            }
            latin(offset + 12, offset + length + 11)
        }
        b"mluc" => {
            if u32_at(offset + 8)? != 1 {
                return None;
            }
            let (length, at) = (u32_at(offset + 20)?, offset + u32_at(offset + 24)?);
            let units: Vec<u16> =
                icc.get(at..at + length)?.as_chunks::<2>().0.iter().map(|&b| u16::from_be_bytes(b)).collect();
            let units = units.strip_suffix(&[0]).unwrap_or(&units);
            Some(String::from_utf16_lossy(units))
        }
        b"text" => latin(offset + 8, (offset + size).checked_sub(7)?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, content_type: &str) -> File {
        File {
            filename: name.into(),
            size: 10,
            content_type: content_type.into(),
            data: Data::Memory(glib::Bytes::from_static(b"0123456789")),
            original_md5: None,
        }
    }

    #[test]
    fn heic_and_jpeg_xr_are_found_by_type_or_extension() {
        let source = |name, kind| Source::of(&file(name, kind));
        assert_eq!(source("a.heic", ""), Some(Source::Heic));
        assert_eq!(source("A.HEIF", "image/png"), Some(Source::Heic), "the extension is enough, in any case");
        assert_eq!(source("a.hif", ""), Some(Source::Heic));
        assert_eq!(source("a", "image/heic-sequence"), Some(Source::Heic));
        assert_eq!(source("a", "image/heif-sequence"), Some(Source::Heic));
        assert_eq!(source("a.wdp", ""), Some(Source::Jxr));
        assert_eq!(source("a.HDP", ""), Some(Source::Jxr));
        assert_eq!(source("a", "image/vnd.ms-photo"), Some(Source::Jxr));
        assert_eq!(source("a.jxr", "image/jxr"), Some(Source::Jxr));
        assert_eq!(source("a.heic.png", "image/png"), None, "only the last extension");
        assert_eq!(source("heic", ""), None, "a name without a dot has no extension");
        assert_eq!(source("a.jxl", "image/jxl"), None);
    }

    #[test]
    fn the_jpeg_takes_the_name() {
        assert_eq!(jpeg_name("IMG_1.HEIC"), "IMG_1.jpg");
        assert_eq!(jpeg_name("a.b.heif"), "a.b.jpg");
        assert_eq!(jpeg_name("scan.wdp"), "scan.jpg");
        assert_eq!(jpeg_name("x.hif"), "x.jpg");
        // Typed as HEIC without the extension: the client adds one.
        assert_eq!(jpeg_name("photo"), "photo.jpg");
        assert_eq!(jpeg_name("photo.png"), "photo.png.jpg");
    }

    #[test]
    fn the_original_type_is_filled_in_for_a_file_the_system_doesnt_know() {
        let heic = |name, kind| Source::Heic.original_type(&file(name, kind));
        assert_eq!(heic("a.HEIC", ""), "image/heic");
        assert_eq!(heic("a.heif", ""), "image/heif");
        assert_eq!(heic("a.hif", ""), "image/heif");
        assert_eq!(heic("a.heic", "image/heif"), "image/heic", "gio's image/heif for it is not what the client sends");
        assert_eq!(heic("a.hif", "image/heif-sequence"), "image/heif-sequence", "any other known type stays");
        assert_eq!(Source::Jxr.original_type(&file("a.jxr", "")), "image/jxr");
        assert_eq!(Source::Jxr.original_type(&file("a.wdp", "image/vnd.ms-photo")), "image/vnd.ms-photo");
    }

    #[test]
    fn jpeg_settings_are_the_clients() {
        assert_eq!((Source::Heic.quality(), Source::Heic.max_size()), (80, 20 * MIB));
        assert_eq!((Source::Jxr.quality(), Source::Jxr.max_size()), (85, 50 * MIB));
    }

    #[test]
    fn the_md5_is_hex() {
        assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    /// Big-endian `u32`s and bytes, laid out as a profile and its tags are.
    fn profile(tags: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut icc = vec![0; 128];
        icc.extend((tags.len() as u32).to_be_bytes());
        let mut offset = 132 + 12 * tags.len();
        let mut data: Vec<u8> = Vec::new();
        for (signature, body) in tags {
            icc.extend(*signature);
            icc.extend((offset as u32).to_be_bytes());
            icc.extend((body.len() as u32).to_be_bytes());
            offset += body.len();
            data.extend(body);
        }
        icc.extend(data);
        icc[36..40].copy_from_slice(b"acsp");
        let length = icc.len() as u32;
        icc[..4].copy_from_slice(&length.to_be_bytes());
        icc
    }

    fn desc(text: &str) -> Vec<u8> {
        let mut tag = b"desc\0\0\0\0".to_vec();
        tag.extend((text.len() as u32 + 1).to_be_bytes());
        tag.extend(text.as_bytes());
        tag.push(0);
        tag
    }

    fn mluc(texts: &[&str]) -> Vec<u8> {
        let mut tag = b"mluc\0\0\0\0".to_vec();
        tag.extend((texts.len() as u32).to_be_bytes());
        tag.extend(12u32.to_be_bytes());
        let mut at = 16 + 12 * texts.len();
        let mut data: Vec<u8> = Vec::new();
        for text in texts {
            let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            tag.extend(*b"enUS");
            tag.extend((units.len() as u32).to_be_bytes());
            tag.extend((at as u32).to_be_bytes());
            at += units.len();
            data.extend(units);
        }
        tag.extend(data);
        tag
    }

    #[test]
    fn a_profile_must_say_srgb_in_its_description() {
        let named = |tag: Vec<u8>| profile_description(&profile(&[(b"desc", tag)]));
        assert_eq!(named(desc("sRGB IEC61966-2.1")).as_deref(), Some("sRGB IEC61966-2.1"));
        assert_eq!(named(mluc(&["Display P3"])).as_deref(), Some("Display P3"));
        assert_eq!(named(mluc(&["a", "b"])), None, "two languages are no single text");
        assert_eq!(profile_description(&profile(&[(b"cprt", desc("sRGB"))])), None, "only `desc` counts");
        // The last `desc` wins.
        assert_eq!(
            profile_description(&profile(&[(b"desc", desc("sRGB")), (b"desc", desc("Adobe RGB"))])).as_deref(),
            Some("Adobe RGB")
        );
        // A header that doesn't hold together has no description.
        let mut short = profile(&[(b"desc", desc("sRGB"))]);
        short.push(0);
        assert_eq!(profile_description(&short), None, "the length field disagrees");
        let mut unsigned = profile(&[(b"desc", desc("sRGB"))]);
        unsigned[36..40].copy_from_slice(b"nope");
        assert_eq!(profile_description(&unsigned), None);
    }

    /// A PNG chunk, with the CRC libpng wants.
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut crc = !0u32;
        for &byte in kind.iter().chain(data) {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(data);
        out.extend((!crc).to_be_bytes());
        out
    }

    /// A zlib stream that stores `data` as it is.
    fn stored(data: &[u8]) -> Vec<u8> {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        let mut out = vec![0x78, 0x01, 1];
        out.extend((data.len() as u16).to_le_bytes());
        out.extend((!(data.len() as u16)).to_le_bytes());
        out.extend(data);
        out.extend(((b << 16) | a).to_be_bytes());
        out
    }

    /// A PNG of `size`×`size` pixels, all `pixel`, with `extra` chunks before
    /// the data. Its data is stored, not compressed.
    fn sized_png(size: usize, color_type: u8, pixel: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = (size as u32).to_be_bytes().to_vec();
        header.extend((size as u32).to_be_bytes());
        header.extend([8, color_type, 0, 0, 0]);
        out.extend(chunk(b"IHDR", &header));
        for extra in extra {
            out.extend(extra);
        }
        // Each row is led by its filter byte.
        let row: Vec<u8> = std::iter::once(0).chain(pixel.iter().copied().cycle().take(pixel.len() * size)).collect();
        out.extend(chunk(b"IDAT", &stored(&row.repeat(size))));
        out.extend(chunk(b"IEND", b""));
        out
    }

    /// One that a WebP is smaller than.
    fn png(color_type: u8, pixel: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
        sized_png(64, color_type, pixel, extra)
    }

    fn iccp(method: u8, profile: &[u8]) -> Vec<u8> {
        let mut data = b"p\0".to_vec();
        data.push(method);
        data.extend(stored(profile));
        chunk(b"iCCP", &data)
    }

    #[test]
    fn reads_what_the_client_reads_of_a_png() {
        let plain = png(2, &[1, 2, 3], &[]);
        let header = Header::parse(&plain).unwrap();
        assert_eq!((header.color_type, header.depth, header.animated, header.icc), (2, 8, false, None));
        let animated = png(2, &[1, 2, 3], &[chunk(b"acTL", &[0; 8])]);
        assert!(Header::parse(&animated).unwrap().animated);
        let profiled = png(2, &[1, 2, 3], &[iccp(0, b"x")]);
        let icc = Header::parse(&profiled).unwrap().icc.unwrap();
        assert_eq!(icc.0, 0);
        assert!(Header::parse(b"not a png").is_none());
        assert!(Header::parse(&plain[..50]).is_none(), "cut short");
        // The header comes first.
        let mut late = b"\x89PNG\r\n\x1a\n".to_vec();
        late.extend(chunk(b"acTL", &[0; 8]));
        assert!(Header::parse(&late).is_none());
    }

    /// `cargo test` runs where gdk-pixbuf may have no loaders to run (a
    /// sandbox that forbids them); what needs them then proves nothing, so
    /// it says so and stops.
    fn loaders() -> bool {
        let works = load(&png(2, &[1, 2, 3], &[])).is_some();
        if !works {
            eprintln!("gdk-pixbuf can't load a PNG here; skipping");
        }
        works
    }

    fn srgb() -> Vec<u8> {
        profile(&[(b"desc", desc("sRGB IEC61966-2.1"))])
    }

    #[test]
    fn a_plain_png_becomes_the_same_pixels_in_webp() {
        if !loaders() {
            return;
        }
        let source = png(2, &[10, 20, 30], &[]);
        let webp = convert_png(&source).expect("converts");
        assert_eq!(&webp[8..12], b"WEBP");
        assert_eq!(&webp[12..16], b"VP8L", "lossless");
        assert_eq!(rgba(&load(&webp).unwrap()), rgba(&load(&source).unwrap()));
    }

    #[test]
    fn a_webp_larger_than_the_png_isnt_sent() {
        if !loaders() {
            return;
        }
        // WebP's own overhead outweighs 2×2 pixels the PNG holds in a few bytes.
        let tiny = sized_png(2, 2, &[10, 20, 30], &[]);
        assert_eq!(convert_png(&tiny).unwrap_err(), Skip::Larger);
    }

    #[test]
    fn what_the_client_leaves_alone_stays_a_png() {
        if !loaders() {
            return;
        }
        let skip = |bytes: &[u8]| convert_png(bytes).unwrap_err();
        assert_eq!(skip(b"RIFF....WEBP"), Skip::Corrupted);
        assert_eq!(skip(&png(2, &[1, 2, 3], &[chunk(b"acTL", &[0; 8])])), Skip::Animated);
        // Real transparency, but not a pixel that is merely 255 or a color key.
        assert_eq!(skip(&png(6, &[1, 2, 3, 254], &[])), Skip::Transparent);
        assert_eq!(skip(&png(4, &[9, 128], &[])), Skip::Transparent);
        // Indexed, whatever its depth.
        assert_eq!(skip(&png(3, &[0], &[chunk(b"PLTE", &[1, 2, 3])])), Skip::Indexed);
        // Another profile, or one with no word of sRGB, or one gone bad.
        let adobe = profile(&[(b"desc", desc("Adobe RGB (1998)"))]);
        assert_eq!(skip(&png(2, &[1, 2, 3], &[iccp(0, &adobe)])), Skip::Profile);
        assert_eq!(skip(&png(2, &[1, 2, 3], &[iccp(0, &profile(&[]))])), Skip::Profile, "no description");
        assert_eq!(skip(&png(2, &[1, 2, 3], &[iccp(1, &srgb())])), Skip::Profile, "method 1 isn't defined");
        assert_eq!(skip(&png(2, &[1, 2, 3], &[chunk(b"iCCP", b"p\0\0garbage")])), Skip::Profile);
    }

    #[test]
    fn a_png_in_srgb_or_with_opaque_alpha_is_converted() {
        if !loaders() {
            return;
        }
        assert!(convert_png(&png(2, &[10, 20, 30], &[iccp(0, &srgb())])).is_ok());
        // Alpha that is 255 everywhere is no transparency.
        assert!(convert_png(&png(6, &[10, 20, 30, 255], &[])).is_ok());
    }

    #[test]
    fn jpeg_is_made_at_the_given_quality() {
        if !loaders() {
            return;
        }
        let source = png(2, &[10, 20, 30], &[]);
        let jpeg = to_jpeg(&source, 80).expect("encodes");
        assert_eq!(&jpeg[..3], b"\xff\xd8\xff");
        assert!(to_jpeg(b"junk", 80).is_none(), "undecodable stays as it was");
    }

    fn heic() -> Vec<u8> {
        include_bytes!("../testdata/gradient.heic").to_vec()
    }

    fn memory(name: &str, content_type: &str, bytes: Vec<u8>) -> File {
        File {
            filename: name.into(),
            size: bytes.len() as u64,
            content_type: content_type.into(),
            data: Data::Memory(glib::Bytes::from_owned(bytes)),
            original_md5: None,
        }
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        glib::MainContext::new().block_on(future)
    }

    #[test]
    fn a_heic_file_becomes_a_jpeg_with_its_originals_hash() {
        if load(&heic()).is_none() {
            return eprintln!("gdk-pixbuf can't load a HEIC here; skipping");
        }
        let original = heic();
        let hash = md5(&original);
        let converted = run(prepare(memory("IMG_0001.HEIC", "", original)));
        assert_eq!(converted.filename, "IMG_0001.jpg");
        assert_eq!(converted.content_type, "image/heic", "the upload still names the original's type");
        assert_eq!(converted.original_md5.as_deref(), Some(hash.as_str()));
        let Data::Memory(bytes) = &converted.data else { panic!("the JPEG is in memory") };
        assert_eq!(&bytes[..3], b"\xff\xd8\xff");
        assert_eq!(converted.size, bytes.len() as u64);
    }

    #[test]
    fn a_heic_file_that_cant_be_converted_goes_as_it_is() {
        // Not a picture, then too big: the same file, but typed as the client types it.
        let junk = run(prepare(memory("a.heif", "", b"not an image".to_vec())));
        assert_eq!(
            (junk.filename.as_str(), junk.content_type.as_str(), junk.original_md5),
            ("a.heif", "image/heif", None)
        );
        let mut big = memory("big.heic", "image/heic", heic());
        big.size = 20 * MIB + 1;
        let kept = run(prepare(big));
        assert_eq!((kept.filename.as_str(), kept.size, kept.original_md5), ("big.heic", 20 * MIB + 1, None));
        // JPEG XR has no decoder here: it takes the same path.
        let jxr = run(prepare(memory("a.jxr", "", b"II\xbc\x01".to_vec())));
        assert_eq!((jxr.filename.as_str(), jxr.content_type.as_str(), jxr.original_md5), ("a.jxr", "image/jxr", None));
        // Other files are not touched.
        let png = memory("a.png", "image/png", vec![1, 2, 3]);
        assert_eq!(run(prepare(png.clone())).size, png.size);
    }

    #[test]
    fn a_small_png_becomes_a_webp_under_its_own_name() {
        if !loaders() {
            return;
        }
        let source = png(2, &[10, 20, 30], &[]);
        let file = memory("shot.png", "image/png", source.clone());
        let webp = run(optimize(&file, 512 * 1024)).expect("converted");
        assert_eq!(webp.filename, "shot.png", "the name stays");
        assert_eq!(webp.content_type, "image/png", "and so does the original's type");
        let Data::Memory(bytes) = &webp.data else { panic!("the WebP is in memory") };
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(webp.size, bytes.len() as u64);
        assert!(webp.size < file.size);
        // Over the variant's size, another type or already smaller: untouched.
        assert!(run(optimize(&file, file.size - 1)).is_none());
        assert!(run(optimize(&File { content_type: "image/jpeg".into(), ..file.clone() }, MIB)).is_none());
        assert!(run(optimize(&File { content_type: "image/webp".into(), ..file }, MIB)).is_none());
    }
}
