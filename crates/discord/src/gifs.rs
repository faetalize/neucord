//! The GIF picker's side that needs no window: what it asks Discord's GIF
//! endpoints (the client never talks to the provider itself), how it reads
//! the answers, and when it asks, which is the client's `GIFPickerViewStore`
//! (821102) and the actions around it (497685).
//!
//! The requests are made only when [`Picker`] says so, and each is built here
//! so that its query is the client's, field for field and in its order.

use serde_json::{Value, json};

use crate::cdn;
use crate::frecency::{FavoriteGif, GifFormat, MAX_FAVORITE_GIFS_BYTES, favorite_gifs_size};
use crate::identity::LOCALE;

/// The format every request names (`getSelectedFormat`, 821102): an animated
/// WebP on the desktop apps for Linux, WebM elsewhere. Neucord is a Linux app.
pub const MEDIA_FORMAT: &str = "tinywebp";

/// How long a search waits for more typing, as the client's debounce does.
pub const SEARCH_DEBOUNCE_MS: u32 = 250;

/// How many suggestions the client asks for.
const SUGGESTIONS: u8 = 5;

/// How long the GIF button pulses after a GIF is favourited, from any star:
/// each favourite restarts it (`er` in 4025).
pub const PULSE_MS: u32 = 2000;

/// Whether "Added to Favorites" shows on the GIF button while it pulses,
/// with the account holding `favorites` after the add: only for the first
/// two (`er` in 4025).
pub fn favorited_hint(favorites: usize) -> bool {
    favorites <= 2
}

/// The results list asks for suggestions once it has scrolled to within this
/// many pixels of its end, counted from the bottom of what is showing
/// (`handleScroll` in 884037: `scrollHeight - scrollTop <= 1180`).
pub const NEAR_END: f64 = 1180.0;

/// `encodeURIComponent`, which is how the client's HTTP layer writes a query
/// value: everything but letters, digits and ``-_.!~*'()`` is escaped.
fn component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A request the picker makes. Each has the query the client builds for it
/// (`k8`, `Z4`, `y` and `_E` in 497685), in the order its fields are written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The front page: categories and a preview of what's trending.
    Trending,
    /// Trending GIFs themselves.
    TrendingGifs,
    /// GIFs for a search, a category or a suggestion.
    Search(String),
    /// Searches to try instead, for what the results list is showing.
    Suggest(String),
}

impl Request {
    /// The endpoint's path after the API base, as in `endpoints.rs`.
    pub fn query(&self) -> String {
        match self {
            Request::Trending => format!("locale={LOCALE}&media_format={MEDIA_FORMAT}"),
            Request::TrendingGifs => format!("media_format={MEDIA_FORMAT}&locale={LOCALE}"),
            Request::Search(q) => format!("q={}&media_format={MEDIA_FORMAT}&locale={LOCALE}", component(q)),
            Request::Suggest(q) => format!("q={}&limit={SUGGESTIONS}&locale={LOCALE}", component(q)),
        }
    }
}

/// A GIF of the results, or a favourite.
#[derive(Clone, Debug, PartialEq)]
pub struct Gif {
    /// The provider's id as the answer gave it, a string or a number, which
    /// a pick reports as it is (`id: o` in 821102); none for a favourite.
    pub id: Option<Value>,
    /// The link a pick sends as the message, and Copy Link copies. A
    /// favourite is filed under it.
    pub url: String,
    src: String,
    /// The still-animated GIF file, as the answer names it.
    gif_src: Option<String>,
    pub width: u32,
    pub height: u32,
    /// What the preview is: a video, which Neucord has no use for.
    pub video: bool,
}

impl Gif {
    fn from_value(value: &Value) -> Option<Gif> {
        let id = match &value["id"] {
            id @ (Value::String(_) | Value::Number(_)) => id.clone(),
            _ => return None,
        };
        let size = |field: &str| value[field].as_u64().unwrap_or(0) as u32;
        let src = value["src"].as_str()?.to_owned();
        Some(Gif {
            id: Some(id),
            url: value["url"].as_str()?.to_owned(),
            video: has_extension(&src, &["mp4", "webm"]),
            src,
            gif_src: value["gif_src"].as_str().map(str::to_owned),
            width: size("width"),
            height: size("height"),
        })
    }

    /// What an answer lists: a plain array.
    pub fn list(value: &Value) -> Vec<Gif> {
        value.as_array().into_iter().flatten().filter_map(Gif::from_value).collect()
    }

    /// A favourite, as the favourites list shows it.
    fn from_favorite(favorite: &FavoriteGif) -> Gif {
        Gif {
            id: None,
            url: favorite.url.clone(),
            video: favorite.format == GifFormat::Video || has_extension(&favorite.src, &["mp4", "webm"]),
            src: favorite.src.clone(),
            gif_src: None,
            width: favorite.width,
            height: favorite.height,
        }
    }

    /// Where the animated preview comes from.
    pub fn preview(&self) -> String {
        preview_url(&self.src)
    }

    /// Whether the account has it as a favourite.
    pub fn is_favorite(&self, favorites: &[FavoriteGif]) -> bool {
        is_favorite(favorites, &self.url)
    }

