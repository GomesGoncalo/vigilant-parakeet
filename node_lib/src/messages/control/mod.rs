pub mod heartbeat;
pub mod key_exchange;
pub mod session_terminated;

use crate::error::NodeError;
use heartbeat::{Heartbeat, HeartbeatReply};
use key_exchange::{KeyExchangeInit, KeyExchangeReply};
pub use session_terminated::SessionTerminated;

#[derive(Debug)]
pub enum Control<'a> {
    Heartbeat(Heartbeat<'a>),
    HeartbeatReply(HeartbeatReply<'a>),
    KeyExchangeInit(KeyExchangeInit<'a>),
    KeyExchangeReply(KeyExchangeReply<'a>),
    /// Session revocation notice from server: target OBU must re-key immediately.
    SessionTerminated(SessionTerminated<'a>),
}

impl<'a> TryFrom<&'a [u8]> for Control<'a> {
    type Error = NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let next = value.get(1..).ok_or_else(|| NodeError::BufferTooShort {
            expected: 2,
            actual: value.len(),
        })?;

        match value.first() {
            Some(0u8) => Ok(Self::Heartbeat(next.try_into()?)),
            Some(1u8) => Ok(Self::HeartbeatReply(next.try_into()?)),
            Some(2u8) => Ok(Self::KeyExchangeInit(next.try_into()?)),
            Some(3u8) => Ok(Self::KeyExchangeReply(next.try_into()?)),
            Some(4u8) => Ok(Self::SessionTerminated(next.try_into()?)),
            _ => Err(NodeError::ParseError(
                "Invalid control message type".to_string(),
            )),
        }
    }
}

impl<'a> From<&Control<'a>> for Vec<u8> {
    fn from(value: &Control<'a>) -> Self {
        let mut buf = Vec::with_capacity(64);
        match value {
            Control::Heartbeat(c) => {
                buf.push(0u8);
                let hb_bytes: Vec<u8> = c.into();
                buf.extend_from_slice(&hb_bytes);
            }
            Control::HeartbeatReply(c) => {
                buf.push(1u8);
                let hbr_bytes: Vec<u8> = c.into();
                buf.extend_from_slice(&hbr_bytes);
            }
            Control::KeyExchangeInit(c) => {
                buf.push(2u8);
                let ke_bytes: Vec<u8> = c.into();
                buf.extend_from_slice(&ke_bytes);
            }
            Control::KeyExchangeReply(c) => {
                buf.push(3u8);
                let ke_bytes: Vec<u8> = c.into();
                buf.extend_from_slice(&ke_bytes);
            }
            Control::SessionTerminated(c) => {
                buf.push(4u8);
                let st_bytes: Vec<u8> = c.into();
                buf.extend_from_slice(&st_bytes);
            }
        }
        buf
    }
}

// Keep backwards compatibility
impl<'a> From<&Control<'a>> for Vec<Vec<u8>> {
    fn from(value: &Control<'a>) -> Self {
        match value {
            Control::Heartbeat(c) => {
                let mut result = vec![vec![0u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
            Control::HeartbeatReply(c) => {
                let mut result = vec![vec![1u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
            Control::KeyExchangeInit(c) => {
                let mut result = vec![vec![2u8]];
                result.push(c.into());
                result
            }
            Control::KeyExchangeReply(c) => {
                let mut result = vec![vec![3u8]];
                result.push(c.into());
                result
            }
            Control::SessionTerminated(c) => {
                let mut result = vec![vec![4u8]];
                result.push(c.into());
                result
            }
        }
    }
}
