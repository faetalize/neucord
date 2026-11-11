//! Voice messages, recorded as the official desktop client does: its native
//! recorder writes a mono 48 kHz Opus stream in Ogg, which goes up as
//! `{n}recording.ogg` with the recording's length and a waveform, in a
//! message flagged `IS_VOICE_MESSAGE` with no text.
//!
//! The recorder is closed source, so the bytes below come from recordings
//! the desktop app made on Linux, not from the web client: Opus packets of
//! 20 ms, `OpusTags` padded to 764 bytes, one Ogg page for every 50 packets,
//! and a final 20 ms silence packet. What the web client does with the file
//! is in 841817 (`es`, `ea`, `em`), the rules for offering it in 87207 and
//! 158781's `K`, and the recorder's bridge in 198620.

use std::collections::VecDeque;

use crate::Snowflake;
use crate::experiments::Experiments;
use crate::uploads::{Data, File};

/// The experiment that makes the desktop client record voice messages: variant
/// 1 turns it on (74477).
pub fn enabled(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, "2026-09-desktop-voice-message-recording").is_some_and(|a| a.variant == 1)
}

/// What a channel's message box is already holding or doing, none of which
/// the client records a voice message next to (87207): an edit, files or a
/// sticker waiting to be sent, or a recording of its own.
#[derive(Clone, Copy, Default)]
pub struct Waiting {
    pub editing: bool,
    pub files: bool,
    pub sticker: bool,
    pub recording: bool,
}

impl Waiting {
    pub fn blocks(self) -> bool {
        self.editing || self.files || self.sticker || self.recording
    }
}

/// A recording stops by itself this long after it starts (`iA` in 518950).
pub const MAX_MS: u64 = 1_199_500;

/// Shorter than this a recording is dropped, not sent (`ea` in 841817).
const MIN_SECS: f64 = 0.5;

/// The rate, and the length of one Opus packet in samples.
pub const SAMPLE_RATE: u32 = 48_000;
const FRAME: usize = 960;

/// What the encoder aims for, as the recorder's packets average between 30
/// and 34 kbps.
pub const BITRATE: u32 = 32_000;

/// How many Opus packets the recorder puts in an Ogg page: one second.
const PACKETS_PER_PAGE: usize = 50;

/// The silence packet each recording ends with: a 20 ms CELT fullband frame
/// holding nothing.
const SILENCE: [u8; 3] = [0xf8, 0xff, 0xfe];

/// The type the client gives a recording by its `.ogg` extension (198620).
pub const CONTENT_TYPE: &str = "audio/ogg";

/// What the recorder names its `n`th recording of this run, across chats.
pub fn file_name(n: u32) -> String {
    format!("{n}recording.ogg")
}

/// The Ogg stream serial the recorder gives its `n`th recording of a run: the
/// `n + 1`th value of glibc's `rand()`, which nothing seeds.
pub fn stream_serial(n: u32) -> u32 {
    let mut rand = Rand::new();
    (0..n).for_each(|_| _ = rand.next());
    rand.next()
}

/// glibc's `rand()` (TYPE_3, seed 1): an additive feedback generator over 31
/// words, discarding its first 310 outputs.
struct Rand {
    /// The last 31 words, oldest first.
    words: VecDeque<u32>,
}

impl Rand {
    fn new() -> Self {
        let mut r = [0u32; 34];
        r[0] = 1;
        for i in 1..31 {
            r[i] = (16807 * u64::from(r[i - 1]) % 2_147_483_647) as u32;
        }
        for i in 31..34 {
            r[i] = r[i - 31];
        }
        let mut rand = Rand { words: r[3..].iter().copied().collect() };
        for _ in 0..310 {
            rand.step();
        }
        rand
    }

    fn step(&mut self) -> u32 {
        let next = self.words[0].wrapping_add(self.words[28]);
        self.words.pop_front();
        self.words.push_back(next);
        next
    }

    fn next(&mut self) -> u32 {
        self.step() >> 1
    }
}

