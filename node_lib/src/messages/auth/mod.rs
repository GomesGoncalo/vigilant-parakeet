pub mod key_exchange;
pub mod session_terminated;

use crate::error::NodeError;
pub use key_exchange::{KeyExchangeInit, KeyExchangeReply};
pub use session_terminated::SessionTerminated;

#[derive(Debug)]
pub enum Auth<'a> {
    /// OBU initiates a key negotiation with the server.
    KeyExchangeInit(KeyExchangeInit<'a>),
    /// Server replies to a key negotiation initiated by the OBU.
    KeyExchangeReply(KeyExchangeReply<'a>),
    /// Server revokes an OBU's session; target OBU must re-key immediately.
    SessionTerminated(SessionTerminated<'a>),
}

impl<'a> Auth<'a> {
    /// Wire size of the auth payload in bytes (excludes the 1-byte type tag).
    pub fn wire_size(&self) -> usize {
        match self {
            Auth::KeyExchangeInit(k) => k.wire_size(),
            Auth::KeyExchangeReply(k) => k.wire_size(),
            Auth::SessionTerminated(s) => s.wire_size(),
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for Auth<'a> {
    type Error = NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let next = value.get(1..).ok_or_else(|| NodeError::BufferTooShort {
            expected: 2,
            actual: value.len(),
        })?;

        match value.first() {
            Some(0u8) => Ok(Self::KeyExchangeInit(next.try_into()?)),
            Some(1u8) => Ok(Self::KeyExchangeReply(next.try_into()?)),
            Some(2u8) => Ok(Self::SessionTerminated(next.try_into()?)),
            _ => Err(NodeError::ParseError(
                "Invalid auth message type".to_string(),
            )),
        }
    }
}

impl<'a> From<&Auth<'a>> for Vec<u8> {
    fn from(value: &Auth<'a>) -> Self {
        let mut buf = Vec::with_capacity(64);
        match value {
            Auth::KeyExchangeInit(k) => {
                buf.push(0u8);
                let ke_bytes: Vec<u8> = k.into();
                buf.extend_from_slice(&ke_bytes);
            }
            Auth::KeyExchangeReply(k) => {
                buf.push(1u8);
                let ke_bytes: Vec<u8> = k.into();
                buf.extend_from_slice(&ke_bytes);
            }
            Auth::SessionTerminated(s) => {
                buf.push(2u8);
                let st_bytes: Vec<u8> = s.into();
                buf.extend_from_slice(&st_bytes);
            }
        }
        buf
    }
}