    /// What its star hands `wg` (497685). The client stores what the
    /// picker's store listed (`GIF_PICKER_QUERY_SUCCESS`, 821102): the source
    /// without its scheme (`g`), which then is never one of Discord's own
    /// links, so the rewrite of those doesn't apply. The format is an image:
    /// the one asked for is, and what is named `webp`, `avif` or `gif` is too.
    pub fn starred(&self) -> Starred {
        Starred {
            url: self.url.clone(),
            src: without_scheme(&self.src),
            gif_src: self.gif_src.as_deref().map(without_scheme),
            width: self.width,
            height: self.height,
            format: GifFormat::Image,
        }
    }
}

/// What a star on a GIF hands `wg` (497685): the picker's results, and a GIF
/// of a message (`b` in 343552 for an embed's, `cM` in 679636 for an
/// attachment's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Starred {
    /// The link it is filed under, less the signature of an attachment's.
    pub url: String,
    pub src: String,
    /// The still-animated file, where one is offered for a video.
    pub gif_src: Option<String>,
    pub width: u32,
    pub height: u32,
    pub format: GifFormat,
}

impl Starred {
    /// The favourite this makes, after those held (`wg`). Where `src` is a
    /// video, with a different `gif_src` to show instead, that is the
    /// picture; so it is where `src` is one of Discord's own links and
    /// there is any. A Discord link that is a WebP, AVIF or GIF is
    /// asked for as an animated WebP, and what is stored as one of those is
    /// an image, whatever the star said.
    pub fn favorite(&self, held: &[FavoriteGif]) -> FavoriteGif {
        let video = has_extension(&self.src, &["mp4", "webm"]);
        let discord = cdn::is_media(&self.src);
        let chosen = match &self.gif_src {
            Some(gif_src) if (video && !gif_src.is_empty() && *gif_src != self.src) || discord => gif_src,
            _ => &self.src,
        };
        let rewritten = if discord && has_extension(chosen, &["webp", "avif", "gif"]) {
            cdn::stored_animated(chosen)
        } else {
            chosen.clone()
        };
        let src = if rewritten.starts_with("//") { format!("https:{rewritten}") } else { rewritten };
        let format = if has_extension(&src, &["webp", "avif", "gif"]) { GifFormat::Image } else { self.format };
        FavoriteGif {
            url: cdn::without_signature(&self.url),
            format,
            src,
            width: self.width,
            height: self.height,
            order: held.iter().map(|f| f.order).max().unwrap_or(0) + 1,
        }
    }

    /// Whether the account has it as a favourite.
    pub fn is_favorite(&self, favorites: &[FavoriteGif]) -> bool {
        is_favorite(favorites, &self.url)
    }

    /// The star on an attachment (`cM`, 679636): one the proxy serves as a
    /// GIF, or as a WebP or AVIF flagged animated, and that has a size. The
    /// proxied link is the picture, and the attachment's own the link.
    pub fn attachment(attachment: &crate::Attachment) -> Option<Starred> {
        let (width, height) = attachment.width.zip(attachment.height)?;
        let proxy = attachment.proxy_url.as_deref()?;
        let animated = attachment.flags & crate::embed::IS_ANIMATED != 0;
        let shown = names_file(proxy, &["gif"]) || (animated && names_file(proxy, &["webp", "avif"]));
        shown.then(|| Starred {
            url: attachment.url.clone(),
            src: proxy.to_owned(),
            gif_src: None,
            width,
            height,
            format: GifFormat::Image,
        })
    }
}

/// Whether `favorites` holds the GIF of `url` (`km` of `T`).
pub fn is_favorite(favorites: &[FavoriteGif], url: &str) -> bool {
    let key = cdn::without_signature(url);
    favorites.iter().any(|f| f.url == key)
}

/// Why a GIF could not be favourited.
#[derive(Debug, PartialEq, Eq)]
pub struct TooBig;

/// The favourites after `star` is added, or why not (`wg`): the settings
/// message can only be so big.
pub fn favorited(star: &Starred, held: &[FavoriteGif], hide_tooltip: bool) -> Result<(Vec<FavoriteGif>, bool), TooBig> {
    // One filed under the same link is replaced where it was.
    let mut gifs = held.to_vec();
    let favorite = star.favorite(held);
    match gifs.iter_mut().find(|f| f.url == favorite.url) {
        Some(same) => *same = favorite,
        None => gifs.push(favorite),
    }
    if favorite_gifs_size(&gifs, hide_tooltip) > MAX_FAVORITE_GIFS_BYTES {
        return Err(TooBig);
    }
    // The hint to favourite one goes away once there are three.
    Ok((gifs.clone(), hide_tooltip || gifs.len() > 2))
}

/// The favourites after the one of `url` is taken out (`Tr`): filed under
/// the link as it is, or else without its signature.
pub fn unfavorited(held: &[FavoriteGif], url: &str) -> Vec<FavoriteGif> {
    let key = if held.iter().any(|f| f.url == url) { url.to_owned() } else { cdn::without_signature(url) };
    held.iter().filter(|f| f.url != key).cloned().collect()
}

