//! Async image loading with memory and disk caches, for avatars, guild
//! icons, custom emoji, inline attachments and embeds, plus the files videos
//! and sound play from.
//!
//! Encoded bytes are fetched once (memory → disk → network). Still images
//! decode once per URL into a shared `gdk::Texture`, so rows rebuilt while
//! scrolling show them at once. GIFs decode per widget into a frame-stepping
//! paintable so they animate (GTK's `Picture` shows only the first frame
//! otherwise). Images from Discord's media proxy are requested at the size
//! they are shown, which keeps both downloads and decodes small.
//!
//! The memory caches and the video files are bounded; the image files on
//! disk are not yet (#18).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gdk;
use gtk::gdk_pixbuf::{PixbufAnimation, PixbufLoader};
use gtk::glib;
use gtk::prelude::*;
use soup::prelude::*;

mod animated;
mod lru;
pub use animated::AnimatedPaintable;
use lru::Lru;

const MIB: u64 = 1024 * 1024;
/// Encoded bytes kept in memory, which saves rereading files.
const BYTES_BUDGET: u64 = 32 * MIB;
/// Decoded still images (4 bytes a pixel).
const TEXTURE_BUDGET: u64 = 96 * MIB;
/// Video files on disk.
const VIDEO_BUDGET: u64 = 256 * MIB;

/// Shared image cache and fetcher. Clone-cheap (`Rc` inside).
#[derive(Clone)]
pub struct Images(Rc<Inner>);

struct Inner {
    session: soup::Session,
    cache_dir: PathBuf,
    video_dir: PathBuf,
    /// Encoded bytes by URL. In-flight loads are deduplicated by the pending map.
    memory: RefCell<Lru<glib::Bytes>>,
    /// Decoded still images by URL.
    textures: RefCell<Lru<gdk::Texture>>,
    /// The window's scale factor, for asking the proxy for enough pixels.
    scale: Cell<i32>,
    /// URLs currently being fetched → wakers waiting for the bytes.
    pending: RefCell<HashMap<String, Vec<async_channel::Sender<Option<glib::Bytes>>>>>,
}

impl Images {
    pub fn new() -> Self {
        let session = soup::Session::new();
        session.set_user_agent(&format!("Neucord/{}", env!("CARGO_PKG_VERSION")));
        let cache = glib::user_cache_dir().join("neucord");
        let (cache_dir, video_dir) = (cache.join("images"), cache.join("videos"));
        let _ = std::fs::create_dir_all(&cache_dir);
        let _ = std::fs::create_dir_all(&video_dir);
        Images(Rc::new(Inner {
            session,
            cache_dir,
            video_dir,
            memory: RefCell::new(Lru::new(BYTES_BUDGET)),
            textures: RefCell::new(Lru::new(TEXTURE_BUDGET)),
            scale: Cell::new(1),
            pending: RefCell::new(HashMap::new()),
        }))
    }

    pub fn set_scale(&self, scale: i32) {
        self.0.scale.set(scale.max(1));
    }

    /// The window's scale factor.
    pub fn scale(&self) -> i32 {
        self.0.scale.get()
    }

    /// Fetch and set an image onto `picture`. `animated` selects the GIF path.
    /// If the picture is dropped before the bytes arrive, nothing happens.
    pub fn set(&self, picture: &gtk::Picture, url: &str, animated: bool) {
        let weak = picture.downgrade();
        self.load(url, animated, move |paintable| {
            if let Some(picture) = weak.upgrade() {
                picture.set_paintable(Some(&paintable));
            }
        });
    }

