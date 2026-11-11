//! Chrome / Edge native messaging frames: a 4-byte little-endian length followed by
//! that many bytes of UTF-8 JSON, in both directions, over stdin / stdout.
//!
//! Limits (https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging):
//! * host → browser: 1 MiB per message. Anything larger makes Chrome kill the port,
//!   so [`write_frame`] refuses to send it and the caller sends an error frame instead.
//! * browser → host: up to 4 GiB in theory. We accept [`MAX_INBOUND`] (the same 16 MiB the
//!   relay ⇄ app contract allows) and skip anything bigger without buffering it.

use std::io::{self, Read, Write};

/// Largest frame the browser may be sent: 1 MiB (1 048 576 bytes).
pub const MAX_OUTBOUND: usize = 1024 * 1024;
/// Largest frame we accept from the browser (CONTRACTS.md §1: requests are at most 16 MiB).
pub const MAX_INBOUND: usize = 16 * 1024 * 1024;

/// What [`read_frame`] found on the stream.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    /// A complete frame (the JSON bytes, unparsed).
    Message(Vec<u8>),
    /// The frame announced more than [`MAX_INBOUND`] bytes. They were consumed and
    /// dropped so the stream stays in sync; carries the announced length.
    TooLarge(u64),
    /// The browser closed the pipe between frames: time to exit.
    Eof,
}

/// Reads one frame. `Err` means the stream is broken (truncated header or body) and the
/// host should exit: Chrome never sends a partial frame unless it died.
pub fn read_frame<R: Read>(input: &mut R) -> io::Result<Inbound> {
    let mut header = [0u8; 4];
    // Distinguish a clean close (0 bytes) from a truncated header.
    let mut got = 0;
    while got < header.len() {
        match input.read(&mut header[got..]) {
            Ok(0) if got == 0 => return Ok(Inbound::Eof),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated frame header")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(header) as u64;
    if len > MAX_INBOUND as u64 {
        let skipped = io::copy(&mut input.by_ref().take(len), &mut io::sink())?;
        if skipped < len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated oversized frame"));
        }
        return Ok(Inbound::TooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    input.read_exact(&mut body)?;
    Ok(Inbound::Message(body))
}

/// Why [`write_frame`] did not write.
#[derive(Debug)]
pub enum WriteError {
    /// The payload is over the 1 MiB host → browser cap; nothing was written.
    TooLarge(usize),
    Io(io::Error),
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        WriteError::Io(e)
    }
}

/// Writes one frame and flushes. Refuses payloads over [`MAX_OUTBOUND`] without
/// writing a single byte, so the caller can substitute an error message.
pub fn write_frame<W: Write>(out: &mut W, payload: &[u8]) -> Result<(), WriteError> {
    if payload.len() > MAX_OUTBOUND {
        return Err(WriteError::TooLarge(payload.len()));
    }
    // One write call: Chrome reads the header and body from the same pipe, and a
    // single buffer keeps a concurrent writer (there is none) from interleaving.
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    out.write_all(&buf)?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut v = (payload.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn round_trips_a_message() {
        let mut wire = Vec::new();
        write_frame(&mut wire, br#"{"op":"hello"}"#).unwrap();
        assert_eq!(&wire[..4], &14u32.to_le_bytes());
        let got = read_frame(&mut Cursor::new(wire)).unwrap();
        assert_eq!(got, Inbound::Message(br#"{"op":"hello"}"#.to_vec()));
    }

    #[test]
    fn header_is_little_endian() {
        let payload = vec![b'x'; 0x0102];
        let wire = framed(&payload);
        assert_eq!(&wire[..4], &[0x02, 0x01, 0x00, 0x00]);
        assert_eq!(read_frame(&mut Cursor::new(wire)).unwrap(), Inbound::Message(payload));
    }

    #[test]
    fn reads_consecutive_frames_then_eof() {
        let mut wire = framed(b"{}");
        wire.extend(framed(b"[1]"));
        let mut cur = Cursor::new(wire);
        assert_eq!(read_frame(&mut cur).unwrap(), Inbound::Message(b"{}".to_vec()));
        assert_eq!(read_frame(&mut cur).unwrap(), Inbound::Message(b"[1]".to_vec()));
        assert_eq!(read_frame(&mut cur).unwrap(), Inbound::Eof);
    }

    #[test]
    fn empty_frame_is_a_message() {
        assert_eq!(read_frame(&mut Cursor::new(framed(b""))).unwrap(), Inbound::Message(Vec::new()));
    }

    #[test]
    fn truncated_header_is_an_error() {
        let err = read_frame(&mut Cursor::new(vec![5, 0])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn truncated_body_is_an_error() {
        let mut wire = 10u32.to_le_bytes().to_vec();
        wire.extend_from_slice(b"abc");
        assert!(read_frame(&mut Cursor::new(wire)).is_err());
    }

    /// A reader that hands out one byte per call, like a slow pipe.
    struct Trickle(Cursor<Vec<u8>>);
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(1);
            self.0.read(&mut buf[..n])
        }
    }

    #[test]
    fn survives_short_reads() {
        let wire = framed(br#"{"op":"vault"}"#);
        let got = read_frame(&mut Trickle(Cursor::new(wire))).unwrap();
        assert_eq!(got, Inbound::Message(br#"{"op":"vault"}"#.to_vec()));
    }

    #[test]
    fn oversized_inbound_frame_is_skipped_and_stream_stays_in_sync() {
        let big = MAX_INBOUND + 1;
        let mut wire = (big as u32).to_le_bytes().to_vec();
        wire.resize(4 + big, b'a');
        wire.extend(framed(b"{}"));
        let mut cur = Cursor::new(wire);
        assert_eq!(read_frame(&mut cur).unwrap(), Inbound::TooLarge(big as u64));
        assert_eq!(read_frame(&mut cur).unwrap(), Inbound::Message(b"{}".to_vec()));
    }

    #[test]
    fn outbound_cap_is_one_mebibyte_exactly() {
        let mut sink = Vec::new();
        write_frame(&mut sink, &vec![b'a'; MAX_OUTBOUND]).unwrap();
        assert_eq!(sink.len(), 4 + MAX_OUTBOUND);

        let mut sink = Vec::new();
        match write_frame(&mut sink, &vec![b'a'; MAX_OUTBOUND + 1]) {
            Err(WriteError::TooLarge(n)) => assert_eq!(n, MAX_OUTBOUND + 1),
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert!(sink.is_empty(), "nothing may be written for an oversized frame");
    }
}
