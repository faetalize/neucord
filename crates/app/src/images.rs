//! Async image loading with memory and disk caches, for avatars, guild
//! icons, custom emoji, inline attachments and embeds, plus the files videos
//! and sound play from.
//!
//! Encoded bytes are fetched once (memory → disk → network). Still images
//! decode once per URL into a shared `gdk::Texture`, so rows rebuilt while
//! scrolling show them at once. Animated ones show such a still until the
//! rules of [`discord::animation`] let them move; they then decode (once per
//! URL while anyone shows them) into a frame-stepping paintable that ticks
//! only while it is playing and in view (GTK's `Picture` shows only the first
//! frame otherwise). Images from Discord's media proxy are requested at the
//! size they are shown, which keeps both downloads and decodes small.
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
mod play;
pub use animated::Lease;
use animated::{AnimatedPaintable, FramePaintable};
use discord::animation::Subject;
use lru::Lru;
pub use play::{Motion, Row, Update};

use crate::rich_text::RichText;

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
    /// Decoded animations by URL, while some widget still holds one.
    moving: RefCell<HashMap<String, glib::WeakRef<gdk::Paintable>>>,
    motion: Motion,
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
            moving: RefCell::default(),
            motion: Motion::new(),
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

    /// Fetch and set an image onto `picture`, as a still: the first frame of
    /// an animation. If the picture is dropped before the bytes arrive,
    /// nothing happens.
    pub fn set(&self, picture: &gtk::Picture, url: &str) {
        let weak = picture.downgrade();
        self.load(url, move |paintable| {
            if let Some(picture) = weak.upgrade() {
                picture.set_paintable(Some(&paintable));
            }
        });
    }

    /// Fetch an image and hand it to `done` as a still: at once if it is
    /// decoded already, else once it has loaded, and never if it fails to.
    pub fn load(&self, url: &str, done: impl FnOnce(gdk::Paintable) + 'static) {
        if let Some(texture) = self.0.textures.borrow_mut().get(url) {
            return done(texture.upcast());
        }
        let url = url.to_owned();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Some(bytes) = this.load_bytes(&url).await else { return };
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

    /// [`load`](Self::load) for media shown without the rules for moving
    /// ones, such as in the viewer: an animation arrives with the lease that
    /// keeps its frames advancing for as long as it is held, and one that
    /// doesn't decode arrives as its still.
    pub fn load_playing(&self, url: &str, animated: bool, done: impl FnOnce(gdk::Paintable, Option<Lease>) + 'static) {
        if !animated {
            return self.load(url, |paintable| done(paintable, None));
        }
        let (this, url) = (self.clone(), url.to_owned());
        glib::spawn_future_local(async move {
            match this.moving(&url).await {
                Some(paintable) => {
                    let lease = animated::play(&paintable);
                    done(paintable, lease);
                }
                None => this.load(&url, |paintable| done(paintable, None)),
            }
        });
    }

    /// Like [`set`](Self::set), but asks Discord's media proxy for the image
    /// at the size it is `shown` at most, never above `original`. Other URLs
    /// load as they are.
    pub fn set_sized(&self, picture: &gtk::Picture, url: &str, shown: (i32, i32), original: Option<(u32, u32)>) {
        self.set(picture, &self.sized(url, shown, original));
    }

    /// [`set_moving`](Self::set_moving) for an image whose still is its own
    /// first frame, sized as [`set_sized`](Self::set_sized) does.
    pub fn set_sized_moving(
        &self,
        picture: &gtk::Picture,
        url: &str,
        shown: (i32, i32),
        original: Option<(u32, u32)>,
        subject: Subject,
        row: Row,
    ) {
        let url = self.sized(url, shown, original);
        self.set_moving(picture, &url, &url, subject, row);
    }

    fn sized(&self, url: &str, shown: (i32, i32), original: Option<(u32, u32)>) -> String {
        let (width, height) = request_size(shown, self.0.scale.get(), original);
        discord::cdn::proxy_sized(url, width, height)
    }

    /// Call `on_update` with where `widget`'s media stands under the rules
    /// for `subject`, now and whenever that changes: for media that isn't a
    /// picture, such as a video.
    pub fn watch(
        &self,
        widget: &impl IsA<gtk::Widget>,
        subject: Subject,
        row: Row,
        on_update: impl Fn(Update) + 'static,
    ) {
        play::attach(&self.0.motion, widget, subject, row, 0, on_update);
    }

    /// What the window, the pointer and the account's settings say about
    /// moving media.
    pub fn motion(&self) -> &Motion {
        &self.0.motion
    }

    /// Show `still` in `picture`, and the animation at `moving` for as long
    /// as the rules for `subject` say it should move: the still comes back
    /// when they stop, and the frames advance only while the picture is in
    /// view and the window has focus. `row` says whose pointer counts as the
    /// pointer on it besides its own, where the rules allow.
    pub fn set_moving(&self, picture: &gtk::Picture, still: &str, moving: &str, subject: Subject, row: Row) {
        #[derive(Default)]
        struct Shown {
            paintable: RefCell<Option<gdk::Paintable>>,
            lease: RefCell<Option<Lease>>,
            update: Cell<Option<play::Update>>,
            loading: Cell<bool>,
        }
        let shown = Rc::new(Shown::default());
        let (this, weak) = (self.clone(), picture.downgrade());
        let (still, moving) = (still.to_owned(), moving.to_owned());
        play::attach(&self.0.motion, picture, subject, row, 0, move |update| {
            let Some(picture) = weak.upgrade() else { return };
            shown.update.set(Some(update));
            if !update.moving {
                *shown.lease.borrow_mut() = None;
                if shown.paintable.take().is_some() || picture.paintable().is_none() {
                    this.set(&picture, &still);
                }
                return;
            }
            let held = shown.paintable.borrow().clone();
            match held {
                Some(paintable) => {
                    *shown.lease.borrow_mut() = update.plays.then(|| animated::play(&paintable)).flatten();
                }
                None if !shown.loading.replace(true) => {
                    let (this, shown, weak, url) = (this.clone(), shown.clone(), weak.clone(), moving.clone());
                    glib::spawn_future_local(async move {
                        let paintable = this.moving(&url).await;
                        shown.loading.set(false);
                        let (Some(picture), Some(paintable)) = (weak.upgrade(), paintable) else { return };
                        // It may have stopped moving while it loaded.
                        let Some(update) = shown.update.get().filter(|u| u.moving) else { return };
                        picture.set_paintable(Some(&paintable));
                        *shown.lease.borrow_mut() = update.plays.then(|| animated::play(&paintable)).flatten();
                        *shown.paintable.borrow_mut() = Some(paintable);
                    });
                }
                None => {}
            }
        });
    }

    /// The custom emoji of `text` (the `n`th image of it, its still and its
    /// animation if it has one), which move as one under the rules for
    /// emoji. Animated ones take places from the budget of how many play at
    /// once.
    pub fn set_emoji(&self, text: &RichText, emoji: Vec<(usize, String, Option<String>)>) {
        #[derive(Default)]
        struct Slot {
            still: RefCell<Option<gdk::Paintable>>,
            moving: RefCell<Option<gdk::Paintable>>,
            lease: RefCell<Option<Lease>>,
            loading: Cell<bool>,
        }
        let mut animated = Vec::new();
        for (n, still, moving) in emoji {
            let slot = Rc::new(Slot::default());
            let weak = text.downgrade();
            self.load(&still, {
                let slot = slot.clone();
                move |paintable| {
                    // The still may arrive after the animation has been put in.
                    *slot.still.borrow_mut() = Some(paintable.clone());
                    if let Some(text) = weak.upgrade()
                        && slot.moving.borrow().is_none()
                    {
                        text.set_image(n, paintable);
                    }
                }
            });
            if let Some(moving) = moving {
                animated.push((n, slot, moving));
            }
        }
        if animated.is_empty() {
            return;
        }
        let update = Rc::new(Cell::new(None::<play::Update>));
        let (this, weak) = (self.clone(), text.downgrade());
        let count = animated.len() as u32;
        let animated = Rc::new(animated);
        play::attach(&self.0.motion, text, Subject::Emoji, Row::Parent, count, move |now| {
            let Some(text) = weak.upgrade() else { return };
            update.set(Some(now));
            for (n, slot, url) in animated.iter() {
                let n = *n;
                if !now.moving {
                    *slot.lease.borrow_mut() = None;
                    if slot.moving.take().is_some()
                        && let Some(still) = slot.still.borrow().clone()
                    {
                        text.set_image(n, still);
                    }
                    continue;
                }
                let held = slot.moving.borrow().clone();
                match held {
                    Some(paintable) => {
                        *slot.lease.borrow_mut() = now.plays.then(|| animated::play(&paintable)).flatten();
                    }
                    None if !slot.loading.replace(true) => {
                        let (this, slot, weak, url, update) =
                            (this.clone(), slot.clone(), weak.clone(), url.clone(), update.clone());
                        glib::spawn_future_local(async move {
                            let paintable = this.moving(&url).await;
                            slot.loading.set(false);
                            let (Some(text), Some(paintable)) = (weak.upgrade(), paintable) else { return };
                            let Some(now) = update.get().filter(|u| u.moving) else { return };
                            text.set_image(n, paintable.clone());
                            *slot.lease.borrow_mut() = now.plays.then(|| animated::play(&paintable)).flatten();
                            *slot.moving.borrow_mut() = Some(paintable);
                        });
                    }
                    None => {}
                }
            }
        });
    }

    /// An animation decoded, shared by every widget that shows it at once.
    async fn moving(&self, url: &str) -> Option<gdk::Paintable> {
        let held = |this: &Self| this.0.moving.borrow().get(url).and_then(|weak| weak.upgrade());
        if let Some(paintable) = held(self) {
            return Some(paintable);
        }
        let bytes = self.load_bytes(url).await?;
        // Another widget may have decoded it while we waited.
        if let Some(paintable) = held(self) {
            return Some(paintable);
        }
        let paintable = apng_paintable(&bytes).or_else(|| animated_paintable(&bytes))?;
        let mut moving = self.0.moving.borrow_mut();
        moving.retain(|_, weak| weak.upgrade().is_some());
        moving.insert(url.to_owned(), paintable.downgrade());
        Some(paintable)
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

    /// A file's bytes, cached nowhere: for saving and copying, which want
    /// the original once.
    pub async fn fetch(&self, url: &str) -> Option<glib::Bytes> {
        self.download(url).await
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

/// An APNG's frames, which gdk-pixbuf's loader doesn't have.
fn apng_paintable(bytes: &glib::Bytes) -> Option<gdk::Paintable> {
    if !discord::apng::is_apng(bytes) {
        return None;
    }
    let animation = discord::apng::decode(bytes)?;
    let (width, height) = (animation.width as i32, animation.height as i32);
    let frames = animation
        .frames
        .into_iter()
        .map(|frame| {
            let pixels = glib::Bytes::from_owned(frame.rgba);
            let texture =
                gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::R8g8b8a8, &pixels, width as usize * 4);
            (texture.upcast(), std::time::Duration::from_millis(u64::from(frame.delay_ms)))
        })
        .collect();
    Some(FramePaintable::new(frames, animation.plays).upcast())
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
