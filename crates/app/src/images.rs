//! Async image loading with a memory + disk cache, for avatars, guild icons,
//! custom emoji and inline attachments.
//!
//! Encoded bytes are fetched once (memory → disk → network) and decoded per
//! widget: static formats via `gdk::Texture`, GIFs via a frame-stepping
//! paintable so they animate (GTK's `Picture` shows only the first frame
//! otherwise).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gdk;
use gtk::gdk_pixbuf::{PixbufAnimation, PixbufLoader};
use gtk::glib;
use gtk::prelude::*;
use soup::prelude::*;

mod animated;
pub use animated::AnimatedPaintable;

/// Shared image cache and fetcher. Clone-cheap (`Rc` inside).
#[derive(Clone)]
pub struct Images(Rc<Inner>);

struct Inner {
    session: soup::Session,
    cache_dir: PathBuf,
    /// Encoded bytes by URL. In-flight loads are deduplicated by the pending map.
    memory: RefCell<HashMap<String, glib::Bytes>>,
    /// URLs currently being fetched → wakers waiting for the bytes.
    pending: RefCell<HashMap<String, Vec<async_channel::Sender<Option<glib::Bytes>>>>>,
}

impl Images {
    pub fn new() -> Self {
        let session = soup::Session::new();
        session.set_user_agent(&format!("Neucord/{}", env!("CARGO_PKG_VERSION")));
        let cache_dir = glib::user_cache_dir().join("neucord").join("images");
        let _ = std::fs::create_dir_all(&cache_dir);
        Images(Rc::new(Inner {
            session,
            cache_dir,
            memory: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashMap::new()),
        }))
    }

    /// Fetch and set an image onto `picture`. `animated` selects the GIF path.
    /// If the picture is dropped before the bytes arrive, nothing happens.
    pub fn set(&self, picture: &gtk::Picture, url: &str, animated: bool) {
        let weak = picture.downgrade();
        let url = url.to_owned();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Some(bytes) = this.load_bytes(&url).await else { return };
            let Some(picture) = weak.upgrade() else { return };
            match paintable(&bytes, animated) {
                Some(paintable) => picture.set_paintable(Some(&paintable)),
                None => eprintln!("images: could not decode {url}"),
            }
        });
    }

    /// Encoded bytes for a URL: memory → disk → network, filling caches.
    async fn load_bytes(&self, url: &str) -> Option<glib::Bytes> {
        if let Some(bytes) = self.0.memory.borrow().get(url).cloned() {
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
            self.0.memory.borrow_mut().insert(url.to_owned(), bytes.clone());
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
        let message = soup::Message::new("GET", url).ok()?;
        let bytes = self
            .0
            .session
            .send_and_read_future(&message, glib::Priority::DEFAULT)
            .await
            .ok()?;
        if !(200..=299).contains(&message.status_code()) || bytes.is_empty() {
            return None;
        }
        let _ = std::fs::write(&path, &bytes);
        Some(bytes)
    }

    fn disk_path(&self, url: &str) -> PathBuf {
        // Hash the URL for a flat, filesystem-safe cache key.
        let digest = glib::compute_checksum_for_string(glib::ChecksumType::Sha256, url)
            .map(String::from)
            .unwrap_or_else(|| url.bytes().map(|b| format!("{b:02x}")).collect());
        self.0.cache_dir.join(digest)
    }
}

/// Decode encoded bytes into a paintable. GIFs become an animated paintable.
fn paintable(bytes: &glib::Bytes, animated: bool) -> Option<gdk::Paintable> {
    // Fall through to a static first frame if animation decoding failed.
    if animated && let Some(p) = animated_paintable(bytes) {
        return Some(p);
    }
    gdk::Texture::from_bytes(bytes).ok().map(|t| t.upcast())
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
