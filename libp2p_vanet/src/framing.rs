/// Magic bytes identifying an L2Transport frame (ASCII "L2").
pub const MAGIC: [u8; 2] = [0x4C, 0x32];

/// Overhead per frame: magic(2) + conn_id(4) + length(2).
pub const FRAME_OVERHEAD: usize = 8;

/// Maximum payload size (MTU 9000 minus overhead).
pub const MAX_PAYLOAD: usize = 8992;

/// Encode a payload into a framed L2Transport packet.
///
/// Wire layout: `MAGIC[2] | conn_id[4 BE] | length[2 BE] | payload[length]`
pub fn encode_frame(conn_id: u32, payload: &[u8]) -> Vec<u8> {
    debug_assert!(payload.len() <= MAX_PAYLOAD, "payload exceeds MTU");
    let mut frame = Vec::with_capacity(FRAME_OVERHEAD + payload.len());
    frame.extend_from_slice(&MAGIC);
    frame.extend_from_slice(&conn_id.to_be_bytes());
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Attempt to decode one frame from `buf`.
///
/// Returns `(conn_id, payload, bytes_consumed)` on success, or `None` when the
/// buffer is too short or the magic bytes do not match.
pub fn decode_frame(buf: &[u8]) -> Option<(u32, &[u8], usize)> {
    if buf.len() < FRAME_OVERHEAD {
        return None;
    }
    if buf[..2] != MAGIC {
        return None;
    }
    let conn_id = u32::from_be_bytes(buf[2..6].try_into().ok()?);
    let length = u16::from_be_bytes(buf[6..8].try_into().ok()?) as usize;
    let total = FRAME_OVERHEAD + length;
    if buf.len() < total {
        return None;
    }
    Some((conn_id, &buf[FRAME_OVERHEAD..total], total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let payload = b"hello vanet";
        let frame = encode_frame(42, payload);
        let (conn_id, decoded, consumed) = decode_frame(&frame).expect("decode");
        assert_eq!(conn_id, 42);
        assert_eq!(decoded, payload);
        assert_eq!(consumed, frame.len());
    }

    #[test]
    fn wrong_magic_returns_none() {
        let mut frame = encode_frame(1, b"data");
        frame[0] = 0x00; // corrupt magic
        assert!(decode_frame(&frame).is_none());
    }

    #[test]
    fn short_buffer_returns_none() {
        assert!(decode_frame(&[0x4C, 0x32, 0x00]).is_none());
    }

    #[test]
    fn empty_payload() {
        let frame = encode_frame(0, b"");
        let (conn_id, payload, consumed) = decode_frame(&frame).expect("decode");
        assert_eq!(conn_id, 0);
        assert!(payload.is_empty());
        assert_eq!(consumed, FRAME_OVERHEAD);
    }
}