    /// Fetch an image and hand it to `done`: at once if it is decoded
    /// already, else once it has loaded, and never if it fails to.
    /// `animated` selects the GIF path.
    pub fn load(&self, url: &str, animated: bool, done: impl FnOnce(gdk::Paintable) + 'static) {
        if !animated && let Some(texture) = self.0.textures.borrow_mut().get(url) {
            return done(texture.upcast());
        }
        let url = url.to_owned();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Some(bytes) = this.load_bytes(&url).await else { return };
            // Fall through to a still first frame if animation decoding failed.
            if animated && let Some(paintable) = animated_paintable(&bytes) {
                return done(paintable);
            }
            // Another picture may have decoded it while we waited.
            let cached = this.0.textures.borrow_mut().get(&url);
            let texture = match cached.map_or_else(|| gdk::Texture::from_bytes(&bytes), Ok) {
                Ok(texture) => texture,
                Err(_) => return eprintln!("images: could not decode {url}"),
            };
            let size = texture.width() as u64 * texture.height() as u64 * 4;
            this.0.textures.borrow_mut().insert(&url, texture.clone(), size);
            done(texture.upcast());
        });
    }

    /// Like [`set`](Self::set), but asks Discord's media proxy for the image
    /// at the size it is `shown` at most, never above `original`. Other URLs
    /// load as they are.
    pub fn set_sized(
        &self,
        picture: &gtk::Picture,
        url: &str,
        shown: (i32, i32),
        original: Option<(u32, u32)>,
        animated: bool,
    ) {
        let (width, height) = request_size(shown, self.0.scale.get(), original);
        self.set(picture, &discord::cdn::proxy_sized(url, width, height), animated);
    }

    /// Encoded bytes for a URL: memory → disk → network, filling caches.
    async fn load_bytes(&self, url: &str) -> Option<glib::Bytes> {
        if let Some(bytes) = self.0.memory.borrow_mut().get(url) {
            return Some(bytes);
        }
        // If someone else is already fetching this URL, wait for their result.
        let (tx, rx) = async_channel::bounded(1);
        let already_fetching = {
            let mut pending = self.0.pending.borrow_mut();
            match pending.get_mut(url) {
                Some(waiters) => {
                    waiters.push(tx);
                    true
                }
                None => {
                    pending.insert(url.to_owned(), Vec::new());
                    false
                }
            }
        };
        if already_fetching {
            return rx.recv().await.ok().flatten();
        }

        let bytes = self.fetch_uncached(url).await;
        if let Some(bytes) = &bytes {
            self.0.memory.borrow_mut().insert(url, bytes.clone(), bytes.len() as u64);
        }
        // Wake any waiters that arrived while we fetched. Collect them before
        // awaiting so the RefCell borrow is not held across an await point.
        let waiters = self.0.pending.borrow_mut().remove(url).unwrap_or_default();
        for waiter in waiters {
            let _ = waiter.send(bytes.clone()).await;
        }
        bytes
    }

    async fn fetch_uncached(&self, url: &str) -> Option<glib::Bytes> {
        let path = self.disk_path(url);
        if let Ok(data) = std::fs::read(&path) {
            return Some(glib::Bytes::from_owned(data));
        }
        let bytes = self.download(url).await?;
        let _ = std::fs::write(&path, &bytes);
        Some(bytes)
    }

    /// A video or sound as a file GTK can open, kept on disk (within a budget, least
    /// recently played going first) and never in memory.
    pub async fn file(&self, url: &str) -> Option<PathBuf> {
        let path = self.0.video_dir.join(cache_key(url));
        if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            // Played again: the modification time stands for last use.
            let _ = file.set_modified(std::time::SystemTime::now());
            return Some(path);
        }
        let bytes = self.download(url).await?;
        std::fs::write(&path, &bytes).ok()?;
        self.prune_videos();
        Some(path)
    }

    fn prune_videos(&self) {
        let Ok(entries) = std::fs::read_dir(&self.0.video_dir) else { return };
        let files = entries
            .flatten()
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((e.path(), meta.len(), meta.modified().ok()?))
            })
            .collect();
        for path in lru::over_budget(files, VIDEO_BUDGET) {
            let _ = std::fs::remove_file(path);
        }
    }

    async fn download(&self, url: &str) -> Option<glib::Bytes> {
        let message = soup::Message::new("GET", url).ok()?;
        let bytes = self.0.session.send_and_read_future(&message, glib::Priority::DEFAULT).await.ok()?;
        ((200..=299).contains(&message.status_code()) && !bytes.is_empty()).then_some(bytes)
    }

    fn disk_path(&self, url: &str) -> PathBuf {
        self.0.cache_dir.join(cache_key(url))
    }
}

/// A flat, filesystem-safe name for a URL.
fn cache_key(url: &str) -> String {
    glib::compute_checksum_for_string(glib::ChecksumType::Sha256, url)
        .map(String::from)
        .unwrap_or_else(|| url.bytes().map(|b| format!("{b:02x}")).collect())
}

/// The pixel size to request for an image shown at `shown` (logical pixels):
/// enough for the screen's scale, but never more than the original has.
pub fn request_size(shown: (i32, i32), scale: i32, original: Option<(u32, u32)>) -> (u32, u32) {
    let scaled = |n: i32| (n.max(1) as u32) * scale.max(1) as u32;
    let (mut width, mut height) = (scaled(shown.0), scaled(shown.1));
    if let Some((w, h)) = original.filter(|&(w, h)| w > 0 && h > 0) {
        // Shrink both sides by the same factor, so the aspect ratio holds.
        let fit = (w as f64 / width as f64).min(h as f64 / height as f64).min(1.0);
        width = ((width as f64 * fit).round() as u32).max(1);
        height = ((height as f64 * fit).round() as u32).max(1);
    }
    (width, height)
}

fn animated_paintable(bytes: &glib::Bytes) -> Option<gdk::Paintable> {
    let loader = PixbufLoader::new();
    loader.write_bytes(bytes).ok()?;
    loader.close().ok()?;
    let animation: PixbufAnimation = loader.animation()?;
    if animation.is_static_image() {
        return None;
    }
    Some(AnimatedPaintable::new(animation).upcast())
}

#[cfg(test)]
mod tests {
    use super::request_size;

    #[test]
    fn requested_size_follows_scale_and_original() {
        assert_eq!(request_size((400, 300), 1, Some((4000, 3000))), (400, 300));
        assert_eq!(request_size((400, 300), 2, Some((4000, 3000))), (800, 600));
        assert_eq!(request_size((400, 300), 2, Some((600, 450))), (600, 450), "no upscaling");
        assert_eq!(request_size((80, 80), 1, None), (80, 80));
    }
}