/// The Ogg checksum: CRC-32 with polynomial 0x04c11db7, most significant bit
/// first, started and ended at zero.
fn crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04c1_1db7 } else { crc << 1 };
        }
    }
    crc
}

const BEGINNING: u8 = 2;
const END: u8 = 4;

/// One Ogg page of whole packets.
fn page(serial: u32, sequence: u32, kind: u8, granule: u64, packets: &[&[u8]]) -> Vec<u8> {
    let mut lacing = Vec::new();
    for packet in packets {
        lacing.extend(std::iter::repeat_n(255, packet.len() / 255));
        lacing.push((packet.len() % 255) as u8);
    }
    let mut page = Vec::with_capacity(27 + lacing.len() + packets.iter().map(|p| p.len()).sum::<usize>());
    page.extend_from_slice(b"OggS");
    page.push(0);
    page.push(kind);
    page.extend_from_slice(&granule.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(lacing.len() as u8);
    page.extend_from_slice(&lacing);
    packets.iter().for_each(|p| page.extend_from_slice(p));
    let sum = crc(&page).to_le_bytes();
    page[22..26].copy_from_slice(&sum);
    page
}

/// `OpusHead`: mono, no pre-skip, 48 kHz in, no gain, mapping family 0.
fn opus_head() -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, 1, 0, 0]);
    head.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    head.extend_from_slice(&[0, 0, 0]);
    head
}

/// `OpusTags`: the vendor and one comment, then zeros up to 764 bytes.
fn opus_tags() -> Vec<u8> {
    let (vendor, comment) = ("Opus provided by WebRTC", "ENCODER=Discord Client");
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor.as_bytes());
    tags.extend_from_slice(&1u32.to_le_bytes());
    tags.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    tags.extend_from_slice(comment.as_bytes());
    tags.resize(764, 0);
    tags
}

/// An Ogg Opus file made of Opus packets as they come.
pub struct OggOpus {
    serial: u32,
    sequence: u32,
    out: Vec<u8>,
    /// Packets of the page not written yet. A page goes out only when
    /// another packet follows, so the last one is known to be the last.
    waiting: Vec<Vec<u8>>,
    /// Packets in the pages written, which each page's granule counts.
    written: u64,
    packets: u64,
}

impl OggOpus {
    pub fn new(serial: u32) -> Self {
        let mut out = page(serial, 0, BEGINNING, 0, &[&opus_head()]);
        out.extend(page(serial, 1, 0, 0, &[&opus_tags()]));
        OggOpus { serial, sequence: 2, out, waiting: Vec::new(), written: 0, packets: 0 }
    }

    /// Add the next packet, of 20 ms.
    pub fn push(&mut self, packet: &[u8]) {
        self.add(packet);
        self.packets += 1;
    }

    /// How many packets there are, the silence at the end not among them.
    pub fn packets(&self) -> u64 {
        self.packets
    }

    fn add(&mut self, packet: &[u8]) {
        let segments = |p: &[u8]| p.len() / 255 + 1;
        let held: usize = self.waiting.iter().map(|p| segments(p)).sum();
        // A page holds at most 255 segments.
        if self.waiting.len() == PACKETS_PER_PAGE || held + segments(packet) > 255 {
            self.flush(0);
        }
        self.waiting.push(packet.to_vec());
    }

    fn flush(&mut self, kind: u8) {
        let waiting = std::mem::take(&mut self.waiting);
        self.written += waiting.len() as u64;
        let packets: Vec<&[u8]> = waiting.iter().map(Vec::as_slice).collect();
        self.out.extend(page(self.serial, self.sequence, kind, self.written * FRAME as u64, &packets));
        self.sequence += 1;
    }

    /// End the stream with the silence packet, and give the file.
    pub fn finish(mut self) -> Vec<u8> {
        self.add(&SILENCE);
        self.flush(END);
        self.out
    }
}

/// How loud the last 20 ms were, as the byte the waveform keeps.
pub struct Levels {
    window: Vec<i16>,
}

