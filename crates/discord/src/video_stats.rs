//! The viewer's Stats for Nerds panel (`VideoStatsOverlay`, 410694): which
//! rows it shows and how it writes them, and its Copy to JSON.
//!
//! The client reads a `<video>` element and the file's header; Neucord plays
//! through `GtkMediaFile`, which exposes neither, so what the file says comes
//! from a GStreamer probe ([`Probe`]) and what changes while it plays from the
//! player ([`Moment`]). A row shows only with its value, so what GStreamer
//! can't give (the client's dropped and decoded frame counts, and its
//! progressive/fragmented format) has no row.

use serde_json::{Value, json};

/// The videos whose panel is on, by their link (`VideoStatsStore`, 953584):
/// it stays on across opening the video again, for the last ten.
#[derive(Debug, Default)]
pub struct Enabled(Vec<String>);

impl Enabled {
    const KEPT: usize = 10;

    pub fn is_on(&self, url: &str) -> bool {
        self.0.iter().any(|u| u == url)
    }

    pub fn set(&mut self, url: &str, on: bool) {
        self.0.retain(|u| u != url);
        if on {
            self.0.push(url.to_owned());
            let excess = self.0.len().saturating_sub(Self::KEPT);
            self.0.drain(..excess);
        }
    }
}

/// What the file says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub container: Option<String>,
    pub video: Option<Video>,
    pub audio: Option<Audio>,
    pub duration_secs: Option<f64>,
    /// In bytes.
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Video {
    /// As written for people, from the caps' media type ([`video_codec`]).
    pub codec: String,
    /// The caps' media type.
    pub raw: String,
    /// Bits per second.
    pub bitrate: Option<u32>,
    pub size: Option<(u32, u32)>,
    /// A fraction.
    pub frame_rate: Option<(u32, u32)>,
    pub profile: Option<String>,
    pub pixel_format: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Audio {
    pub codec: String,
    pub raw: String,
    pub bitrate: Option<u32>,
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
}

/// What the player is doing now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moment {
    /// Of the picture, in pixels.
    pub viewport: (u32, u32),
    pub position_secs: f64,
    /// Zero until the player knows.
    pub duration_secs: f64,
    pub error: Option<String>,
}

impl Probe {
    /// The frame rate the client shows: whole frames a second.
    fn fps(&self) -> Option<u32> {
        let (num, den) = self.video.as_ref()?.frame_rate?;
        (num > 0 && den > 0).then(|| (f64::from(num) / f64::from(den)).round() as u32)
    }
}

impl Moment {
    /// How much of the video lies ahead of the position. The file is on disk
    /// whole before it plays, so all of the rest is buffered.
    pub fn buffered_secs(&self) -> Option<f64> {
        (self.duration_secs > 0.0).then(|| (self.duration_secs - self.position_secs).max(0.0))
    }
}

/// The panel's rows, in the client's order and with its labels, then
/// Neucord's own after them (what else the file says); the error closes the
/// list as it does the client's.
pub fn rows(probe: &Probe, now: &Moment) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    let mut row = |label, value: Option<String>| rows.extend(value.map(|v| (label, v)));
    row("Container", probe.container.clone());
    let video = probe.video.as_ref();
    row(
        "Resolution",
        video.and_then(|v| v.size).map(|(w, h)| match probe.fps() {
            Some(fps) => format!("{w}x{h} @ {fps} fps"),
            None => format!("{w}x{h}"),
        }),
    );
    row(
        "Viewport",
        (now.viewport.0 > 0 && now.viewport.1 > 0).then(|| format!("{}x{}", now.viewport.0, now.viewport.1)),
    );
    row("Video", video.map(|v| with_bitrate(&v.codec, v.bitrate)));
    let audio = probe.audio.as_ref();
    row("Audio", audio.map(|a| with_bitrate(&a.codec, a.bitrate)));
    row(
        "Audio Channels",
        audio.and_then(|a| a.channels).map(|channels| match audio.and_then(|a| a.sample_rate) {
            Some(rate) => format!("{} @ {}", channel_layout(channels), sample_rate(rate)),
            None => channel_layout(channels),
        }),
    );
    row("Buffer Health", now.buffered_secs().map(|s| format!("{s:.1}s")));
    row("Pixel Format", video.and_then(|v| v.pixel_format.clone()));
    row("Video Profile", video.and_then(|v| v.profile.clone()));
    row("Duration", probe.duration_secs.filter(|d| *d > 0.0).map(duration));
    row("File Size", probe.bytes.map(file_size));
    row("Error", now.error.clone());
    rows
}