/// The favourites the Favorites page lists for `query`, newest first (`IE`,
/// 996566): the ones whose link holds the query, ignoring case, `-`, `_` and
/// spaces of the query (and `-` and `_` of the link).
pub fn favorites_for(favorites: &[FavoriteGif], query: &str) -> Vec<Gif> {
    let mut sorted: Vec<&FavoriteGif> = favorites.iter().collect();
    // `sortBy("order").reverse()`: equal orders come out the other way round.
    sorted.sort_by_key(|f| f.order);
    sorted.reverse();
    let needle: String = query.to_lowercase().chars().filter(|c| !matches!(c, '-' | '_' | ' ')).collect();
    sorted
        .into_iter()
        .filter(|f| needle.is_empty() || f.url.to_lowercase().replace(['-', '_'], "").contains(&needle))
        .map(Gif::from_favorite)
        .collect()
}

/// `g` in 821102: the source loads over the page's own scheme.
fn without_scheme(src: &str) -> String {
    src.strip_prefix("https:").or_else(|| src.strip_prefix("http:")).unwrap_or(src).to_owned()
}

/// Whether `url` has one of `extensions` before a `?` or its end, wherever
/// (`/\.(a|b)(\?|$)/i`).
fn has_extension(url: &str, extensions: &[&str]) -> bool {
    names(url, extensions, &['?'])
}

/// The same with a `#` ending it too (`/\.a($|\?|#)/i`, 619517).
fn names_file(url: &str, extensions: &[&str]) -> bool {
    names(url, extensions, &['?', '#'])
}

fn names(url: &str, extensions: &[&str], ends: &[char]) -> bool {
    let lower = url.to_ascii_lowercase();
    extensions.iter().any(|e| {
        let dotted = format!(".{e}");
        lower
            .match_indices(&dotted)
            .any(|(at, _)| lower[at + dotted.len()..].chars().next().is_none_or(|next| ends.contains(&next)))
    })
}

/// An image the client shows from an answer: the store strips the scheme
/// (`g`, 821102), so it loads over the page's own, and Discord's media
/// proxy is asked to keep it animated (`F`, 884037).
fn preview_url(src: &str) -> String {
    let without = src.strip_prefix("https:").or_else(|| src.strip_prefix("http:")).unwrap_or(src);
    let url = if without.starts_with("//") { format!("https:{without}") } else { without.to_owned() };
    cdn::animated(&url)
}

/// What a front page tile opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The account's favourites.
    Favorites,
    /// What's trending.
    TrendingGifs,
    /// A category, which is a search for its name.
    Category,
    /// What a typed search, or a suggestion, asks for.
    Search,
}

/// A tile of the front page.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub kind: Kind,
    pub name: String,
    src: String,
}

impl Tile {
    /// The tile of the favourites, in front of the others (`favoritesTile`,
    /// 649529): the picture is one of the favourites. The client shows a
    /// stock GIF from a third party when there is none, which Neucord leaves
    /// out, and the tile has no picture.
    pub fn favorites(sample: Option<&FavoriteGif>) -> Tile {
        let src = sample.map(|f| f.src.clone()).unwrap_or_default();
        Tile { kind: Kind::Favorites, name: FAVORITES_NAME.to_owned(), src }
    }

    /// The tile's picture, unless it is a video, which Neucord has no use for
    /// here: the client plays those as a looping video (`p`, 821102).
    pub fn preview(&self) -> Option<String> {
        let path = self.src.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        (!self.src.is_empty() && !path.ends_with(".mp4") && !path.ends_with(".webm")).then(|| preview_url(&self.src))
    }
}

/// What the client calls the first tile.
const TRENDING_NAME: &str = "Trending GIFs";
const FAVORITES_NAME: &str = "Favorites";

/// The front page: a tile of what's trending, using the first of the GIFs the
/// answer carries, then the categories (`GIF_PICKER_TRENDING_FETCH_SUCCESS`).
#[derive(Debug, Default)]
pub struct Tiles {
    tiles: Vec<Tile>,
    fetching: bool,
}

impl Tiles {
    /// Read what `/gifs/trending` answers: `{categories, gifs}`.
    pub fn parse(value: &Value) -> Vec<Tile> {
        let preview = value["gifs"].as_array().and_then(|gifs| gifs.first()).and_then(|gif| gif["src"].as_str());
        let trending =
            preview.map(|src| Tile { kind: Kind::TrendingGifs, name: TRENDING_NAME.to_owned(), src: src.to_owned() });
        let categories = value["categories"].as_array().into_iter().flatten().filter_map(|c| {
            Some(Tile {
                kind: Kind::Category,
                name: c["name"].as_str()?.to_owned(),
                src: c["src"].as_str()?.to_owned(),
            })
        });
        trending.into_iter().chain(categories).collect()
    }

    pub fn all(&self) -> &[Tile] {
        &self.tiles
    }

    /// What opening the front page asks for: the client fetches when it has
    /// no tiles, and so again after a fetch that failed. One already under
    /// way isn't repeated.
    pub fn opened(&mut self) -> Option<Request> {
        if !self.tiles.is_empty() || self.fetching {
            return None;
        }
        self.fetching = true;
        Some(Request::Trending)
    }