impl Levels {
    pub fn new() -> Self {
        Levels { window: Vec::with_capacity(FRAME) }
    }

    /// Take more samples, and give the level of every 20 ms they complete.
    pub fn push(&mut self, pcm: &[i16]) -> Vec<u8> {
        let mut levels = Vec::new();
        for &sample in pcm {
            self.window.push(sample);
            if self.window.len() == FRAME {
                levels.push(level(&self.window));
                self.window.clear();
            }
        }
        levels
    }
}

impl Default for Levels {
    fn default() -> Self {
        Self::new()
    }
}

/// The level of `pcm` in dBFS (its RMS against full scale) as the client's
/// `VoiceActivity` handler turns one into a byte: held between -100 and 0
/// and spread over 0 to 255 (`handleVoiceActivity`, 198620).
fn level(pcm: &[i16]) -> u8 {
    let mean = pcm.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / pcm.len() as f64;
    let db = 20.0 * (mean.sqrt() / 32768.0).log10();
    ((db.clamp(-100.0, 0.0) + 100.0) / 100.0 * 255.0).round() as u8
}

/// The `waveform` of a message: at most 256 levels, averaged down into
/// 256 when there are more (934581), as base64 (`el` in 841817).
pub fn waveform(levels: &[u8]) -> String {
    if levels.is_empty() {
        return String::new();
    }
    let bytes: Vec<u8> = if levels.len() > 256 {
        let size = levels.len() as f64 / 256.0;
        let mut from = 0;
        (1..=256)
            .map(|k| {
                let to = (k as f64 * size).round() as usize;
                let part = &levels[from..to.min(levels.len())];
                from = to;
                let mean = part.iter().map(|&l| f64::from(l)).sum::<f64>() / part.len() as f64;
                mean.round().min(255.0) as u8
            })
            .collect()
    } else {
        levels.to_vec()
    };
    glib::base64_encode(&bytes).to_string()
}

/// What a recorder has heard so far.
pub struct Capture {
    number: u32,
    ogg: OggOpus,
    levels: Levels,
    heard: Vec<u8>,
}

/// A finished recording, ready to upload.
#[derive(Clone)]
pub struct Recording {
    pub file: File,
    pub duration_secs: f64,
    pub waveform: String,
}

impl Capture {
    /// For the `n`th recording of this run.
    pub fn new(n: u32) -> Self {
        Capture { number: n, ogg: OggOpus::new(stream_serial(n)), levels: Levels::new(), heard: Vec::new() }
    }

    pub fn packet(&mut self, packet: &[u8]) {
        self.ogg.push(packet);
    }

    /// Take raw mono samples at 48 kHz, and give the levels they complete.
    pub fn samples(&mut self, pcm: &[i16]) -> Vec<u8> {
        let levels = self.levels.push(pcm);
        self.heard.extend_from_slice(&levels);
        levels
    }

    /// How long it has gone on, in milliseconds.
    pub fn millis(&self) -> u64 {
        self.ogg.packets() * 20
    }