/// The panel as JSON, in the client's shape (what Copy to JSON puts on the
/// clipboard). What it can't know is `null`, as it is there.
pub fn json(probe: &Probe, now: &Moment) -> String {
    let video = probe.video.as_ref();
    let audio = probe.audio.as_ref();
    let value: Value = json!({
        "media": {
            "video": {
                "codec": video.map(|v| &v.codec),
                "codecRaw": video.map(|v| &v.raw),
                "bitRate": video.and_then(|v| v.bitrate),
                "frameRate": probe.fps(),
                "width": video.and_then(|v| v.size).map(|s| s.0),
                "height": video.and_then(|v| v.size).map(|s| s.1),
            },
            "audio": {
                "codec": audio.map(|a| &a.codec),
                "codecRaw": audio.map(|a| &a.raw),
                "bitRate": audio.and_then(|a| a.bitrate),
                "channels": audio.and_then(|a| a.channels),
                "sampleRate": audio.and_then(|a| a.sample_rate),
            },
            "fileSizeBytes": probe.bytes,
            "durationSeconds": probe.duration_secs,
            "containerFormat": probe.container,
            "isProgressive": null,
            "isFragmented": null,
        },
        "playback": {
            "viewportWidth": now.viewport.0,
            "viewportHeight": now.viewport.1,
            "currentTimeSeconds": now.position_secs,
            "bufferedSeconds": now.buffered_secs(),
            "droppedFrames": null,
            "totalDecodedFrames": null,
            "droppedFramesPercent": null,
            "errorCode": now.error.as_ref().map(|_| 0),
            "errorMessage": now.error,
        },
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// A video codec by its GStreamer media type, as the client names them
/// (`avc1` is H.264/AVC), or the type itself.
pub fn video_codec(media_type: &str) -> String {
    match media_type {
        "video/x-h264" => "H.264/AVC",
        "video/x-h265" => "H.265/HEVC",
        "video/x-vp8" => "VP8",
        "video/x-vp9" => "VP9",
        "video/x-av1" => "AV1",
        other => other.strip_prefix("video/x-").or(other.strip_prefix("video/")).unwrap_or(other),
    }
    .to_owned()
}

/// An audio codec by its media type and, for AAC, the caps' `profile`, as
/// the client names them (`mp4a.40.2` is AAC-LC).
pub fn audio_codec(media_type: &str, mpeg_version: Option<i32>, profile: Option<&str>) -> String {
    match (media_type, mpeg_version, profile) {
        ("audio/mpeg", Some(4 | 2), Some("lc")) => "AAC-LC",
        ("audio/mpeg", Some(4 | 2), Some("he-aac-v1")) => "HE-AAC",
        ("audio/mpeg", Some(4 | 2), Some("he-aac-v2")) => "HE-AACv2",
        ("audio/mpeg", Some(4 | 2), _) => "AAC",
        ("audio/mpeg", Some(1), _) => "MP3",
        ("audio/x-opus", ..) => "Opus",
        ("audio/x-vorbis", ..) => "Vorbis",
        ("audio/x-flac", ..) => "FLAC",
        ("audio/x-ac3", ..) => "AC-3",
        ("audio/x-eac3", ..) => "E-AC-3",
        (other, ..) => other.strip_prefix("audio/x-").or(other.strip_prefix("audio/")).unwrap_or(other),
    }
    .to_owned()
}

/// A container by its media type and the caps' `variant` (`isom` and the
/// like are MP4, `qt` is QuickTime).
pub fn container(media_type: &str, variant: Option<&str>) -> Option<String> {
    Some(
        match (media_type, variant) {
            ("video/quicktime", Some("apple")) => "QuickTime",
            ("video/quicktime", Some("3gpp")) => "3GPP",
            ("video/quicktime", _) => "MP4",
            ("video/x-matroska", _) => "Matroska",
            ("video/webm", _) => "WebM",
            ("video/x-msvideo", _) => "AVI",
            ("application/ogg", _) => "Ogg",
            ("video/mpegts", _) => "MPEG-TS",
            _ => return None,
        }
        .to_owned(),
    )
}

/// "H.264/AVC @ 2.50 Mbps": the client's `BZ`, which has bits a second under
/// a thousand, then Kbps to a decimal and Mbps to two.
pub fn with_bitrate(codec: &str, bitrate: Option<u32>) -> String {
    match bitrate.filter(|b| *b > 0) {
        Some(bps) => format!("{codec} @ {}", bitrate_text(bps)),
        None => codec.to_owned(),
    }
}

fn bitrate_text(bps: u32) -> String {
    match bps {
        0..1000 => format!("{bps} bps"),
        1000..1_000_000 => format!("{:.1} Kbps", f64::from(bps) / 1e3),
        _ => format!("{:.2} Mbps", f64::from(bps) / 1e6),
    }
}

/// The client's `At`.
pub fn channel_layout(channels: u32) -> String {
    match channels {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        6 => "5.1 Surround".to_owned(),
        8 => "7.1 Surround".to_owned(),
        n => format!("{n} channels"),
    }
}

/// The client's `YQ`.
pub fn sample_rate(hz: u32) -> String {
    if hz < 1000 { format!("{hz} Hz") } else { format!("{:.1} kHz", f64::from(hz) / 1e3) }
}

fn duration(secs: f64) -> String {
    crate::media::clock(secs)
}

/// A byte count in decimal units, to a decimal (the client's panel has none:
/// this is Neucord's own row).
pub fn file_size(bytes: u64) -> String {
    match bytes {
        0..1000 => format!("{bytes} B"),
        1000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.2} GB", bytes as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Probe {
        Probe {
            container: Some("MP4".into()),
            video: Some(Video {
                codec: "H.264/AVC".into(),
                raw: "video/x-h264".into(),
                bitrate: Some(2_500_000),
                size: Some((1920, 1080)),
                frame_rate: Some((30000, 1001)),
                profile: Some("high".into()),
                pixel_format: Some("4:2:0 8-bit".into()),
            }),
            audio: Some(Audio {
                codec: "AAC-LC".into(),
                raw: "audio/mpeg".into(),
                bitrate: Some(128_000),
                channels: Some(2),
                sample_rate: Some(48_000),
            }),
            duration_secs: Some(65.0),
            bytes: Some(12_345_678),
        }
    }

    fn moment() -> Moment {
        Moment { viewport: (960, 540), position_secs: 5.0, duration_secs: 65.0, error: None }
    }

    #[test]
    fn the_panel_stays_on_for_the_last_ten_videos() {
        let mut on = Enabled::default();
        for n in 0..11 {
            on.set(&format!("u{n}"), true);
        }
        assert!(!on.is_on("u0"), "the oldest went");
        assert!(on.is_on("u1") && on.is_on("u10"));
        on.set("u1", false);
        assert!(!on.is_on("u1"));
        // Turned on again, it is the newest, so it outlasts the others.
        on.set("u2", true);
        on.set("u11", true);
        on.set("u12", true);
        assert!(on.is_on("u2") && !on.is_on("u3"));
    }

    #[test]
    fn rows_follow_the_clients_order_and_then_neucords_own() {
        let labels: Vec<_> = rows(&probe(), &moment()).into_iter().map(|(l, _)| l).collect();
        assert_eq!(
            labels,
            [
                "Container",
                "Resolution",
                "Viewport",
                "Video",
                "Audio",
                "Audio Channels",
                "Buffer Health",
                "Pixel Format",
                "Video Profile",
                "Duration",
                "File Size"
            ]
        );
    }

    #[test]
    fn rows_are_written_as_the_client_writes_them() {
        let rows = rows(&probe(), &moment());
        let value = |label: &str| rows.iter().find(|(l, _)| *l == label).map(|(_, v)| v.as_str());
        assert_eq!(value("Resolution"), Some("1920x1080 @ 30 fps"));
        assert_eq!(value("Viewport"), Some("960x540"));
        assert_eq!(value("Video"), Some("H.264/AVC @ 2.50 Mbps"));
        assert_eq!(value("Audio"), Some("AAC-LC @ 128.0 Kbps"));
        assert_eq!(value("Audio Channels"), Some("Stereo @ 48.0 kHz"));
        assert_eq!(value("Buffer Health"), Some("60.0s"));
        assert_eq!(value("File Size"), Some("12.3 MB"));
    }

    #[test]
    fn a_row_shows_only_with_its_value() {
        // A video that couldn't be probed: the viewport is all that is known.
        let rows = rows(&Probe::default(), &Moment { viewport: (640, 360), ..Moment::default() });
        assert_eq!(rows, [("Viewport", "640x360".to_owned())]);
        // Sound alone: no video rows, and a rate-less layout reads on its own.
        let probe = Probe {
            audio: Some(Audio { codec: "Opus".into(), channels: Some(6), ..Audio::default() }),
            ..Probe::default()
        };
        let rows = super::rows(&probe, &Moment::default());
        assert_eq!(rows, [("Audio", "Opus".to_owned()), ("Audio Channels", "5.1 Surround".to_owned())]);
    }

    #[test]
    fn the_error_comes_last() {
        let now = Moment { error: Some("Could not decode".into()), ..moment() };
        assert_eq!(rows(&probe(), &now).last(), Some(&("Error", "Could not decode".to_owned())));
    }

    #[test]
    fn buffer_health_is_what_lies_ahead_and_never_negative() {
        assert_eq!(Moment { position_secs: 70.0, ..moment() }.buffered_secs(), Some(0.0));
        assert_eq!(Moment { duration_secs: 0.0, ..moment() }.buffered_secs(), None);
    }

    #[test]
    fn bitrates_and_rates_round_as_the_clients_do() {
        assert_eq!(with_bitrate("VP9", Some(999)), "VP9 @ 999 bps");
        assert_eq!(with_bitrate("VP9", Some(1_500)), "VP9 @ 1.5 Kbps");
        assert_eq!(with_bitrate("VP9", Some(1_234_567)), "VP9 @ 1.23 Mbps");
        assert_eq!(with_bitrate("VP9", None), "VP9");
        assert_eq!(sample_rate(800), "800 Hz");
        assert_eq!(sample_rate(44_100), "44.1 kHz");
        assert_eq!(channel_layout(8), "7.1 Surround");
        assert_eq!(channel_layout(3), "3 channels");
    }

    #[test]
    fn file_sizes_count_in_thousands() {
        assert_eq!(file_size(512), "512 B");
        assert_eq!(file_size(1_500), "1.5 KB");
        assert_eq!(file_size(2_345_000_000), "2.35 GB");
    }

    #[test]
    fn codecs_take_the_clients_names() {
        assert_eq!(video_codec("video/x-h265"), "H.265/HEVC");
        assert_eq!(video_codec("video/x-theora"), "theora");
        assert_eq!(audio_codec("audio/mpeg", Some(4), Some("lc")), "AAC-LC");
        assert_eq!(audio_codec("audio/mpeg", Some(4), None), "AAC");
        assert_eq!(audio_codec("audio/x-opus", None, None), "Opus");
        assert_eq!(container("video/quicktime", Some("iso")).as_deref(), Some("MP4"));
        assert_eq!(container("video/quicktime", Some("apple")).as_deref(), Some("QuickTime"));
        assert_eq!(container("video/webm", None).as_deref(), Some("WebM"));
        assert_eq!(container("video/x-unknown", None), None);
    }

    #[test]
    fn json_has_the_clients_shape_with_null_for_what_is_unknown() {
        let value: Value = serde_json::from_str(&json(&probe(), &moment())).unwrap();
        assert_eq!(value["media"]["video"]["frameRate"], 30);
        assert_eq!(value["media"]["video"]["codec"], "H.264/AVC");
        assert_eq!(value["media"]["audio"]["sampleRate"], 48_000);
        assert_eq!(value["media"]["fileSizeBytes"], 12_345_678);
        assert_eq!(value["playback"]["viewportWidth"], 960);
        assert_eq!(value["playback"]["bufferedSeconds"], 60.0);
        assert!(value["playback"]["droppedFrames"].is_null());
        assert!(value["playback"]["errorMessage"].is_null());
    }
}
