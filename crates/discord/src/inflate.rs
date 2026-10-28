//! The gateway's `zlib-stream` transport compression: the whole connection
//! is one zlib stream, and each payload ends with a sync flush, whose last
//! four bytes are `00 00 FF FF`. A payload may span several frames.

use gio::prelude::*;

const SYNC_FLUSH: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// One per connection: later payloads refer back to earlier ones, so the
/// context can't be reset or shared.
pub(crate) struct Inflater {
    zlib: gio::ZlibDecompressor,
    pending: Vec<u8>,
}

impl Inflater {
    pub(crate) fn new() -> Inflater {
        Inflater { zlib: gio::ZlibDecompressor::new(gio::ZlibCompressorFormat::Zlib), pending: Vec::new() }
    }

    /// Feed a frame. Returns the payload once its last frame is in.
    pub(crate) fn push(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>, glib::Error> {
        self.pending.extend_from_slice(frame);
        if !self.pending.ends_with(&SYNC_FLUSH) {
            return Ok(None);
        }
        let input = std::mem::take(&mut self.pending);
        let mut input = &input[..];
        let mut payload = Vec::with_capacity(input.len() * 4);
        let mut chunk = vec![0; 64 * 1024];
        loop {
            // Without FLUSH, running out of input is an error rather than a
            // FLUSHED result, whose byte counts GIO leaves unset.
            let (result, read, written) = match self.zlib.convert(input, &mut chunk[..], gio::ConverterFlags::NONE) {
                Ok(done) => done,
                Err(e) if input.is_empty() && e.matches(gio::IOErrorEnum::PartialInput) => break,
                Err(e) => return Err(e),
            };
            payload.extend_from_slice(&chunk[..written]);
            input = &input[read..];
            // Space left in the chunk means zlib has nothing more to give.
            let drained = input.is_empty() && written < chunk.len();
            if drained || result == gio::ConverterResult::Finished || (read == 0 && written == 0) {
                break;
            }
        }
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{"op":11,"d":null}` twice from one zlib context: the second is
    /// only a back-reference to the first.
    const HEARTBEAT_ACK: [u8; 26] = [
        0x78, 0x9c, 0xaa, 0x56, 0xca, 0x2f, 0x50, 0xb2, 0x32, 0x34, 0xd4, 0x51, 0x4a, 0x51, 0xb2, 0xca, 0x2b, 0xcd,
        0xc9, 0xa9, 0x05, 0x00, 0x00, 0x00, 0xff, 0xff,
    ];
    const HEARTBEAT_ACK_AGAIN: [u8; 9] = [0xaa, 0xc6, 0x10, 0x01, 0x00, 0x00, 0x00, 0xff, 0xff];

    #[test]
    fn payloads_share_the_context() {
        let mut inflater = Inflater::new();
        let ack = br#"{"op":11,"d":null}"#.to_vec();
        assert_eq!(inflater.push(&HEARTBEAT_ACK).unwrap(), Some(ack.clone()));
        assert_eq!(inflater.push(&HEARTBEAT_ACK_AGAIN).unwrap(), Some(ack));
    }

    #[test]
    fn payload_over_several_frames() {
        let mut inflater = Inflater::new();
        let (first, rest) = HEARTBEAT_ACK.split_at(10);
        assert_eq!(inflater.push(first).unwrap(), None);
        assert_eq!(inflater.push(rest).unwrap().as_deref(), Some(&br#"{"op":11,"d":null}"#[..]));
    }

    /// A payload larger than one output chunk, as READY is, in stored
    /// (uncompressed) deflate blocks ending with a sync flush.
    #[test]
    fn payload_larger_than_a_chunk() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut stream = vec![0x78, 0x01];
        for block in data.chunks(u16::MAX as usize) {
            let len = block.len() as u16;
            stream.push(0x00);
            stream.extend_from_slice(&len.to_le_bytes());
            stream.extend_from_slice(&(!len).to_le_bytes());
            stream.extend_from_slice(block);
        }
        stream.extend_from_slice(&[0x00, 0x00, 0x00, 0xff, 0xff]);
        assert_eq!(Inflater::new().push(&stream).unwrap(), Some(data));
    }

    #[test]
    fn corrupt_stream() {
        assert!(Inflater::new().push(&[0x12, 0x34, 0x00, 0x00, 0xff, 0xff]).is_err());
    }
}