    /// Close the recording. Nothing for one shorter than half a second.
    pub fn finish(self) -> Option<Recording> {
        let duration_secs = (self.millis() as f64) / 1e3;
        if duration_secs < MIN_SECS {
            return None;
        }
        let waveform = waveform(&self.heard);
        let bytes = glib::Bytes::from_owned(self.ogg.finish());
        let file = File {
            filename: file_name(self.number),
            size: bytes.len() as u64,
            content_type: CONTENT_TYPE.to_owned(),
            original_md5: None,
            data: Data::Memory(bytes),
        };
        Some(Recording { file, duration_secs, waveform })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page as it lies in a file.
    struct Parsed {
        kind: u8,
        granule: u64,
        serial: u32,
        sequence: u32,
        packets: Vec<Vec<u8>>,
        checksum_ok: bool,
    }

    fn parse(file: &[u8]) -> Vec<Parsed> {
        let mut pages = Vec::new();
        let mut at = 0;
        while at < file.len() {
            assert_eq!(&file[at..at + 5], b"OggS\0", "a page starts at {at}");
            let count = file[at + 26] as usize;
            let lacing = &file[at + 27..at + 27 + count];
            let size: usize = lacing.iter().map(|&s| s as usize).sum();
            let end = at + 27 + count + size;
            let mut sum_less = file[at..end].to_vec();
            sum_less[22..26].fill(0);
            let (mut packets, mut current, mut body) = (Vec::new(), 0, at + 27 + count);
            for &segment in lacing {
                current += segment as usize;
                if segment < 255 {
                    packets.push(file[body..body + current].to_vec());
                    body += current;
                    current = 0;
                }
            }
            assert_eq!(current, 0, "no packet runs on into the next page");
            pages.push(Parsed {
                kind: file[at + 5],
                granule: u64::from_le_bytes(file[at + 6..at + 14].try_into().unwrap()),
                serial: u32::from_le_bytes(file[at + 14..at + 18].try_into().unwrap()),
                sequence: u32::from_le_bytes(file[at + 18..at + 22].try_into().unwrap()),
                packets,
                checksum_ok: crc(&sum_less).to_le_bytes() == file[at + 22..at + 26],
            });
            at = end;
        }
        pages
    }

    fn recording(packets: usize, serial: u32) -> Vec<u8> {
        let mut ogg = OggOpus::new(serial);
        for i in 0..packets {
            ogg.push(&[0x78, i as u8, 7]);
        }
        ogg.finish()
    }

    #[test]
    fn the_first_page_is_the_clients_opus_head() {
        // Mono, pre-skip 0, 48000 Hz, gain 0, mapping family 0.
        let expected = "4f67675300020000000000000000\
            67458b6b00000000e78bb80101134f707573486561640101000080bb0000000000";
        let file = recording(1, 0x6b8b4567);
        let hex: String = file[..47].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, expected);
    }

    #[test]
    fn the_second_page_is_the_clients_opus_tags() {
        let file = recording(1, 0x6b8b4567);
        let pages = parse(&file);
        let tags = &pages[1];
        assert_eq!((tags.kind, tags.granule, tags.sequence), (0, 0, 1));
        let packet = &tags.packets[0];
        assert_eq!(packet.len(), 764);
        let mut expected =
            b"OpusTags\x17\0\0\0Opus provided by WebRTC\x01\0\0\0\x16\0\0\0ENCODER=Discord Client".to_vec();
        expected.resize(764, 0);
        assert_eq!(packet, &expected);
        // Three segments (255, 255, 254), and the checksum the client's own page for this stream has.
        let at = 47 - 1 + 1;
        assert_eq!(&file[at + 26..at + 30], [3, 255, 255, 254]);
        assert_eq!(&file[at + 22..at + 26], 0x29e8_a8c7u32.to_le_bytes());
    }

