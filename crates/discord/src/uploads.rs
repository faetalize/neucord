//! Attaching files to a message, the way the official client does: each file
//! gets an upload URL of its own (`POST /channels/{id}/attachments`), its
//! bytes go there in a resumable `PUT`, and the message names what was
//! uploaded. There is no multipart upload, whatever the size.
//!
//! The flow is `CloudUpload` in 48325, which starts as soon as a file is
//! added to the draft, not when the message is sent. `getUploadPayload` in
//! 122924 builds the URL request, `OW` in 292348 the message's attachment,
//! and what is checked before any request is 518960's `R` and 433777's
//! `UploaderBase`.
//!
//! Images are converted first as the client does ([`crate::images`]); a
//! converted HEIC or JPEG XR file also tells Discord the original's MD5.
//! A voice message's recording is [`crate::voice`]'s.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::limits::Uploads;
use crate::rest::Rest;
use crate::{Error, Snowflake};

/// How many times a request or upload is retried (`retryOpts`, 48325).
const RETRIES: u32 = 12;

/// Past this, an upload URL is asked for again (`isResponseUrlStale`), in
/// microseconds of wall-clock time, which a suspended machine still counts.
const URL_LIFETIME: i64 = 12 * 60 * 60 * 1_000_000;

fn url_stale(set_at: i64, now: i64) -> bool {
    now - set_at > URL_LIFETIME
}

/// Statuses the client's HTTP layer retries a request on (620632), before
/// giving up on it.
const RETRY_STATUSES: [u32; 8] = [502, 504, 507, 598, 599, 522, 523, 524];

/// The bytes of a file.
#[derive(Clone)]
pub enum Data {
    File(gio::File),
    /// Made here: a pasted image, or text uploaded as a file.
    Memory(glib::Bytes),
}

impl Data {
    /// A stream of the bytes from `from` on.
    async fn open(&self, from: u64) -> Result<gio::InputStream, glib::Error> {
        use gio::prelude::*;
        match self {
            Data::File(file) => {
                let stream = file.read_future(glib::Priority::DEFAULT).await?;
                if from > 0 {
                    stream.seek(from as i64, glib::SeekType::Set, gio::Cancellable::NONE)?;
                }
                Ok(stream.upcast())
            }
            Data::Memory(bytes) => {
                Ok(gio::MemoryInputStream::from_bytes(&glib::Bytes::from_bytes(bytes, from as usize..))
                    .upcast::<gio::InputStream>())
            }
        }
    }
}

/// A file picked for a message.
#[derive(Clone)]
pub struct File {
    pub filename: String,
    pub size: u64,
    /// What a browser's `File.type` would say: from the name's extension,
    /// empty when unknown.
    pub content_type: String,
    pub data: Data,
    /// The MD5 of the file this one was converted from, in hex.
    pub original_md5: Option<String>,
}

/// The type a browser gives a file by its name: by the extension alone, and
/// none for one it doesn't know.
pub fn content_type(filename: &str) -> String {
    if !filename.contains('.') {
        return String::new();
    }
    let (guess, _) = gio::content_type_guess(Some(filename), None::<&[u8]>);
    if guess == "application/octet-stream" { String::new() } else { guess.to_string() }
}

/// Whether the client takes a file of this type for an image, which can have
/// alt text (453771's `GD`): any but Photoshop and SVG.
pub fn is_image(content_type: &str) -> bool {
    content_type.starts_with("image/")
        && !content_type.starts_with("image/vnd.adobe.photoshop")
        && !content_type.starts_with("image/svg+xml")
}

pub fn is_video(content_type: &str) -> bool {
    content_type.starts_with("video/")
}

thread_local! {
    static IDS: Cell<u64> = const { Cell::new(0) };
}

/// A fresh id, like lodash's `uniqueId`, which the client numbers requests by.
pub fn unique_id() -> String {
    IDS.with(|ids| {
        ids.set(ids.get() + 1);
        ids.get().to_string()
    })
}

