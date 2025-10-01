pub mod heartbeat;

use crate::error::NodeError;
use heartbeat::{Heartbeat, HeartbeatReply};

#[derive(Debug)]
pub enum Control<'a> {
    Heartbeat(Heartbeat<'a>),
    HeartbeatReply(HeartbeatReply<'a>),
}

impl<'a> TryFrom<&'a [u8]> for Control<'a> {
    type Error = NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let next = value.get(1..).ok_or_else(|| {
            NodeError::BufferTooShort {
                expected: 2,
                actual: value.len(),
            }
        })?;

        match value.first() {
            Some(0u8) => Ok(Self::Heartbeat(next.try_into()?)),
            Some(1u8) => Ok(Self::HeartbeatReply(next.try_into()?)),
            _ => Err(NodeError::ParseError(
                "Invalid control message type".to_string(),
            )),
        }
    }
}

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
        }
    }
}