    pub fn fetched(&mut self, tiles: Option<Vec<Tile>>) {
        self.fetching = false;
        if let Some(tiles) = tiles {
            self.tiles = tiles;
        }
    }
}

/// What the picker shows.
#[derive(Debug, PartialEq)]
pub enum Screen<'a> {
    /// Categories to choose from.
    Front,
    /// Waiting for GIFs.
    Loading,
    /// The answer had none.
    Empty,
    /// The favourites, filtered by what is typed.
    Favorites,
    Results {
        gifs: &'a [Gif],
        suggestions: &'a [String],
    },
}

/// What typing in the search box means.
#[derive(Debug, PartialEq, Eq)]
pub enum Typed {
    /// An empty box: back to the front page, nothing asked.
    Front,
    /// A search, after [`SEARCH_DEBOUNCE_MS`] of no more typing. Emptying the
    /// box meanwhile doesn't call it off, as in the client.
    Search(String),
    /// The favourites are filtered by it, with nothing asked.
    Filter,
}

/// One opening of the picker, as `GIFPickerViewStore` keeps it. Everything
/// but the front page's tiles starts over each time.
#[derive(Debug, Default)]
pub struct Picker {
    /// What the results are of; none on the front page.
    page: Option<Kind>,
    /// The search box.
    query: String,
    /// What the results answer; `Some` once an answer came.
    gifs: Option<Vec<Gif>>,
    /// The query those results were for.
    result_query: String,
    suggestions: Vec<String>,
    /// The results list asks for suggestions once for each query it shows.
    suggested_for: Option<String>,
}