/// What `getUploadPayload` (122924) asks for an upload URL with. Clips are
/// the only other thing it carries, which a clip upload adds.
pub fn create_body(id: &str, file: &File) -> Value {
    json!({ "files": [{
        "filename": file.filename,
        "file_size": file.size,
        "id": id,
        "is_clip": false,
        "original_content_type": file.content_type,
    }] })
}

/// Where a file's bytes go, and the name Discord gave them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub url: String,
    pub filename: String,
}

impl Slot {
    fn from_response(body: &[u8]) -> Result<Slot, Error> {
        let body: Value = serde_json::from_slice(body)?;
        let first = &body["attachments"][0];
        let text = |key: &str| first[key].as_str().map(str::to_owned);
        match (text("upload_url"), text("upload_filename")) {
            (Some(url), Some(filename)) => Ok(Slot { url, filename }),
            _ => Err(Error::Protocol("no upload URL in the answer".into())),
        }
    }
}

/// An uploaded file, as the message names it.
pub struct Sent<'a> {
    pub filename: &'a str,
    pub uploaded_filename: &'a str,
    /// Alt text, once the user has set any (even an empty one).
    pub description: Option<&'a str>,
    pub spoiler: bool,
    pub content_type: &'a str,
    /// What a voice message adds.
    pub voice: Option<Voice<'a>>,
}

/// A voice message's length and waveform.
#[derive(Clone, Copy)]
pub struct Voice<'a> {
    pub duration_secs: f64,
    /// Base64 of at most 256 bytes.
    pub waveform: &'a str,
}

impl Sent<'_> {
    /// The message's `attachments[index]`, as `OW` (292348) builds it.
    pub fn body(&self, index: usize) -> Value {
        let mut body = json!({ "id": index.to_string() });
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        body["filename"] = json!(self.filename);
        body["uploaded_filename"] = json!(self.uploaded_filename);
        if self.spoiler {
            body["is_spoiler"] = json!(true);
        }
        if let Some(voice) = self.voice {
            body["duration_secs"] = json!(voice.duration_secs);
            body["waveform"] = json!(voice.waveform);
        }
        body["original_content_type"] = json!(self.content_type);
        body
    }
}

/// Why files are turned back before any request: the client's own words
/// (`openUploadError` in 518960).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    /// A file over what we may upload.
    TooLarge {
        max: u64,
    },
    /// All the files together are.
    TotalTooLarge {
        max: u64,
    },
    TooMany {
        max: usize,
    },
}

impl Refusal {
    pub fn heading(&self) -> &'static str {
        match self {
            Refusal::Empty => "Upload Failed",
            Refusal::TooLarge { .. } | Refusal::TotalTooLarge { .. } => "Your files are too powerful",
            Refusal::TooMany { .. } => "Too many uploads!",
        }
    }

    pub fn body(&self) -> String {
        match self {
            Refusal::Empty => "File cannot be empty".to_owned(),
            Refusal::TooLarge { max } => format!("Max file size is {} please.", size_words(*max)),
            Refusal::TotalTooLarge { max } => format!("Total message size must be less than {}", size_words(*max)),
            Refusal::TooMany { max } => format!("You can only upload {max} files at a time!"),
        }
    }
}