    #[test]
    fn fifty_packets_make_a_page_and_the_last_one_ends_the_stream() {
        // 234 packets and the silence: four pages of 50 and one of 35.
        let pages = parse(&recording(234, 0x6b8b4567));
        assert!(pages.iter().all(|p| p.checksum_ok && p.serial == 0x6b8b4567));
        assert_eq!(pages.iter().map(|p| p.sequence).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(pages.iter().map(|p| p.kind).collect::<Vec<_>>(), [2, 0, 0, 0, 0, 0, 4]);
        assert_eq!(pages.iter().map(|p| p.packets.len()).collect::<Vec<_>>(), [1, 1, 50, 50, 50, 50, 35]);
        // One second of 48 kHz samples a page, 960 for each packet.
        assert_eq!(pages.iter().map(|p| p.granule).collect::<Vec<_>>(), [0, 0, 48000, 96000, 144000, 192000, 225600]);
    }

    #[test]
    fn a_full_last_page_ends_the_stream_without_an_empty_one() {
        // 99 packets and the silence fill two pages exactly.
        let pages = parse(&recording(99, 1));
        assert_eq!(pages.iter().map(|p| p.packets.len()).collect::<Vec<_>>(), [1, 1, 50, 50]);
        assert_eq!(pages.last().map(|p| p.kind), Some(4));
        // And one more makes a third, of one packet.
        let pages = parse(&recording(100, 1));
        assert_eq!(pages.iter().map(|p| p.packets.len()).collect::<Vec<_>>(), [1, 1, 50, 50, 1]);
        assert_eq!(pages.last().map(|p| (p.kind, p.granule)), Some((4, 101 * 960)));
    }

    #[test]
    fn a_stream_ends_with_the_silence_packet() {
        let pages = parse(&recording(3, 9));
        let last = pages.last().unwrap();
        assert_eq!(last.packets.last().unwrap(), &[0xf8, 0xff, 0xfe]);
        assert_eq!(last.packets.len(), 4);
    }

    #[test]
    fn a_big_packet_takes_more_than_one_segment() {
        let mut ogg = OggOpus::new(1);
        ogg.push(&[7; 510]);
        ogg.push(&[8; 300]);
        let pages = parse(&ogg.finish());
        let lengths: Vec<usize> = pages[2].packets.iter().map(Vec::len).collect();
        assert_eq!(lengths, [510, 300, 3], "510 ends in an empty segment, 300 in 45");
        assert!(pages[2].checksum_ok);
    }

    #[test]
    fn a_page_never_has_more_than_255_segments() {
        let mut ogg = OggOpus::new(1);
        for _ in 0..50 {
            ogg.push(&[1; 1000]);
        }
        let pages = parse(&ogg.finish());
        assert!(pages.iter().all(|p| p.checksum_ok));
        let counts: Vec<usize> = pages[2..].iter().map(|p| p.packets.len()).collect();
        assert_eq!(counts.iter().sum::<usize>(), 51, "50 packets and the silence, none lost");
        assert!(counts.len() > 1, "split to fit");
        // Each page's granule counts every packet up to its last.
        let mut seen = 0u64;
        for page in &pages[2..] {
            seen += page.packets.len() as u64;
            assert_eq!(page.granule, seen * 960, "after {seen} packets");
        }
    }

    #[test]
    fn the_stream_serial_is_glibcs_rand() {
        // What the first values of an unseeded glibc `rand()` are, which are
        // the serials of the client's first two recordings.
        assert_eq!(stream_serial(0), 1_804_289_383);
        assert_eq!(stream_serial(1), 846_930_886);
        assert_eq!(stream_serial(2), 1_681_692_777);
        assert_eq!(stream_serial(3), 1_714_636_915);
        assert_eq!(stream_serial(9), 1_189_641_421);
        assert_eq!(stream_serial(19), 1_303_455_736);
    }

    #[test]
    fn anything_waiting_in_the_box_stops_a_recording() {
        assert!(!Waiting::default().blocks());
        let one = |waiting: Waiting| waiting.blocks();
        assert!(one(Waiting { editing: true, ..Default::default() }));
        assert!(one(Waiting { files: true, ..Default::default() }));
        assert!(one(Waiting { sticker: true, ..Default::default() }), "a pending sticker, as in 87207");
        assert!(one(Waiting { recording: true, ..Default::default() }));
    }

    #[test]
    fn recordings_are_numbered_from_zero() {
        assert_eq!(file_name(0), "0recording.ogg");
        assert_eq!(file_name(12), "12recording.ogg");
    }

    #[test]
    fn a_level_is_the_clients_byte_for_its_decibels() {
        assert_eq!(level(&[0; 960]), 0, "silence is the floor");
        // Full scale: 0 dBFS.
        assert_eq!(level(&[i16::MIN; 960]), 255);
        // Half of full scale is -6.02 dBFS: (93.98 / 100) * 255 = 239.7.
        assert_eq!(level(&[16384; 960]), 240);
        // The sign of a sample doesn't matter, only its size.
        assert_eq!(level(&[100, -100, 100, -100]), level(&[100; 4]));
    }

    #[test]
    fn a_level_comes_for_every_20_ms() {
        let mut levels = Levels::new();
        assert!(levels.push(&[500; 959]).is_empty());
        assert_eq!(levels.push(&[500; 1]).len(), 1);
        assert_eq!(levels.push(&[500; 1920]).len(), 2);
        // Buffers of any size add up to the same windows.
        let mut chunks = Levels::new();
        let all: Vec<u8> = (0..1000).flat_map(|_| chunks.push(&[300; 7])).collect();
        assert_eq!(all.len(), 7000 / 960);
    }

    #[test]
    fn a_waveform_keeps_what_fits_and_averages_the_rest() {
        assert_eq!(waveform(&[]), "");
        assert_eq!(waveform(&[0, 1, 2, 255]), "AAEC/w==");
        // 256 levels go as they are.
        let exact: Vec<u8> = (0..=255).collect();
        assert_eq!(glib::base64_decode(&waveform(&exact)).as_slice(), exact.as_slice());
        // Twice as many are averaged in pairs, rounding half up as JavaScript does.
        let twice: Vec<u8> = (0..512).map(|i| if i % 2 == 0 { 10 } else { 11 }).collect();
        let averaged = glib::base64_decode(&waveform(&twice));
        assert_eq!(averaged.len(), 256);
        assert!(averaged.iter().all(|&b| b == 11), "10.5 rounds to 11");
        // One more than 256 still comes out as 256, none of the buckets empty.
        let odd: Vec<u8> = (0..257).map(|i| (i % 200) as u8).collect();
        assert_eq!(glib::base64_decode(&waveform(&odd)).len(), 256);
        // Long recordings are 256 bytes too: base64 of 256 is 344 characters.
        assert_eq!(waveform(&vec![9; 60_000]).len(), 344);
    }

    #[test]
    fn a_recording_under_half_a_second_is_dropped() {
        let mut capture = Capture::new(0);
        for _ in 0..24 {
            capture.packet(&[0x78, 1, 2]);
        }
        assert_eq!(capture.millis(), 480);
        assert!(capture.finish().is_none());
        let mut capture = Capture::new(5);
        for _ in 0..25 {
            capture.packet(&[0x78, 1, 2]);
        }
        let recording = capture.finish().unwrap();
        assert_eq!(recording.duration_secs, 0.5);
        assert_eq!(recording.file.filename, "5recording.ogg");
        assert_eq!(recording.file.content_type, "audio/ogg");
    }

    #[test]
    fn a_recording_is_what_the_upload_and_the_message_name() {
        let mut capture = Capture::new(1);
        for i in 0..150 {
            capture.packet(&[0x68, i as u8]);
        }
        assert_eq!(capture.samples(&vec![16384; 960 * 150]).len(), 150);
        let recording = capture.finish().unwrap();
        assert_eq!(recording.duration_secs, 3.0);
        assert_eq!(glib::base64_decode(&recording.waveform).len(), 150, "under 256: one byte for each 20 ms");
        let Data::Memory(bytes) = &recording.file.data else { panic!("kept in memory") };
        assert_eq!(recording.file.size, bytes.len() as u64);
        assert_eq!(parse(bytes)[0].serial, 846_930_886);
    }

    #[test]
    fn offered_only_in_variant_one() {
        use serde_json::json;
        let assign = |variant: i64| {
            Experiments::from_ready(&json!({ "apex_experiments": { "assignments": { "1": { "10": { "assignments": [
                [crate::experiments::murmur3("2026-09-desktop-voice-message-recording"), variant, 0, 1]
            ] } } } } }))
        };
        let me = Snowflake(10);
        assert!(enabled(&assign(1), me));
        assert!(!enabled(&assign(0), me), "the control group has it off");
        assert!(!enabled(&Experiments::default(), me), "not assigned: the default config is off");
    }
}