impl Picker {
    pub fn screen(&self) -> Screen<'_> {
        match (self.page, &self.gifs) {
            (None, _) => Screen::Front,
            (Some(Kind::Favorites), _) => Screen::Favorites,
            (Some(_), None) => Screen::Loading,
            (Some(_), Some(gifs)) if gifs.is_empty() => Screen::Empty,
            (Some(_), Some(gifs)) => Screen::Results { gifs, suggestions: &self.suggestions },
        }
    }

    /// What the results are of, if there are any.
    pub fn page(&self) -> Option<Kind> {
        self.page
    }

    /// Whether the results have a way back to the front page.
    pub fn has_back(&self) -> bool {
        self.page.is_some()
    }

    /// Whether the search box shows: the trending page has none, nor the
    /// favourites when there are none (`favorites`).
    pub fn shows_search(&self, favorites: usize) -> bool {
        match self.page {
            Some(Kind::TrendingGifs) => false,
            Some(Kind::Favorites) => favorites > 0,
            _ => true,
        }
    }

    /// What the search box holds.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The favourites page was chosen.
    pub fn favorites(&mut self) {
        self.page = Some(Kind::Favorites);
        self.query.clear();
        self.gifs = None;
    }

    /// Start over: the front page, nothing searched.
    pub fn clear(&mut self) {
        *self = Picker::default();
    }

    /// Something was typed into the search box.
    pub fn typed(&mut self, text: &str) -> Typed {
        // The favourites page filters, and an empty box keeps it.
        if self.page == Some(Kind::Favorites) {
            self.query = text.to_owned();
            return Typed::Filter;
        }
        if text.is_empty() {
            self.clear();
            return Typed::Front;
        }
        self.query = text.to_owned();
        self.page = Some(Kind::Search);
        Typed::Search(text.to_owned())
    }

    /// A category or a suggestion was chosen: its search is made at once.
    pub fn searched(&mut self, query: &str, kind: Kind) -> Request {
        self.query = query.to_owned();
        self.page = Some(kind);
        Request::Search(query.to_owned())
    }

    /// The trending tile was chosen.
    pub fn trending(&mut self) -> Request {
        self.page = Some(Kind::TrendingGifs);
        Request::TrendingGifs
    }

    /// What a tile opens: the request it makes, if it does. The favourites
    /// make none.
    pub fn open(&mut self, tile: &Tile) -> Option<Request> {
        match tile.kind {
            Kind::Favorites => {
                self.favorites();
                None
            }
            Kind::TrendingGifs => Some(self.trending()),
            _ => Some(self.searched(&tile.name, Kind::Category)),
        }
    }

    /// The answer to a search: `None` if it failed, which the client shows as
    /// no results. An answer to a search typed over is dropped.
    pub fn answered(&mut self, query: &str, gifs: Option<Vec<Gif>>) {
        if self.page.is_none() || query != self.query {
            return;
        }
        self.result_query = query.to_owned();
        self.gifs = Some(gifs.unwrap_or_default());
    }

    /// What a pick of `gif` reports (`g4` in 497685, from `selectItem` in
    /// 884037, Shift or not, whatever becomes of the message): its id and the
    /// search the shown results answer, `""` for Trending. A GIF without an
    /// id, a favourite, reports nothing.
    pub fn selection(&self, gif: &Gif) -> Option<Value> {
        Some(json!({"id": gif.id.clone()?, "q": self.result_query}))
    }

    /// The answer to the trending request.
    pub fn answered_trending(&mut self, gifs: Option<Vec<Gif>>) {
        if self.page == Some(Kind::TrendingGifs) {
            self.gifs = Some(gifs.unwrap_or_default());
        }
    }

    /// The results list scrolled to `remaining` pixels from its end. The
    /// suggestions it asks for, once for each query the results are of, and
    /// never for none (`handleScroll` and `_E`).
    pub fn scrolled(&mut self, remaining: f64) -> Option<Request> {
        if self.page.is_none() || remaining > NEAR_END {
            return None;
        }
        if self.suggested_for.as_deref() == Some(self.result_query.as_str()) {
            return None;
        }
        self.suggested_for = Some(self.result_query.clone());
        (!self.result_query.is_empty()).then(|| Request::Suggest(self.result_query.clone()))
    }

    /// What the client suggests searching instead. It replaces the last ones,
    /// whatever they were for.
    pub fn suggested(&mut self, suggestions: Vec<String>) {
        if self.page.is_some() {
            self.suggestions = suggestions;
        }
    }

    /// Read what `/gifs/suggest` answers: a list of searches.
    pub fn parse_suggestions(value: &Value) -> Vec<String> {
        value.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn gif(id: &str) -> Gif {
        Gif::from_value(
            &json!({"id": id, "url": format!("https://example.test/gif/{id}"), "src": "//example.test/a.webp"}),
        )
        .unwrap()
    }

    #[test]
    fn each_request_has_the_clients_query_in_its_order() {
        assert_eq!(Request::Trending.query(), "locale=en-US&media_format=tinywebp");
        assert_eq!(Request::TrendingGifs.query(), "media_format=tinywebp&locale=en-US");
        assert_eq!(Request::Search("cats".into()).query(), "q=cats&media_format=tinywebp&locale=en-US");
        assert_eq!(Request::Suggest("cats".into()).query(), "q=cats&limit=5&locale=en-US");
    }

    #[test]
    fn a_query_is_escaped_as_encode_uri_component_does() {
        let query = Request::Search("a b&c=d/é!~*'()-_.".into()).query();
        assert_eq!(query, "q=a%20b%26c%3Dd%2F%C3%A9!~*'()-_.&media_format=tinywebp&locale=en-US");
    }

    #[test]
    fn a_gif_is_read_with_its_link_and_a_preview_over_https() {
        let gifs = Gif::list(&json!([
            {"id": "7", "url": "https://example.test/view/7", "src": "//example.test/7.webp", "gif_src": "//x", "width": 1, "height": 1},
            {"id": 8, "url": "https://example.test/view/8", "src": "https://example.test/8.webp"},
            {"id": "9", "src": "//example.test/9.webp"},
        ]));
        assert_eq!(gifs.iter().map(|g| g.id.clone()).collect::<Vec<_>>(), [Some(json!("7")), Some(json!(8))]);
        assert_eq!(gifs[0].url, "https://example.test/view/7");
        assert_eq!(gifs[0].preview(), "https://example.test/7.webp");
        assert_eq!(gifs[1].preview(), "https://example.test/8.webp");
    }

    #[test]
    fn the_front_page_is_the_trending_tile_then_the_categories() {
        let tiles = Tiles::parse(&json!({
            "categories": [{"name": "happy", "src": "//example.test/h.webp"}, {"name": "sad", "src": "//example.test/s.mp4"}],
            "gifs": [{"id": "1", "url": "u", "src": "//example.test/t.webp"}, {"id": "2", "url": "v", "src": "//example.test/u.webp"}],
        }));
        let names: Vec<_> = tiles.iter().map(|t| (t.kind, t.name.as_str())).collect();
        assert_eq!(names, [(Kind::TrendingGifs, "Trending GIFs"), (Kind::Category, "happy"), (Kind::Category, "sad")]);
        assert_eq!(tiles[0].preview().as_deref(), Some("https://example.test/t.webp"));
        assert_eq!(tiles[2].preview(), None);
        // Without any trending GIF there is no such tile.
        assert_eq!(Tiles::parse(&json!({"categories": [], "gifs": []})).len(), 0);
    }

    #[test]
    fn tiles_are_fetched_when_there_are_none_and_not_while_fetching() {
        let mut tiles = Tiles::default();
        assert_eq!(tiles.opened(), Some(Request::Trending));
        assert_eq!(tiles.opened(), None);
        // A failed fetch is asked for again the next time.
        tiles.fetched(None);
        assert_eq!(tiles.opened(), Some(Request::Trending));
        tiles.fetched(Some(Tiles::parse(&json!({"categories": [{"name": "a", "src": "b"}]}))));
        assert_eq!(tiles.opened(), None);
        assert_eq!(tiles.all().len(), 1);
    }

    #[test]
    fn typing_searches_and_emptying_the_box_goes_back_without_asking() {
        let mut picker = Picker::default();
        assert_eq!(picker.screen(), Screen::Front);
        assert_eq!(picker.typed("cat"), Typed::Search("cat".into()));
        assert_eq!(picker.screen(), Screen::Loading);
        picker.answered("cat", Some(vec![gif("1")]));
        assert!(matches!(picker.screen(), Screen::Results { gifs, .. } if gifs.len() == 1));
        assert_eq!(picker.typed(""), Typed::Front);
        assert_eq!(picker.screen(), Screen::Front);
        // The answer to a search that was cleared meanwhile changes nothing.
        picker.answered("cat", Some(vec![gif("1")]));
        assert_eq!(picker.screen(), Screen::Front);
    }

    #[test]
    fn a_failed_search_is_no_results_and_an_old_answer_is_dropped() {
        let mut picker = Picker::default();
        picker.typed("ca");
        picker.typed("cat");
        picker.answered("ca", Some(vec![gif("1")]));
        assert_eq!(picker.screen(), Screen::Loading);
        picker.answered("cat", None);
        assert_eq!(picker.screen(), Screen::Empty);
    }

    #[test]
    fn a_category_searches_its_name_at_once_and_trending_asks_for_trending() {
        let mut picker = Picker::default();
        let tiles = Tiles::parse(&json!({
            "categories": [{"name": "happy", "src": "//example.test/h.webp"}],
            "gifs": [{"id": "1", "url": "u", "src": "//example.test/t.webp"}],
        }));
        assert_eq!(picker.open(&tiles[1]), Some(Request::Search("happy".into())));
        assert!(picker.has_back() && picker.shows_search(0));
        picker.answered("happy", Some(vec![gif("1")]));
        picker.clear();
        assert_eq!(picker.open(&tiles[0]), Some(Request::TrendingGifs));
        assert!(!picker.shows_search(0));
        picker.answered_trending(Some(vec![gif("2")]));
        assert!(matches!(picker.screen(), Screen::Results { .. }));
    }

    #[test]
    fn suggestions_are_asked_once_for_each_query_and_only_near_the_end() {
        let mut picker = Picker::default();
        picker.typed("cat");
        picker.answered("cat", Some(vec![gif("1")]));
        assert_eq!(picker.scrolled(NEAR_END + 1.0), None);
        assert_eq!(picker.scrolled(NEAR_END), Some(Request::Suggest("cat".into())));
        assert_eq!(picker.scrolled(0.0), None);
        // Another search is asked about too.
        picker.searched("dog", Kind::Search);
        picker.answered("dog", Some(vec![gif("2")]));
        assert_eq!(picker.scrolled(0.0), Some(Request::Suggest("dog".into())));
    }

    #[test]
    fn trending_has_no_query_to_suggest_for() {
        let mut picker = Picker::default();
        picker.trending();
        picker.answered_trending(Some(vec![gif("1")]));
        assert_eq!(picker.scrolled(0.0), None);
    }

    #[test]
    fn suggestions_show_under_the_results_and_start_over_with_the_front_page() {
        let mut picker = Picker::default();
        picker.typed("cat");
        picker.answered("cat", Some(vec![gif("1")]));
        picker.suggested(Picker::parse_suggestions(&json!(["kitten", "cats", 3])));
        assert!(matches!(picker.screen(), Screen::Results { suggestions, .. } if suggestions == ["kitten", "cats"]));
        picker.clear();
        picker.typed("cat");
        picker.answered("cat", Some(vec![gif("1")]));
        assert!(matches!(picker.screen(), Screen::Results { suggestions, .. } if suggestions.is_empty()));
    }

    #[test]
    fn a_pick_reports_the_gifs_id_and_the_search_its_results_answer() {
        let mut picker = Picker::default();
        picker.typed("cat");
        picker.answered("cat", Some(vec![gif("7")]));
        // In the client's field order.
        assert_eq!(picker.selection(&gif("7")).unwrap().to_string(), r#"{"id":"7","q":"cat"}"#);
        // A search still waiting leaves the shown results, and what they answer.
        picker.typed("dog");
        assert_eq!(picker.selection(&gif("7")), Some(json!({"id": "7", "q": "cat"})));

        let tiles = Tiles::parse(&json!({
            "categories": [{"name": "happy", "src": "//example.test/h.webp"}],
            "gifs": [{"id": "1", "url": "u", "src": "//example.test/t.webp"}],
        }));
        picker.clear();
        picker.open(&tiles[1]);
        picker.answered("happy", Some(vec![gif("1")]));
        assert_eq!(picker.selection(&gif("1")), Some(json!({"id": "1", "q": "happy"})));

        // A suggestion starts over, as `handleSelectSuggestion` does.
        picker.clear();
        picker.searched("kitten", Kind::Search);
        picker.answered("kitten", Some(vec![gif("2")]));
        assert_eq!(picker.selection(&gif("2")), Some(json!({"id": "2", "q": "kitten"})));

        picker.clear();
        picker.open(&tiles[0]);
        picker.answered_trending(Some(vec![gif("3")]));
        assert_eq!(picker.selection(&gif("3")), Some(json!({"id": "3", "q": ""})));
    }

    #[test]
    fn the_favourited_hint_is_for_the_first_two_favourites() {
        assert!([0, 1, 2].into_iter().all(favorited_hint));
        assert!(!favorited_hint(3) && !favorited_hint(100));
    }

    #[test]
    fn a_numeric_id_goes_back_as_a_number() {
        let mut picker = Picker::default();
        picker.typed("cat");
        let gifs = Gif::list(&json!([{"id": 8, "url": "https://example.test/view/8", "src": "//example.test/8.webp"}]));
        picker.answered("cat", Some(gifs.clone()));
        assert_eq!(picker.selection(&gifs[0]).unwrap().to_string(), r#"{"id":8,"q":"cat"}"#);
    }

    #[test]
    fn a_favourite_reports_nothing() {
        let mut picker = Picker::default();
        picker.favorites();
        let listed = favorites_for(&[held("https://example.test/view/1", 1)], "");
        assert_eq!(picker.selection(&listed[0]), None);
    }

    fn result(src: &str, gif_src: &str) -> Gif {
        Gif::from_value(&json!({"id": "1", "url": "https://example.test/view/1", "src": src, "gif_src": gif_src, "width": 320, "height": 240}))
            .unwrap()
    }

    fn held(url: &str, order: u32) -> FavoriteGif {
        FavoriteGif {
            url: url.into(),
            format: GifFormat::Image,
            src: format!("https://media.example/{url}.webp"),
            width: 1,
            height: 1,
            order,
        }
    }

    #[test]
    fn a_result_is_favorited_as_the_client_stores_it() {
        let favorite = result("//media.example/a/1.webp", "//media.example/a/1.gif")
            .starred()
            .favorite(&[held("a", 4), held("b", 2)]);
        assert_eq!(
            favorite,
            FavoriteGif {
                url: "https://example.test/view/1".into(),
                format: GifFormat::Image,
                src: "https://media.example/a/1.webp".into(),
                width: 320,
                height: 240,
                order: 5,
            },
            "the link it is filed under, the source with its scheme back, the size, after the newest"
        );
        assert_eq!(result("//m.example/1.webp", "").starred().favorite(&[]).order, 1, "the first is 1");
        // A video source gives way to the GIF file, if there is one that differs.
        assert_eq!(
            result("//m.example/1.mp4", "//m.example/1.gif").starred().favorite(&[]).src,
            "https://m.example/1.gif"
        );
        assert_eq!(
            result("//m.example/1.mp4?x=1", "//m.example/1.gif").starred().favorite(&[]).src,
            "https://m.example/1.gif"
        );
        assert_eq!(
            result("//m.example/1.mp4", "//m.example/1.mp4").starred().favorite(&[]).src,
            "https://m.example/1.mp4"
        );
        assert_eq!(result("//m.example/1.mp4", "").starred().favorite(&[]).src, "https://m.example/1.mp4");
        assert_eq!(
            result("//m.example/1.mp4#x.gif", "//m.example/g.gif").starred().favorite(&[]).src,
            "https://m.example/1.mp4#x.gif"
        );
    }

    fn attachment(proxy: Option<&str>, flags: u64, size: bool) -> crate::Attachment {
        let mut value = json!({"id": "1", "filename": "a.gif", "url": "https://cdn.discordapp.com/attachments/1/2/a.gif?ex=65a0&is=6590&hm=ab&", "flags": flags});
        if let Some(proxy) = proxy {
            value["proxy_url"] = proxy.into();
        }
        if size {
            value["width"] = 320.into();
            value["height"] = 240.into();
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn an_attachment_has_a_star_as_a_gif_or_a_flagged_animation_with_a_size() {
        let media = "https://media.discordapp.net/attachments/1/2/a";
        let star = |proxy: Option<&str>, flags, size| Starred::attachment(&attachment(proxy, flags, size));
        assert!(star(Some(&format!("{media}.gif?ex=65a0&is=6590&hm=ab&")), 0, true).is_some());
        assert!(star(Some(&format!("{media}.GIF#x")), 0, true).is_some(), "`#` ends it too, whatever the case");
        assert!(star(Some(&format!("{media}.webp")), 0, true).is_none(), "a WebP only when flagged");
        assert!(star(Some(&format!("{media}.webp")), 1 << 5, true).is_some());
        assert!(star(Some(&format!("{media}.avif?x=1")), 1 << 5, true).is_some());
        assert!(star(Some(&format!("{media}.png")), 1 << 5, true).is_none());
        assert!(star(Some(&format!("{media}.gifx")), 0, true).is_none());
        assert!(star(None, 0, true).is_none(), "no proxied link");
        assert!(star(Some(&format!("{media}.gif")), 0, false).is_none(), "no size");
    }

    #[test]
    fn an_attachment_is_stored_by_its_proxied_link_asked_animated_under_its_unsigned_link() {
        let proxied = "https://media.discordapp.net/attachments/1/2/a.gif?ex=65a0&is=6590&hm=ab&";
        let star = Starred::attachment(&attachment(Some(proxied), 0, true)).unwrap();
        let favorite = star.favorite(&[held("a", 7)]);
        assert_eq!(
            favorite,
            FavoriteGif {
                url: "https://cdn.discordapp.com/attachments/1/2/a.gif".into(),
                format: GifFormat::Image,
                src: format!("{}&format=webp&animated=true", proxied.trim_end_matches('&')),
                width: 320,
                height: 240,
                order: 8,
            },
            "the signature is no part of the key, but stays in the source"
        );
        // Found and taken back out by the link as the message has it, or by the key.
        assert!(star.is_favorite(std::slice::from_ref(&favorite)));
        assert!(is_favorite(std::slice::from_ref(&favorite), &star.url));
        assert!(unfavorited(std::slice::from_ref(&favorite), &star.url).is_empty());
        // One filed before the signature was dropped goes by the link it was filed under.
        let old = FavoriteGif { url: star.url.clone(), ..favorite };
        assert!(unfavorited(&[old], &star.url).is_empty());
    }

    #[test]
    fn what_the_star_of_an_embed_hands_over_is_stored_as_wg_does() {
        let star = |src: &str, gif_src: Option<&str>, format| Starred {
            url: "https://klipy.test/gif/1".into(),
            src: src.into(),
            gif_src: gif_src.map(Into::into),
            width: 4,
            height: 3,
            format,
        };
        let proxy = "https://images-ext-1.discordapp.net/external/k/https/klipy.test/1";
        // A proxied video gives way to the thumbnail, which is a picture and so an image.
        let klipy = star(&format!("{proxy}.mp4"), Some(&format!("{proxy}.webp")), GifFormat::Video).favorite(&[]);
        assert_eq!(
            (klipy.src.as_str(), klipy.format),
            (format!("{proxy}.webp?animated=true").as_str(), GifFormat::Image)
        );
        // A GIF of the proxy is asked for as a WebP; so is an AVIF.
        assert_eq!(
            star(&format!("{proxy}.gif"), None, GifFormat::Image).favorite(&[]).src,
            format!("{proxy}.gif?format=webp&animated=true")
        );
        assert_eq!(
            star(&format!("{proxy}.avif"), None, GifFormat::Image).favorite(&[]).src,
            format!("{proxy}.avif?format=webp&animated=true")
        );
        // Another provider's GIF link is the video, as a video.
        let tenor = star(&format!("{proxy}.mp4"), None, GifFormat::Video).favorite(&[]);
        assert_eq!((tenor.src.as_str(), tenor.format), (format!("{proxy}.mp4").as_str(), GifFormat::Video));
        // A link off Discord's hosts is left alone, and a video there only gives way to a different GIF.
        assert_eq!(
            star("https://x.test/1.mp4", Some("https://x.test/1.gif"), GifFormat::Video).favorite(&[]).src,
            "https://x.test/1.gif"
        );
        assert_eq!(
            star("https://x.test/1.gif", Some("https://x.test/2.gif"), GifFormat::Image).favorite(&[]).src,
            "https://x.test/1.gif"
        );
    }

    #[test]
    fn favouriting_adds_in_place_hides_the_hint_at_three_and_stops_at_the_size() {
        let gif = result("//m.example/1.webp", "");
        let (one, hide) = favorited(&gif.starred(), &[], false).unwrap();
        assert_eq!((one.len(), hide), (1, false));
        let (two, _) = favorited(&gif.starred(), &[held("a", 1), held("b", 2)], false).unwrap();
        assert!(
            two.len() == 3 && favorited(&gif.starred(), &[held("a", 1), held("b", 2)], false).unwrap().1,
            "the third hides it"
        );
        // The same link again replaces itself.
        let (same, _) = favorited(&gif.starred(), &one, false).unwrap();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].order, 2);
        // Past what the client allows nothing more is added.
        let big: Vec<FavoriteGif> =
            (0..3000).map(|n| FavoriteGif { src: "x".repeat(300), ..held(&format!("{n}"), n) }).collect();
        assert_eq!(favorited(&gif.starred(), &big, false), Err(TooBig));
        assert_eq!(unfavorited(&two, &two[0].url).len(), 2);
    }

    #[test]
    fn the_favorites_list_newest_first_and_filter_by_link() {
        let mut list = vec![held("Funny-Cat", 1), held("dog_run", 3), held("cat nap", 2)];
        list[2].url = "cat-nap".into();
        let urls = |query: &str| favorites_for(&list, query).iter().map(|g| g.url.clone()).collect::<Vec<_>>();
        assert_eq!(urls(""), ["dog_run", "cat-nap", "Funny-Cat"]);
        assert_eq!(urls("CAT"), ["cat-nap", "Funny-Cat"], "whatever the case");
        assert_eq!(urls("funny cat"), ["Funny-Cat"], "spaces and dashes of the query go");
        assert_eq!(urls("dogrun"), ["dog_run"], "and underscores");
        assert!(urls("zebra").is_empty());
        assert!(favorites_for(&list, "")[0].is_favorite(&list));
    }

    #[test]
    fn the_favorites_page_filters_what_is_typed_and_keeps_an_emptied_box() {
        let mut picker = Picker::default();
        let tile = Tile::favorites(None);
        assert_eq!(picker.open(&tile), None, "nothing is asked for them");
        assert_eq!(picker.screen(), Screen::Favorites);
        assert!(picker.has_back());
        assert!(!picker.shows_search(0) && picker.shows_search(1));
        assert_eq!(picker.typed("cat"), Typed::Filter);
        assert_eq!(picker.query(), "cat");
        assert_eq!(picker.typed(""), Typed::Filter);
        assert_eq!(picker.screen(), Screen::Favorites, "emptying the box stays on the page");
        picker.clear();
        assert_eq!(picker.screen(), Screen::Front);
        assert_eq!(Tile::favorites(None).preview(), None, "no stock picture for no favourites");
        assert_eq!(Tile::favorites(Some(&held("a", 1))).preview().as_deref(), Some("https://media.example/a.webp"));
    }
}