/// A size as the client writes it: `20 MB`, with mebibytes.
pub fn size_words(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    match bytes {
        b if b >= 1024 * MIB && b % (1024 * MIB) == 0 => format!("{} GB", b / (1024 * MIB)),
        b if b >= MIB && b % MIB == 0 => format!("{} MB", b / MIB),
        b if b >= MIB => format!("{:.1} MB", b as f64 / MIB as f64),
        b if b >= 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// Whether `added` may join `queued` in a message, by size. The client asks
/// in this order when files are added (518960's `R`): an empty file, one
/// over the limit or a batch over the total, then the count. It checks the
/// total of all files only at send, after the uploads. Neucord checks it
/// here as well, deliberately: uploads start when a file is added, so a
/// draft over the total would be uploaded in full just to be turned back at
/// send. Holding back earlier never sends more than the client would.
pub fn check_added(limits: &Uploads, queued: &[u64], added: &[u64]) -> Result<(), Refusal> {
    if added.contains(&0) {
        return Err(Refusal::Empty);
    }
    if added.iter().any(|&size| size > limits.file_size) {
        return Err(Refusal::TooLarge { max: limits.file_size });
    }
    if queued.iter().chain(added).sum::<u64>() > limits.total_size {
        return Err(Refusal::TotalTooLarge { max: limits.total_size });
    }
    if queued.len() + added.len() > limits.files {
        return Err(Refusal::TooMany { max: limits.files });
    }
    Ok(())
}

/// Whether `sizes`, everything the draft holds, may be sent: the count, then
/// the total, as `compressAndCheckFileSize` (433777) asks.
pub fn check_send(limits: &Uploads, sizes: &[u64]) -> Result<(), Refusal> {
    if sizes.len() > limits.files {
        return Err(Refusal::TooMany { max: limits.files });
    }
    if sizes.iter().sum::<u64>() > limits.total_size {
        return Err(Refusal::TotalTooLarge { max: limits.total_size });
    }
    Ok(())
}

/// The message box's text as the attach menu's Upload Text as File sends it.
#[derive(Debug, PartialEq, Eq)]
pub struct TextFile<'a> {
    /// `message.txt`, or `message.<language>` of a code block.
    pub filename: String,
    pub content: &'a str,
    /// What stays in the box: the text around a code block.
    pub rest: String,
}

/// Split `text` for Upload Text as File (316576): with a fenced code block
/// that has a language, only the code goes, and the text around it stays.
/// This is the client's `/(.*)```(\w+)\n(.*)```(.*)/s`: the last block that
/// closes, to the last fence after it.
pub fn text_file(text: &str) -> TextFile<'_> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (start, _) in text.rmatch_indices("```") {
        let after = &text[start + 3..];
        let language = after.len() - after.trim_start_matches(word).len();
        if language == 0 || !after[language..].starts_with('\n') {
            continue;
        }
        let body = &after[language + 1..];
        let Some(end) = body.rfind("```") else { continue };
        return TextFile {
            filename: format!("message.{}", &after[..language]),
            content: &body[..end],
            rest: format!("{}{}", &text[..start], &body[end + 3..]),
        };
    }
    TextFile { filename: "message.txt".to_owned(), content: text, rest: String::new() }
}

/// How long to wait between retries (158390): from half a second, growing
/// by a random share of double itself, to ten times the first.
pub struct Backoff {
    current: f64,
    max: f64,
}

impl Backoff {
    pub fn new() -> Self {
        Backoff { current: 0.5, max: 5.0 }
    }

    /// The wait after another failure, for a random `roll` between 0 and 1.
    fn fail_with(&mut self, roll: f64) -> Duration {
        self.current = (self.current + 2.0 * self.current * roll).min(self.max);
        Duration::from_secs_f64(self.current)
    }

    pub fn fail(&mut self) -> Duration {
        self.fail_with(glib::random_double_range(0.0, 1.0))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// How an upload request failed, for deciding whether to go on (`v` in
/// 48325).
fn retryable(error: &Error) -> bool {
    match error {
        // The network, not what Discord said.
        Error::Transport(e) => !e.matches(gio::IOErrorEnum::Cancelled),
        Error::Http { status, .. } => *status == 429 || (500..600).contains(status),
        _ => false,
    }
}

/// `Content-Range` for the rest of a file from `from` (`startOrResumeUpload`).
fn content_range(from: u64, size: u64) -> String {
    format!("bytes {from}-{}/{size}", size.saturating_sub(1))
}

/// Where an upload stands, from the `Range` of a 308 answer to
/// `Content-Range: bytes */*` (`parseRangeHeader`): the next byte to send.
fn resume_from(range: Option<&str>) -> u64 {
    let parse = |range: &str| {
        let (first, rest) = range.strip_prefix("bytes=")?.split_once('-')?;
        let last = rest.split_once('/').map_or(rest, |(last, _)| last);
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        (digits(first) && digits(last)).then(|| last.parse::<u64>().ok()).flatten()
    };
    range.and_then(parse).map_or(0, |last| last + 1)
}

/// Discord's error code for a file over what we may upload.
const ENTITY_TOO_LARGE: u64 = 40005;

/// Why an upload failed.
#[derive(Debug)]
pub enum Failure {
    /// Discord, or the network, turned a request down.
    Request(Error),
    /// Retried as often as the client does.
    GaveUp { attempts: u32 },
}

impl Failure {
    /// Discord's error code, when it said one.
    pub fn code(&self) -> Option<u64> {
        match self {
            Failure::Request(Error::Http { body, .. }) => serde_json::from_str::<Value>(body).ok()?["code"].as_u64(),
            _ => None,
        }
    }

    /// Whether sending the message tries the upload again: for every failure
    /// but a file Discord called too large (358579).
    pub fn restarts(&self) -> bool {
        self.code() != Some(ENTITY_TOO_LARGE)
    }

    pub fn words(&self) -> String {
        match (self.code(), self) {
            (Some(ENTITY_TOO_LARGE), _) => "The file is too large.".to_owned(),
            (_, Failure::GaveUp { attempts }) => format!("Gave up after {attempts} attempts."),
            (_, Failure::Request(e)) => e.to_string(),
        }
    }
}

/// Upload `file` to `channel`'s message attachments and give back the name
/// Discord keeps it by, as `CloudUpload.upload` does after the size checks
/// (those are [`check_added`]). `sent` is told how many bytes have gone.
/// `attempts` counts the tries of this file across sends, as the client's
/// `uploadAttempts` does: one that used them all gets no more.
/// Dropping the future cancels it.
pub async fn upload(
    rest: &Rest,
    channel: Snowflake,
    file: &File,
    attempts: &Cell<u32>,
    sent: Rc<dyn Fn(u64)>,
) -> Result<String, Failure> {
    let mut slot = rest
        .create_upload(channel, create_body(&unique_id(), file), file.original_md5.as_deref())
        .await
        .map_err(Failure::Request)?;
    let mut slot_at = glib::real_time();
    let mut backoff = Backoff::new();
    let mut from = 0;
    while attempts.get() <= RETRIES {
        attempts.set(attempts.get() + 1);
        let step = async {
            if url_stale(slot_at, glib::real_time()) {
                let body = create_body(&unique_id(), file);
                let fresh = rest.create_upload(channel, body, file.original_md5.as_deref()).await?;
                (slot, slot_at, from) = (fresh, glib::real_time(), 0);
            } else if attempts.get() > 1 {
                from = rest.upload_position(&slot.url, file.size).await?;
            }
            let (at, sent) = (from, sent.clone());
            rest.put_upload(&slot.url, file, from, Rc::new(move |n| sent(at + n))).await
        };
        match step.await {
            Ok(()) => return Ok(slot.filename),
            Err(e) if retryable(&e) => glib::timeout_future(backoff.fail()).await,
            Err(e) => return Err(Failure::Request(e)),
        }
    }
    Err(Failure::GaveUp { attempts: attempts.get() })
}

/// Take an upload back, once it has been made: the file stays on Discord's
/// side until then, and the message that was to name it never will. Like the
/// client's `delete`, it ignores a failure.
pub async fn delete(rest: &Rest, uploaded_filename: &str) {
    rest.delete_upload(uploaded_filename).await;
}

impl Rest {
    /// An upload URL for a file (`getUploadPayload`'s body). The client's HTTP
    /// layer retries this on a few statuses and on network errors, telling
    /// Discord how often in `X-Failed-Requests`, and never on a rate limit.
    /// A converted file adds its original's hash (`buildOriginalMd5Headers`).
    async fn create_upload(&self, channel: Snowflake, body: Value, original_md5: Option<&str>) -> Result<Slot, Error> {
        use crate::endpoints::call;
        let mut backoff = Backoff::new();
        let mut failed = 0;
        loop {
            let result = self
                .send_upload_request(call!(CREATE_ATTACHMENT, channel), Some(body.clone()), failed, original_md5)
                .await;
            let again = match &result {
                Ok((status, _)) => RETRY_STATUSES.contains(status),
                Err(e @ Error::Transport(_)) => retryable(e),
                Err(_) => false,
            };
            if !again || failed >= RETRIES {
                let (status, response) = result?;
                return match status {
                    200..=299 => Slot::from_response(&response),
                    401 => Err(Error::Unauthorized),
                    _ => Err(Error::Http {
                        status,
                        body: String::from_utf8_lossy(&response).chars().take(300).collect(),
                    }),
                };
            }
            failed += 1;
            glib::timeout_future(backoff.fail()).await;
        }
    }

    /// Where an interrupted upload has got to (`getResumePosition`).
    async fn upload_position(&self, url: &str, size: u64) -> Result<u64, Error> {
        let (status, range) = self.put(url, None, Some("bytes */*"), Rc::new(|_| {})).await?;
        match status {
            200 | 201 => Ok(size),
            308 => Ok(resume_from(range.as_deref())),
            status => Err(Error::Http { status, body: String::new() }),
        }
    }

    /// The bytes of `file` from `from` on (`startOrResumeUpload`).
    async fn put_upload(&self, url: &str, file: &File, from: u64, sent: Rc<dyn Fn(u64)>) -> Result<(), Error> {
        let stream = file.data.open(from).await.map_err(Error::Transport)?;
        let range = (from > 0).then(|| content_range(from, file.size));
        let body = Some((stream, (file.size - from) as isize));
        match self.put(url, body, range.as_deref(), sent).await? {
            (200 | 201, _) => Ok(()),
            (status, _) => Err(Error::Http { status, body: String::new() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn limits() -> Uploads {
        Uploads { file_size: 20 * MIB, files: 10, total_size: 500 * MIB }
    }

    fn file(name: &str, size: u64, content_type: &str) -> File {
        File {
            filename: name.into(),
            size,
            content_type: content_type.into(),
            data: Data::Memory(glib::Bytes::from_static(b"")),
            original_md5: None,
        }
    }

    #[test]
    fn asks_for_an_upload_url_with_the_clients_fields() {
        // `getUploadPayload`: no title or clip fields outside of clips.
        let body = create_body("7", &file("cat.png", 1234, "image/png"));
        assert_eq!(
            body,
            json!({ "files": [{ "filename": "cat.png", "file_size": 1234, "id": "7", "is_clip": false,
                "original_content_type": "image/png" }] })
        );
        // A name no extension tells the type of has none, as in a browser.
        assert_eq!(create_body("8", &file("notes", 1, ""))["files"][0]["original_content_type"], "");
    }

    #[test]
    fn the_message_names_each_upload() {
        let plain = Sent {
            filename: "cat.png",
            uploaded_filename: "u/cat.png",
            description: None,
            spoiler: false,
            content_type: "image/png",
            voice: None,
        };
        assert_eq!(
            plain.body(0),
            json!({ "id": "0", "filename": "cat.png", "uploaded_filename": "u/cat.png",
                "original_content_type": "image/png" })
        );
        // Alt text is sent once set, even when empty; the spoiler flag only when on.
        let set = Sent { description: Some(""), spoiler: true, ..plain };
        assert_eq!(
            set.body(3),
            json!({ "id": "3", "description": "", "filename": "cat.png", "uploaded_filename": "u/cat.png",
                "is_spoiler": true, "original_content_type": "image/png" })
        );
    }

    #[test]
    fn a_voice_message_names_its_length_and_waveform() {
        // `OW` (292348) puts both after `is_spoiler`, before the type.
        let sent = Sent {
            filename: "0recording.ogg",
            uploaded_filename: "u/0recording.ogg",
            description: None,
            spoiler: false,
            content_type: "audio/ogg",
            voice: Some(Voice { duration_secs: 4.68, waveform: "AAEC/w==" }),
        };
        assert_eq!(
            sent.body(0),
            json!({ "id": "0", "filename": "0recording.ogg", "uploaded_filename": "u/0recording.ogg",
                "duration_secs": 4.68, "waveform": "AAEC/w==", "original_content_type": "audio/ogg" })
        );
    }

    #[test]
    fn reads_the_upload_url() {
        let body =
            br#"{"attachments":[{"id":7,"upload_url":"https://cdn.example/u?x=1","upload_filename":"u/cat.png"}]}"#;
        assert_eq!(
            Slot::from_response(body).unwrap(),
            Slot { url: "https://cdn.example/u?x=1".into(), filename: "u/cat.png".into() }
        );
        assert!(Slot::from_response(br#"{"attachments":[]}"#).is_err());
    }

    #[test]
    fn files_are_turned_back_in_the_clients_order() {
        let l = limits();
        assert_eq!(check_added(&l, &[], &[1, 0]), Err(Refusal::Empty));
        assert_eq!(check_added(&l, &[], &[0, 21 * MIB]), Err(Refusal::Empty), "empty is asked first");
        assert_eq!(check_added(&l, &[], &[20 * MIB + 1]), Err(Refusal::TooLarge { max: 20 * MIB }));
        assert_eq!(check_added(&l, &[], &[20 * MIB]), Ok(()), "the limit itself is fine");
        // Ten files fit; the eleventh doesn't, whether added together or later.
        assert_eq!(check_added(&l, &[1; 9], &[1]), Ok(()));
        assert_eq!(check_added(&l, &[1; 9], &[1, 1]), Err(Refusal::TooMany { max: 10 }));
        assert_eq!(check_added(&l, &[], &[1; 11]), Err(Refusal::TooMany { max: 10 }));
        // Together they stay under the total, which the draft counts as well.
        let big = Uploads { file_size: 400 * MIB, ..l };
        assert_eq!(check_added(&big, &[300 * MIB], &[200 * MIB]), Ok(()));
        assert_eq!(check_added(&big, &[300 * MIB], &[200 * MIB + 1]), Err(Refusal::TotalTooLarge { max: 500 * MIB }));
        assert_eq!(check_send(&big, &[300 * MIB, 201 * MIB]), Err(Refusal::TotalTooLarge { max: 500 * MIB }));
        assert_eq!(check_send(&l, &[1; 11]), Err(Refusal::TooMany { max: 10 }));
        assert_eq!(check_send(&l, &[1; 10]), Ok(()));
    }

    #[test]
    fn refusals_speak_like_the_client() {
        assert_eq!(Refusal::TooMany { max: 10 }.heading(), "Too many uploads!");
        assert_eq!(Refusal::TooMany { max: 10 }.body(), "You can only upload 10 files at a time!");
        assert_eq!(Refusal::TooLarge { max: 20 * MIB }.body(), "Max file size is 20 MB please.");
        assert_eq!(Refusal::TotalTooLarge { max: 1024 * MIB }.body(), "Total message size must be less than 1 GB");
        assert_eq!(Refusal::Empty.body(), "File cannot be empty");
    }

    #[test]
    fn upload_text_as_file_takes_a_code_block_alone() {
        // No block: all of it, and nothing stays.
        let plain = text_file("hello\nworld");
        assert_eq!((plain.filename.as_str(), plain.content, plain.rest.as_str()), ("message.txt", "hello\nworld", ""));
        // A block with a language: only the code goes; what's around it stays.
        let block = text_file("look:\n```rust\nfn main() {}\n```\nthanks");
        assert_eq!(block.filename, "message.rust");
        assert_eq!(block.content, "fn main() {}\n");
        assert_eq!(block.rest, "look:\n\nthanks");
        // A block without a language, or never closed, is just text.
        assert_eq!(text_file("```\ncode\n```").filename, "message.txt");
        assert_eq!(text_file("```js\nnever closed").filename, "message.txt");
        // The regex is greedy: the last block that closes, to the last fence.
        let two = text_file("```a\none\n```\n```b\ntwo\n```");
        assert_eq!((two.filename.as_str(), two.content, two.rest.as_str()), ("message.b", "two\n", "```a\none\n```\n"));
        let inner = text_file("```a\none\n```x\n```");
        assert_eq!((inner.filename.as_str(), inner.content, inner.rest.as_str()), ("message.x", "", "```a\none\n"));
        // `\w` is ASCII, and the newline must follow the language at once.
        assert_eq!(text_file("```c++\nx\n```").filename, "message.txt");
        assert_eq!(text_file("```js x\ny\n```").filename, "message.txt");
    }

    #[test]
    fn resumes_where_discord_says() {
        assert_eq!(content_range(5, 10), "bytes 5-9/10");
        assert_eq!(resume_from(Some("bytes=0-41")), 42);
        assert_eq!(resume_from(Some("bytes=0-41/100")), 42);
        assert_eq!(resume_from(None), 0, "nothing received yet");
        assert_eq!(resume_from(Some("bytes=abc")), 0);
        assert_eq!(resume_from(Some("bytes=0-")), 0);
    }

    #[test]
    fn an_upload_url_lasts_twelve_hours() {
        let hour = 3_600_000_000;
        assert!(!url_stale(0, 12 * hour) && url_stale(0, 12 * hour + 1));
    }

    #[test]
    fn waits_longer_after_each_failure_up_to_five_seconds() {
        let mut backoff = Backoff::new();
        let waits: Vec<_> = (0..5).map(|_| backoff.fail_with(0.5).as_secs_f64()).collect();
        assert_eq!(waits, [1.0, 2.0, 4.0, 5.0, 5.0]);
        let mut backoff = Backoff::new();
        assert_eq!(backoff.fail_with(0.0).as_secs_f64(), 0.5, "no jitter, no growth");
    }

    #[test]
    fn retries_what_the_client_retries() {
        let http = |status| Error::Http { status, body: String::new() };
        assert!(retryable(&http(500)) && retryable(&http(503)) && retryable(&http(429)));
        assert!(!retryable(&http(400)) && !retryable(&http(403)) && !retryable(&http(404)));
        assert!(retryable(&Error::Transport(glib::Error::new(gio::IOErrorEnum::ConnectionRefused, "reset"))));
        assert!(!retryable(&Error::Transport(glib::Error::new(gio::IOErrorEnum::Cancelled, "cancelled"))));
    }

    #[test]
    fn writes_sizes_like_the_client() {
        assert_eq!(size_words(20 * MIB), "20 MB");
        assert_eq!(size_words(1024 * MIB), "1 GB");
        assert_eq!(size_words(1536 * 1024), "1.5 MB");
        assert_eq!(size_words(100_000), "97.7 KB");
        assert_eq!(size_words(70), "70 B");
    }

    #[test]
    fn ids_count_up() {
        let (a, b) = (unique_id().parse::<u64>().unwrap(), unique_id().parse::<u64>().unwrap());
        assert_eq!(b, a + 1);
    }

    #[test]
    fn a_file_discord_called_too_large_is_not_tried_again_at_send() {
        let http = |body: &str| Failure::Request(Error::Http { status: 413, body: body.to_owned() });
        assert!(!http(r#"{"code": 40005}"#).restarts(), "358579 skips ENTITY_TOO_LARGE");
        assert!(http(r#"{"code": 50035}"#).restarts());
        assert!(Failure::GaveUp { attempts: 13 }.restarts());
    }

    #[test]
    fn alt_text_goes_on_images_and_videos() {
        assert!(is_image("image/png") && is_video("video/mp4"));
        assert!(!is_image("image/svg+xml") && !is_image("image/vnd.adobe.photoshop") && !is_image(""));
        assert!(!is_image("text/plain") && !is_video("audio/mpeg"));
    }

    #[test]
    fn types_come_from_the_name() {
        assert_eq!(content_type("cat.png"), "image/png");
        assert_eq!(content_type("notes.txt"), "text/plain");
        assert_eq!(content_type("README"), "", "no extension");
        assert_eq!(content_type("data.zzzunknown"), "");
    }
}
